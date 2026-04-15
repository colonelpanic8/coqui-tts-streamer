use std::{
    fs,
    io::{self, Read},
    path::PathBuf,
    sync::Arc,
    time::Duration,
};

use anyhow::{Context, Result};
use clap::{Parser, ValueEnum};
use serde_json::json;
use streamer_audio_process::ProcessAudioOutput;
use streamer_core::{
    AppEvent, PipelineConfig, ReaderCommand, SegmentMode, SegmenterConfig, segment_document,
    spawn_pipeline,
};
use streamer_tts_coqui::{CoquiConfig, CoquiTtsEngine};
use streamer_ui_tui::run_tui;

#[derive(Clone, Copy, Debug, ValueEnum)]
enum CliSegmentMode {
    Paragraph,
    Sentence,
}

impl From<CliSegmentMode> for SegmentMode {
    fn from(value: CliSegmentMode) -> Self {
        match value {
            CliSegmentMode::Paragraph => SegmentMode::Paragraph,
            CliSegmentMode::Sentence => SegmentMode::Sentence,
        }
    }
}

#[derive(Parser, Debug)]
#[command(name = "coqui-tts-streamer")]
#[command(about = "Incremental Coqui article reader with buffered playback and optional TUI.")]
struct Cli {
    #[arg(long)]
    file: Option<PathBuf>,
    #[arg(long, default_value = "http://[::1]:11115")]
    host: String,
    #[arg(long)]
    speaker: Option<String>,
    #[arg(long)]
    language: Option<String>,
    #[arg(long, default_value_t = 320)]
    target_chars: usize,
    #[arg(long, default_value_t = 550)]
    max_chars: usize,
    #[arg(long, value_enum, default_value_t = CliSegmentMode::Paragraph)]
    segment_mode: CliSegmentMode,
    #[arg(long, default_value_t = 10.0)]
    prebuffer_secs: f32,
    #[arg(long)]
    max_buffered_secs: Option<f32>,
    #[arg(long, default_value_t = 2)]
    max_retries: usize,
    #[arg(long, default_value_t = 2)]
    synthesis_concurrency: usize,
    #[arg(long)]
    tui: bool,
    #[arg(long)]
    json_events: bool,
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    let raw_text = read_input(cli.file.as_ref())?;

    let segmenter_config = SegmenterConfig {
        target_chars: cli.target_chars,
        max_chars: cli.max_chars,
        mode: cli.segment_mode.into(),
    };
    let (document, segments) = segment_document(None, &raw_text, &segmenter_config)?;

    let pipeline_config = PipelineConfig {
        prebuffer_audio: Duration::from_secs_f32(cli.prebuffer_secs),
        max_buffered_audio: cli
            .max_buffered_secs
            .map(|secs| Duration::from_secs_f32(secs.max(cli.prebuffer_secs))),
        max_retries: cli.max_retries,
        max_concurrent_synthesis: cli.synthesis_concurrency,
    };

    let tts_engine = Arc::new(CoquiTtsEngine::new(CoquiConfig {
        base_url: cli.host,
        speaker: cli.speaker,
        language: cli.language,
    })?);
    let audio_output = Arc::new(ProcessAudioOutput::new()?);
    let pipeline = spawn_pipeline(
        document,
        segments,
        pipeline_config,
        tts_engine,
        audio_output,
    );

    let state_rx = pipeline.state_rx.clone();
    let command_tx = pipeline.command_tx.clone();
    let shutdown_tx = pipeline.command_tx.clone();
    let mut event_rx = pipeline.event_rx;
    let join_handle = pipeline.join_handle;

    let printer_task = if cli.tui {
        None
    } else {
        Some(tokio::spawn(async move {
            while let Ok(event) = event_rx.recv().await {
                print_event(&event, cli.json_events);
            }
        }))
    };

    let tui_task = if cli.tui {
        Some(tokio::task::spawn_blocking(move || {
            run_tui(state_rx, command_tx)
        }))
    } else {
        None
    };

    let signal_task = tokio::spawn(async move {
        let _ = tokio::signal::ctrl_c().await;
        let _ = shutdown_tx.send(ReaderCommand::Quit);
    });

    let pipeline_result = join_handle.await.context("pipeline task failed to join")?;
    signal_task.abort();

    if let Some(task) = printer_task {
        task.abort();
    }

    if let Some(task) = tui_task {
        task.await.context("TUI task failed to join")??;
    }

    pipeline_result
}

fn read_input(path: Option<&PathBuf>) -> Result<String> {
    if let Some(path) = path {
        return fs::read_to_string(path)
            .with_context(|| format!("failed to read {}", path.display()));
    }

    let mut buffer = String::new();
    io::stdin()
        .read_to_string(&mut buffer)
        .context("failed to read stdin")?;
    if buffer.trim().is_empty() {
        anyhow::bail!("no input text supplied via stdin or --file");
    }
    Ok(buffer)
}

fn print_event(event: &AppEvent, json_events: bool) {
    if json_events {
        let payload = match event {
            AppEvent::SegmentQueued {
                segment_id,
                buffered_audio,
            } => json!({
                "event": "segment_queued",
                "segment_id": segment_id,
                "buffered_audio_secs": buffered_audio.as_secs_f32(),
            }),
            AppEvent::SegmentSynthesized {
                segment_id,
                duration,
            } => json!({
                "event": "segment_synthesized",
                "segment_id": segment_id,
                "duration_secs": duration.as_secs_f32(),
            }),
            AppEvent::Playback(playback_event) => json!({
                "event": "playback",
                "payload": format!("{playback_event:?}"),
            }),
            AppEvent::Error(message) => json!({
                "event": "error",
                "message": message,
            }),
            AppEvent::Completed => json!({
                "event": "completed",
            }),
        };
        println!("{payload}");
        return;
    }

    match event {
        AppEvent::SegmentQueued {
            segment_id,
            buffered_audio,
        } => eprintln!(
            "queued segment {} ({:.1}s buffered)",
            segment_id,
            buffered_audio.as_secs_f32()
        ),
        AppEvent::SegmentSynthesized {
            segment_id,
            duration,
        } => eprintln!(
            "synthesized segment {} ({:.1}s audio)",
            segment_id,
            duration.as_secs_f32()
        ),
        AppEvent::Playback(playback_event) => eprintln!("playback {playback_event:?}"),
        AppEvent::Error(message) => eprintln!("error: {message}"),
        AppEvent::Completed => eprintln!("completed"),
    }
}
