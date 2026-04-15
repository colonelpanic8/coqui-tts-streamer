use std::{
    collections::{BTreeMap, VecDeque},
    sync::Arc,
    time::Duration,
};

use anyhow::{Result, anyhow};
use tokio::{
    select,
    sync::{broadcast, mpsc, watch},
    task::{JoinHandle, JoinSet},
};

use crate::{
    AppEvent, AppState, AudioChunk, AudioOutput, PlaybackEvent, PlaybackItem, PlaybackState,
    ReaderCommand, Segment, TtsEngine,
};

#[derive(Clone, Debug)]
pub struct PipelineConfig {
    pub prebuffer_audio: Duration,
    pub max_buffered_audio: Option<Duration>,
    pub max_retries: usize,
    pub max_concurrent_synthesis: usize,
}

impl Default for PipelineConfig {
    fn default() -> Self {
        Self {
            prebuffer_audio: Duration::from_secs(10),
            max_buffered_audio: None,
            max_retries: 2,
            max_concurrent_synthesis: 2,
        }
    }
}

pub struct PipelineHandle {
    pub state_rx: watch::Receiver<AppState>,
    pub event_rx: broadcast::Receiver<AppEvent>,
    pub command_tx: mpsc::UnboundedSender<ReaderCommand>,
    pub join_handle: JoinHandle<Result<()>>,
}

struct SynthResult {
    segment: Segment,
    attempt: usize,
    result: Result<AudioChunk>,
}

pub fn spawn_pipeline(
    document: crate::Document,
    segments: Vec<Segment>,
    config: PipelineConfig,
    tts_engine: Arc<dyn TtsEngine>,
    audio_output: Arc<dyn AudioOutput>,
) -> PipelineHandle {
    let initial_state = AppState::new(document, segments);
    let (state_tx, state_rx) = watch::channel(initial_state);
    let (event_tx, event_rx) = broadcast::channel(256);
    let (command_tx, command_rx) = mpsc::unbounded_channel();
    let join_handle = tokio::spawn(run_pipeline(
        config,
        state_tx,
        event_tx,
        command_rx,
        tts_engine,
        audio_output,
    ));

    PipelineHandle {
        state_rx,
        event_rx,
        command_tx,
        join_handle,
    }
}

