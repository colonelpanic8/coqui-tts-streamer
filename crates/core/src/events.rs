use std::time::Duration;

#[derive(Clone, Debug)]
pub enum ReaderCommand {
    Pause,
    Resume,
    TogglePause,
    Quit,
}

#[derive(Clone, Debug)]
pub enum PlaybackEvent {
    SegmentStarted {
        segment_id: usize,
        duration: Duration,
    },
    SegmentFinished {
        segment_id: usize,
        duration: Duration,
    },
    Paused,
    Resumed,
    Starved,
    Stopped,
    Error(String),
}

#[derive(Clone, Debug)]
pub enum AppEvent {
    SegmentQueued {
        segment_id: usize,
        buffered_audio: Duration,
    },
    SegmentSynthesized {
        segment_id: usize,
        duration: Duration,
    },
    Playback(PlaybackEvent),
    Error(String),
    Completed,
}
