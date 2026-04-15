use std::{sync::Arc, time::Duration};

use crate::Segment;

#[derive(Clone, Debug)]
pub struct AudioChunk {
    pub segment_id: usize,
    pub bytes: Arc<[u8]>,
    pub duration: Duration,
}

impl AudioChunk {
    pub fn new(segment_id: usize, bytes: Vec<u8>, duration: Duration) -> Self {
        Self {
            segment_id,
            bytes: bytes.into(),
            duration,
        }
    }
}

#[derive(Clone, Debug)]
pub struct PlaybackItem {
    pub segment: Segment,
    pub chunk: AudioChunk,
}
