use std::{
    process::Command,
    sync::{Arc, Mutex, mpsc},
    thread,
    time::Duration,
};

use anyhow::{Context, Result, anyhow};
use async_trait::async_trait;
use streamer_core::{AudioOutput, PlaybackEvent, PlaybackItem};
use tempfile::NamedTempFile;
use tokio::sync::broadcast;

enum PlaybackCommand {
    Enqueue(PlaybackItem),
    Stop,
}

pub struct ProcessAudioOutput {
    command_tx: mpsc::Sender<PlaybackCommand>,
    event_tx: broadcast::Sender<PlaybackEvent>,
    current_pid: Arc<Mutex<Option<u32>>>,
}

impl ProcessAudioOutput {
    pub fn new() -> Result<Self> {
        let (command_tx, command_rx) = mpsc::channel();
        let (event_tx, _) = broadcast::channel(128);
        let current_pid = Arc::new(Mutex::new(None));
        let worker_events = event_tx.clone();
        let worker_pid = current_pid.clone();

        thread::spawn(move || {
            let mut ever_started = false;
            let mut starved = false;

            loop {
                match command_rx.recv_timeout(Duration::from_millis(250)) {
                    Ok(PlaybackCommand::Enqueue(item)) => {
                        ever_started = true;
                        starved = false;
                        let _ = worker_events.send(PlaybackEvent::SegmentStarted {
                            segment_id: item.segment.id,
                            duration: item.chunk.duration,
                        });

                        match play_item(&item, &worker_pid) {
                            Ok(()) => {
                                let _ = worker_events.send(PlaybackEvent::SegmentFinished {
                                    segment_id: item.segment.id,
                                    duration: item.chunk.duration,
                                });
                            }
                            Err(error) => {
                                let _ = worker_events.send(PlaybackEvent::Error(error.to_string()));
                            }
                        }
                    }
                    Ok(PlaybackCommand::Stop) => {
                        kill_current_process(&worker_pid);
                        let _ = worker_events.send(PlaybackEvent::Stopped);
                        break;
                    }
                    Err(mpsc::RecvTimeoutError::Timeout) => {
                        if ever_started && !starved {
                            let _ = worker_events.send(PlaybackEvent::Starved);
                            starved = true;
                        }
                    }
                    Err(mpsc::RecvTimeoutError::Disconnected) => {
                        kill_current_process(&worker_pid);
                        let _ = worker_events.send(PlaybackEvent::Stopped);
                        break;
                    }
                }
            }
        });

        Ok(Self {
            command_tx,
            event_tx,
            current_pid,
        })
    }
}

#[async_trait]
impl AudioOutput for ProcessAudioOutput {
    async fn enqueue(&self, item: PlaybackItem) -> Result<()> {
        self.command_tx
            .send(PlaybackCommand::Enqueue(item))
            .map_err(|_| anyhow!("playback thread is unavailable"))?;
        Ok(())
    }

    async fn pause(&self) -> Result<()> {
        send_signal(&self.current_pid, libc::SIGSTOP)?;
        let _ = self.event_tx.send(PlaybackEvent::Paused);
        Ok(())
    }

    async fn resume(&self) -> Result<()> {
        send_signal(&self.current_pid, libc::SIGCONT)?;
        let _ = self.event_tx.send(PlaybackEvent::Resumed);
        Ok(())
    }

    async fn stop(&self) -> Result<()> {
        self.command_tx
            .send(PlaybackCommand::Stop)
            .map_err(|_| anyhow!("playback thread is unavailable"))?;
        Ok(())
    }

    fn subscribe(&self) -> broadcast::Receiver<PlaybackEvent> {
        self.event_tx.subscribe()
    }
}

fn play_item(item: &PlaybackItem, current_pid: &Arc<Mutex<Option<u32>>>) -> Result<()> {
    let mut temp_file = NamedTempFile::new().context("failed to create temporary wav file")?;
    std::io::Write::write_all(&mut temp_file, item.chunk.bytes.as_ref())
        .context("failed to write temporary wav file")?;
    let temp_path = temp_file.into_temp_path();

    let mut child = Command::new("ffplay")
        .arg("-nodisp")
        .arg("-autoexit")
        .arg("-loglevel")
        .arg("warning")
        .arg(temp_path.as_os_str())
        .spawn()
        .context("failed to spawn ffplay")?;

    {
        let mut guard = current_pid.lock().unwrap();
        *guard = Some(child.id());
    }

    let status = child.wait().context("ffplay wait failed")?;
    {
        let mut guard = current_pid.lock().unwrap();
        *guard = None;
    }
    temp_path.close().ok();

    if !status.success() {
        return Err(anyhow!("ffplay exited with status {status}"));
    }

    Ok(())
}

fn send_signal(current_pid: &Arc<Mutex<Option<u32>>>, signal: i32) -> Result<()> {
    let pid = current_pid
        .lock()
        .unwrap()
        .ok_or_else(|| anyhow!("no active playback process"))?;
    let result = unsafe { libc::kill(pid as i32, signal) };
    if result == 0 {
        Ok(())
    } else {
        Err(anyhow!("failed to signal playback process"))
    }
}

fn kill_current_process(current_pid: &Arc<Mutex<Option<u32>>>) {
    let _ = send_signal(current_pid, libc::SIGTERM);
}
