use std::{sync::Arc, time::Duration};

use crate::{Document, Segment};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SegmentStatus {
    Pending,
    Synthesizing,
    Buffered,
    Playing,
    Played,
    Failed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PlaybackState {
    Buffering,
    Playing,
    Paused,
    Starved,
    Completed,
    Stopped,
    Error,
}

#[derive(Clone, Debug)]
pub struct SegmentRuntime {
    pub status: SegmentStatus,
    pub attempts: usize,
    pub duration: Option<Duration>,
    pub last_error: Option<String>,
}

impl Default for SegmentRuntime {
    fn default() -> Self {
        Self {
            status: SegmentStatus::Pending,
            attempts: 0,
            duration: None,
            last_error: None,
        }
    }
}

#[derive(Clone, Debug)]
pub struct AppState {
    pub document: Document,
    pub segments: Arc<Vec<Segment>>,
    pub runtimes: Vec<SegmentRuntime>,
    pub generated_segments: usize,
    pub played_segments: usize,
    pub current_segment_id: Option<usize>,
    pub buffered_audio: Duration,
    pub playback_state: PlaybackState,
    pub next_segment_to_generate: usize,
    pub started_playback: bool,
    pub fatal_error: Option<String>,
}

impl AppState {
    pub fn new(document: Document, segments: Vec<Segment>) -> Self {
        let count = segments.len();
        Self {
            document,
            segments: Arc::new(segments),
            runtimes: vec![SegmentRuntime::default(); count],
            generated_segments: 0,
            played_segments: 0,
            current_segment_id: None,
            buffered_audio: Duration::ZERO,
            playback_state: PlaybackState::Buffering,
            next_segment_to_generate: 0,
            started_playback: false,
            fatal_error: None,
        }
    }

    pub fn total_segments(&self) -> usize {
        self.segments.len()
    }

    pub fn mark_synthesizing(&mut self, segment_id: usize, attempt: usize) {
        if let Some(runtime) = self.runtimes.get_mut(segment_id) {
            runtime.status = SegmentStatus::Synthesizing;
            runtime.attempts = attempt;
            runtime.last_error = None;
        }
    }

    pub fn mark_buffered(&mut self, segment_id: usize, duration: Duration) {
        if let Some(runtime) = self.runtimes.get_mut(segment_id) {
            runtime.status = SegmentStatus::Buffered;
            runtime.duration = Some(duration);
            runtime.last_error = None;
        }
    }

    pub fn mark_playing(&mut self, segment_id: usize) {
        self.current_segment_id = Some(segment_id);
        self.playback_state = PlaybackState::Playing;
        if let Some(runtime) = self.runtimes.get_mut(segment_id) {
            runtime.status = SegmentStatus::Playing;
        }
    }

    pub fn mark_played(&mut self, segment_id: usize) {
        self.current_segment_id = None;
        if let Some(runtime) = self.runtimes.get_mut(segment_id) {
            runtime.status = SegmentStatus::Played;
        }
    }

    pub fn mark_failed(&mut self, segment_id: usize, error: String) {
        if let Some(runtime) = self.runtimes.get_mut(segment_id) {
            runtime.status = SegmentStatus::Failed;
            runtime.last_error = Some(error);
        }
    }
}
