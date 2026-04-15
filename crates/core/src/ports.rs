use anyhow::Result;
use async_trait::async_trait;
use tokio::sync::broadcast;

use crate::{PlaybackEvent, PlaybackItem, Segment, audio::AudioChunk};

#[async_trait]
pub trait TtsEngine: Send + Sync {
    async fn synthesize(&self, segment: Segment) -> Result<AudioChunk>;
}

#[async_trait]
pub trait AudioOutput: Send + Sync {
    async fn enqueue(&self, item: PlaybackItem) -> Result<()>;
    async fn pause(&self) -> Result<()>;
    async fn resume(&self) -> Result<()>;
    async fn stop(&self) -> Result<()>;
    fn subscribe(&self) -> broadcast::Receiver<PlaybackEvent>;
}
