pub mod audio;
pub mod document;
pub mod events;
pub mod pipeline;
pub mod ports;
pub mod segment;
pub mod segmenter;
pub mod state;

pub use audio::{AudioChunk, PlaybackItem};
pub use document::Document;
pub use events::{AppEvent, PlaybackEvent, ReaderCommand};
pub use pipeline::{PipelineConfig, PipelineHandle, spawn_pipeline};
pub use ports::{AudioOutput, TtsEngine};
pub use segment::Segment;
pub use segmenter::{SegmentMode, SegmenterConfig, normalize_text, segment_document};
pub use state::{AppState, PlaybackState, SegmentRuntime, SegmentStatus};