async fn run_pipeline(
    config: PipelineConfig,
    state_tx: watch::Sender<AppState>,
    event_tx: broadcast::Sender<AppEvent>,
    mut command_rx: mpsc::UnboundedReceiver<ReaderCommand>,
    tts_engine: Arc<dyn TtsEngine>,
    audio_output: Arc<dyn AudioOutput>,
) -> Result<()> {
    let mut state = state_tx.borrow().clone();
    let total_segments = state.total_segments();
    if total_segments == 0 {
        state.playback_state = PlaybackState::Completed;
        publish_state(&state_tx, &state);
        let _ = event_tx.send(AppEvent::Completed);
        return Ok(());
    }

    let mut playback_rx = audio_output.subscribe();
    let mut synth_tasks = JoinSet::new();

    let mut next_to_schedule = 0usize;
    let mut next_to_enqueue = 0usize;
    let mut in_flight = 0usize;
    let mut prebuffer_queue = VecDeque::new();
    let mut synthesized_pending = BTreeMap::new();

    maybe_schedule_more(
        &config,
        &mut state,
        &state_tx,
        &tts_engine,
        &mut synth_tasks,
        &mut next_to_schedule,
        &mut in_flight,
    )?;

    loop {
        if state.played_segments >= total_segments {
            state.playback_state = PlaybackState::Completed;
            publish_state(&state_tx, &state);
            let _ = event_tx.send(AppEvent::Completed);
            let _ = audio_output.stop().await;
            break;
        }

        select! {
            Some(command) = command_rx.recv() => {
                match command {
                    ReaderCommand::Pause => {
                        audio_output.pause().await?;
                    }
                    ReaderCommand::Resume => {
                        audio_output.resume().await?;
                    }
                    ReaderCommand::TogglePause => {
                        if state.playback_state == PlaybackState::Paused {
                            audio_output.resume().await?;
                        } else {
                            audio_output.pause().await?;
                        }
                    }
                    ReaderCommand::Quit => {
                        audio_output.stop().await?;
                        synth_tasks.abort_all();
                        state.playback_state = PlaybackState::Stopped;
                        publish_state(&state_tx, &state);
                        break;
                    }
                }
            }
            Some(join_result) = synth_tasks.join_next(), if in_flight > 0 => {
                in_flight = in_flight.saturating_sub(1);
                let result = match join_result {
                    Ok(result) => result,
                    Err(error) if error.is_cancelled() => {
                        continue;
                    }
                    Err(error) => return Err(error.into()),
                };
                match result.result {
                    Ok(chunk) => {
                        state.generated_segments += 1;
                        state.mark_buffered(result.segment.id, chunk.duration);
                        let _ = event_tx.send(AppEvent::SegmentSynthesized {
                            segment_id: result.segment.id,
                            duration: chunk.duration,
                        });

                        synthesized_pending.insert(result.segment.id, PlaybackItem {
                            segment: result.segment,
                            chunk,
                        });

                        flush_ready_segments(
                            &config,
                            &mut state,
                            &event_tx,
                            audio_output.as_ref(),
                            &mut synthesized_pending,
                            &mut prebuffer_queue,
                            &mut next_to_enqueue,
                            next_to_schedule,
                            total_segments,
                            in_flight,
                        )
                        .await?;
                    }
                    Err(error) => {
                        let message = error.to_string();
                        if result.attempt < config.max_retries {
                            state.mark_synthesizing(result.segment.id, result.attempt + 1);
                            publish_state(&state_tx, &state);
                            spawn_synthesis(
                                &tts_engine,
                                &mut synth_tasks,
                                result.segment,
                                result.attempt + 1,
                            );
                            in_flight += 1;
                            continue;
                        } else {
                            state.mark_failed(result.segment.id, message.clone());
                            state.playback_state = PlaybackState::Error;
                            state.fatal_error = Some(message.clone());
                            publish_state(&state_tx, &state);
                            let _ = event_tx.send(AppEvent::Error(message.clone()));
                            let _ = audio_output.stop().await;
                            synth_tasks.abort_all();
                            return Err(anyhow!(message));
                        }
                    }
                }

                publish_state(&state_tx, &state);
                maybe_schedule_more(
                    &config,
                    &mut state,
                    &state_tx,
                    &tts_engine,
                    &mut synth_tasks,
                    &mut next_to_schedule,
                    &mut in_flight,
                )?;
            }
            Ok(playback_event) = playback_rx.recv() => {
                match playback_event.clone() {
                    PlaybackEvent::SegmentStarted { segment_id, .. } => {
                        state.mark_playing(segment_id);
                    }
                    PlaybackEvent::SegmentFinished { segment_id, duration } => {
                        state.mark_played(segment_id);
                        state.played_segments += 1;
                        state.buffered_audio = state.buffered_audio.saturating_sub(duration);
                        if state.played_segments < total_segments {
                            state.playback_state = if state.buffered_audio.is_zero() {
                                PlaybackState::Starved
                            } else {
                                PlaybackState::Buffering
                            };
                        }
                    }
                    PlaybackEvent::Paused => {
                        state.playback_state = PlaybackState::Paused;
                    }
                    PlaybackEvent::Resumed => {
                        state.playback_state = PlaybackState::Playing;
                    }
                    PlaybackEvent::Starved => {
                        if state.played_segments < total_segments {
                            state.playback_state = PlaybackState::Starved;
                        }
                    }
                    PlaybackEvent::Stopped => {
                        state.playback_state = PlaybackState::Stopped;
                    }
                    PlaybackEvent::Error(message) => {
                        state.playback_state = PlaybackState::Error;
                        state.fatal_error = Some(message.clone());
                        let _ = event_tx.send(AppEvent::Error(message));
                    }
                }

                let _ = event_tx.send(AppEvent::Playback(playback_event));
                publish_state(&state_tx, &state);
                maybe_schedule_more(
                    &config,
                    &mut state,
                    &state_tx,
                    &tts_engine,
                    &mut synth_tasks,
                    &mut next_to_schedule,
                    &mut in_flight,
                )?;
            }
        }
    }

    synth_tasks.abort_all();
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn flush_ready_segments(
    config: &PipelineConfig,
    state: &mut AppState,
    event_tx: &broadcast::Sender<AppEvent>,
    audio_output: &dyn AudioOutput,
    synthesized_pending: &mut BTreeMap<usize, PlaybackItem>,
    prebuffer_queue: &mut VecDeque<PlaybackItem>,
    next_to_enqueue: &mut usize,
    next_to_schedule: usize,
    total_segments: usize,
    in_flight: usize,
) -> Result<()> {
    while let Some(item) = synthesized_pending.remove(next_to_enqueue) {
        state.buffered_audio += item.chunk.duration;
        if state.started_playback {
            audio_output.enqueue(item.clone()).await?;
            let _ = event_tx.send(AppEvent::SegmentQueued {
                segment_id: item.segment.id,
                buffered_audio: state.buffered_audio,
            });
        } else {
            prebuffer_queue.push_back(item);
        }
        *next_to_enqueue += 1;
    }

    let ready_to_start =
        state.buffered_audio >= config.prebuffer_audio && !prebuffer_queue.is_empty();
    let all_audio_ready =
        next_to_schedule >= total_segments && in_flight == 0 && !prebuffer_queue.is_empty();
    if !state.started_playback && (ready_to_start || all_audio_ready) {
        while let Some(item) = prebuffer_queue.pop_front() {
            audio_output.enqueue(item.clone()).await?;
            let _ = event_tx.send(AppEvent::SegmentQueued {
                segment_id: item.segment.id,
                buffered_audio: state.buffered_audio,
            });
        }
        state.started_playback = true;
    }

    Ok(())
}

fn maybe_schedule_more(
    config: &PipelineConfig,
    state: &mut AppState,
    state_tx: &watch::Sender<AppState>,
    tts_engine: &Arc<dyn TtsEngine>,
    synth_tasks: &mut JoinSet<SynthResult>,
    next_to_schedule: &mut usize,
    in_flight: &mut usize,
) -> Result<()> {
    if config.max_concurrent_synthesis == 0 {
        return Err(anyhow!("max_concurrent_synthesis must be positive"));
    }

    while *next_to_schedule < state.total_segments() && *in_flight < config.max_concurrent_synthesis
    {
        if state.started_playback
            && config
                .max_buffered_audio
                .is_some_and(|limit| state.buffered_audio >= limit)
        {
            break;
        }

        let segment = state.segments[*next_to_schedule].clone();
        state.mark_synthesizing(segment.id, 1);
        state.next_segment_to_generate = *next_to_schedule + 1;
        publish_state(state_tx, state);

        spawn_synthesis(tts_engine, synth_tasks, segment, 1);
        *next_to_schedule += 1;
        *in_flight += 1;
    }

    Ok(())
}

fn spawn_synthesis(
    tts_engine: &Arc<dyn TtsEngine>,
    synth_tasks: &mut JoinSet<SynthResult>,
    segment: Segment,
    attempt: usize,
) {
    let synth_engine = tts_engine.clone();
    synth_tasks.spawn(async move {
        let result = synth_engine.synthesize(segment.clone()).await;
        SynthResult {
            segment,
            attempt,
            result,
        }
    });
}

fn publish_state(state_tx: &watch::Sender<AppState>, state: &AppState) {
    let _ = state_tx.send(state.clone());
}

#[cfg(test)]
mod tests {
    use std::{
        sync::{Arc, Mutex},
        time::Duration,
    };

    use anyhow::Result;
    use async_trait::async_trait;
    use tokio::sync::{Notify, broadcast, mpsc};
    use tokio::time::timeout;

    use crate::{
        AudioOutput, PlaybackEvent, PlaybackItem, ReaderCommand, Segment, SegmentMode,
        SegmenterConfig, TtsEngine, audio::AudioChunk, segmenter::segment_document,
    };

    use super::{PipelineConfig, spawn_pipeline};

    struct FakeTtsEngine {
        seen: Arc<Mutex<Vec<usize>>>,
        duration: Duration,
    }

    #[async_trait]
    impl TtsEngine for FakeTtsEngine {
        async fn synthesize(&self, segment: Segment) -> Result<AudioChunk> {
            self.seen.lock().unwrap().push(segment.id);
            Ok(AudioChunk::new(segment.id, vec![1, 2, 3, 4], self.duration))
        }
    }

    struct BlockingTtsEngine {
        started_tx: mpsc::UnboundedSender<usize>,
        release: Arc<Notify>,
        duration: Duration,
    }

    #[async_trait]
    impl TtsEngine for BlockingTtsEngine {
        async fn synthesize(&self, segment: Segment) -> Result<AudioChunk> {
            let _ = self.started_tx.send(segment.id);
            self.release.notified().await;
            Ok(AudioChunk::new(segment.id, vec![1, 2, 3, 4], self.duration))
        }
    }

    struct PerSegmentBlockingTtsEngine {
        started_tx: mpsc::UnboundedSender<usize>,
        releases: Vec<Arc<Notify>>,
        duration: Duration,
    }

    #[async_trait]
    impl TtsEngine for PerSegmentBlockingTtsEngine {
        async fn synthesize(&self, segment: Segment) -> Result<AudioChunk> {
            let _ = self.started_tx.send(segment.id);
            self.releases[segment.id].notified().await;
            Ok(AudioChunk::new(segment.id, vec![1, 2, 3, 4], self.duration))
        }
    }

    struct FakeAudioOutput {
        queued: Arc<Mutex<Vec<usize>>>,
        event_tx: broadcast::Sender<PlaybackEvent>,
    }

    #[async_trait]
    impl AudioOutput for FakeAudioOutput {
        async fn enqueue(&self, item: PlaybackItem) -> Result<()> {
            self.queued.lock().unwrap().push(item.segment.id);
            let _ = self.event_tx.send(PlaybackEvent::SegmentStarted {
                segment_id: item.segment.id,
                duration: item.chunk.duration,
            });
            let _ = self.event_tx.send(PlaybackEvent::SegmentFinished {
                segment_id: item.segment.id,
                duration: item.chunk.duration,
            });
            Ok(())
        }

        async fn pause(&self) -> Result<()> {
            let _ = self.event_tx.send(PlaybackEvent::Paused);
            Ok(())
        }

        async fn resume(&self) -> Result<()> {
            let _ = self.event_tx.send(PlaybackEvent::Resumed);
            Ok(())
        }

        async fn stop(&self) -> Result<()> {
            let _ = self.event_tx.send(PlaybackEvent::Stopped);
            Ok(())
        }

        fn subscribe(&self) -> broadcast::Receiver<PlaybackEvent> {
            self.event_tx.subscribe()
        }
    }

    #[tokio::test]
    async fn pipeline_completes_for_multi_segment_document() {
        let text = concat!(
            "Segment one starts with enough text to avoid collapsing into a tiny chunk. ",
            "Segment two should be forced by the target size. Segment three should also exist.\n\n",
            "A second paragraph gives the coordinator more than one chunk to schedule and play."
        );
        let (document, segments) = segment_document(
            None,
            text,
            &SegmenterConfig {
                target_chars: 45,
                max_chars: 70,
                mode: SegmentMode::Paragraph,
            },
        )
        .unwrap();
        assert!(segments.len() >= 3);

        let synthesized = Arc::new(Mutex::new(Vec::new()));
        let queued = Arc::new(Mutex::new(Vec::new()));
        let tts_engine = Arc::new(FakeTtsEngine {
            seen: synthesized.clone(),
            duration: Duration::from_millis(40),
        });
        let (event_tx, _) = broadcast::channel(64);
        let audio_output = Arc::new(FakeAudioOutput {
            queued: queued.clone(),
            event_tx,
        });

        let pipeline = spawn_pipeline(
            document,
            segments.clone(),
            PipelineConfig {
                prebuffer_audio: Duration::from_millis(20),
                max_buffered_audio: Some(Duration::from_secs(1)),
                max_retries: 1,
                max_concurrent_synthesis: 2,
            },
            tts_engine,
            audio_output,
        );

        timeout(Duration::from_secs(2), pipeline.join_handle)
            .await
            .expect("pipeline timed out")
            .expect("pipeline task join failed")
            .expect("pipeline returned an error");

        let final_state = pipeline.state_rx.borrow().clone();
        assert_eq!(final_state.generated_segments, segments.len());
        assert_eq!(final_state.played_segments, segments.len());
        assert_eq!(synthesized.lock().unwrap().len(), segments.len());
        assert_eq!(queued.lock().unwrap().len(), segments.len());
    }

    #[tokio::test]
    async fn pipeline_schedules_multiple_synthesis_jobs_up_front() {
        let text = concat!(
            "Segment one starts with enough text to avoid collapsing into a tiny chunk. ",
            "Segment two should be forced by the target size. Segment three should also exist.\n\n",
            "A second paragraph gives the coordinator more than one chunk to schedule and play."
        );
        let (document, segments) = segment_document(
            None,
            text,
            &SegmenterConfig {
                target_chars: 45,
                max_chars: 70,
                mode: SegmentMode::Paragraph,
            },
        )
        .unwrap();
        assert!(segments.len() >= 3);

        let (started_tx, mut started_rx) = mpsc::unbounded_channel();
        let release = Arc::new(Notify::new());
        let tts_engine = Arc::new(BlockingTtsEngine {
            started_tx,
            release: release.clone(),
            duration: Duration::from_millis(40),
        });
        let (event_tx, _) = broadcast::channel(64);
        let audio_output = Arc::new(FakeAudioOutput {
            queued: Arc::new(Mutex::new(Vec::new())),
            event_tx,
        });

        let pipeline = spawn_pipeline(
            document,
            segments,
            PipelineConfig {
                prebuffer_audio: Duration::from_secs(5),
                max_buffered_audio: Some(Duration::from_secs(5)),
                max_retries: 1,
                max_concurrent_synthesis: 2,
            },
            tts_engine,
            audio_output,
        );

        let first = timeout(Duration::from_secs(1), started_rx.recv())
            .await
            .expect("timed out waiting for first synthesis task")
            .expect("missing first synthesis task");
        let second = timeout(Duration::from_secs(1), started_rx.recv())
            .await
            .expect("timed out waiting for second synthesis task")
            .expect("missing second synthesis task");
        assert_eq!((first, second), (0, 1));

        pipeline
            .command_tx
            .send(ReaderCommand::Quit)
            .expect("failed to stop pipeline");
        release.notify_waiters();

        timeout(Duration::from_secs(2), pipeline.join_handle)
            .await
            .expect("pipeline timed out")
            .expect("pipeline task join failed")
            .expect("pipeline returned an error");
    }

    #[tokio::test]
    async fn pipeline_preserves_playback_order_with_out_of_order_synthesis() {
        let text = concat!(
            "Segment one starts with enough text to avoid collapsing into a tiny chunk. ",
            "Segment two should be forced by the target size. Segment three should also exist.\n\n",
            "A second paragraph gives the coordinator more than one chunk to schedule and play."
        );
        let (document, segments) = segment_document(
            None,
            text,
            &SegmenterConfig {
                target_chars: 45,
                max_chars: 70,
                mode: SegmentMode::Paragraph,
            },
        )
        .unwrap();
        assert!(segments.len() >= 3);
        let total_segments = segments.len();

        let queued = Arc::new(Mutex::new(Vec::new()));
        let (started_tx, mut started_rx) = mpsc::unbounded_channel();
        let releases = (0..total_segments)
            .map(|_| Arc::new(Notify::new()))
            .collect::<Vec<_>>();
        let tts_engine = Arc::new(PerSegmentBlockingTtsEngine {
            started_tx,
            releases: releases.clone(),
            duration: Duration::from_millis(40),
        });
        let (event_tx, _) = broadcast::channel(64);
        let audio_output = Arc::new(FakeAudioOutput {
            queued: queued.clone(),
            event_tx,
        });

        let pipeline = spawn_pipeline(
            document,
            segments,
            PipelineConfig {
                prebuffer_audio: Duration::from_millis(20),
                max_buffered_audio: Some(Duration::from_secs(1)),
                max_retries: 1,
                max_concurrent_synthesis: 2,
            },
            tts_engine,
            audio_output,
        );

        let first = timeout(Duration::from_secs(1), started_rx.recv())
            .await
            .expect("timed out waiting for first synthesis task")
            .expect("missing first synthesis task");
        let second = timeout(Duration::from_secs(1), started_rx.recv())
            .await
            .expect("timed out waiting for second synthesis task")
            .expect("missing second synthesis task");
        assert_eq!((first, second), (0, 1));

        releases[1].notify_waiters();
        releases[0].notify_waiters();

        for _ in 2..total_segments {
            let next = timeout(Duration::from_secs(1), started_rx.recv())
                .await
                .expect("timed out waiting for synthesis task")
                .expect("missing synthesis task");
            releases[next].notify_waiters();
        }

        timeout(Duration::from_secs(2), pipeline.join_handle)
            .await
            .expect("pipeline timed out")
            .expect("pipeline task join failed")
            .expect("pipeline returned an error");

        assert_eq!(
            *queued.lock().unwrap(),
            (0..total_segments).collect::<Vec<_>>()
        );
    }
}
