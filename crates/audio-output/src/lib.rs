use anyhow::Result;
use async_trait::async_trait;
use streamer_core::{AudioOutput, PlaybackEvent, PlaybackItem};
use tokio::sync::broadcast;

#[derive(Clone)]
pub struct NoopAudioOutput {
    event_tx: broadcast::Sender<PlaybackEvent>,
}

impl NoopAudioOutput {
    pub fn new() -> Self {
        let (event_tx, _) = broadcast::channel(64);
        Self { event_tx }
    }
}

impl Default for NoopAudioOutput {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl AudioOutput for NoopAudioOutput {
    async fn enqueue(&self, item: PlaybackItem) -> Result<()> {
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
