use std::{io::Cursor, time::Duration};

use anyhow::{Context, Result, anyhow};
use async_trait::async_trait;
use reqwest::Client;
use serde::Serialize;
use streamer_core::{AudioChunk, Segment, TtsEngine};

#[derive(Clone, Debug)]
pub struct CoquiConfig {
    pub base_url: String,
    pub speaker: Option<String>,
    pub language: Option<String>,
}

impl Default for CoquiConfig {
    fn default() -> Self {
        Self {
            base_url: "http://[::1]:11115".to_string(),
            speaker: None,
            language: None,
        }
    }
}

#[derive(Clone)]
pub struct CoquiTtsEngine {
    client: Client,
    config: CoquiConfig,
}

impl CoquiTtsEngine {
    pub fn new(config: CoquiConfig) -> Result<Self> {
        let client = Client::builder()
            .build()
            .context("failed to build HTTP client")?;
        Ok(Self { client, config })
    }

    async fn request_bytes(&self, text: &str) -> Result<Vec<u8>> {
        let post_response = self
            .client
            .post(format!(
                "{}/api/tts",
                self.config.base_url.trim_end_matches('/')
            ))
            .json(&CoquiPayload::new(
                text,
                self.config.speaker.as_deref(),
                self.config.language.as_deref(),
            ))
            .send()
            .await
            .context("POST /api/tts failed")?;

        if post_response.status().is_success() {
            return Ok(post_response.bytes().await?.to_vec());
        }
        let post_status = post_response.status();
        let post_body = post_response.text().await.unwrap_or_default();

        let get_response = self
            .client
            .get(format!(
                "{}/api/tts",
                self.config.base_url.trim_end_matches('/')
            ))
            .query(&CoquiPayload::new(
                text,
                self.config.speaker.as_deref(),
                self.config.language.as_deref(),
            ))
            .send()
            .await
            .context("GET /api/tts fallback failed")?;

        if !get_response.status().is_success() {
            let status = get_response.status();
            let body = get_response.text().await.unwrap_or_default();
            return Err(anyhow!(
                "Coqui POST failed with {post_status}: {post_body}\nCoqui GET fallback failed with {status}: {body}"
            ));
        }

        Ok(get_response.bytes().await?.to_vec())
    }
}

#[async_trait]
impl TtsEngine for CoquiTtsEngine {
    async fn synthesize(&self, segment: Segment) -> Result<AudioChunk> {
        let bytes = self.request_bytes(segment.text()).await?;
        let duration = wav_duration(&bytes)?;
        Ok(AudioChunk::new(segment.id, bytes, duration))
    }
}

#[derive(Serialize)]
struct CoquiPayload<'a> {
    text: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    speaker_id: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    speaker: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    language_id: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    language: Option<&'a str>,
}

impl<'a> CoquiPayload<'a> {
    fn new(text: &'a str, speaker: Option<&'a str>, language: Option<&'a str>) -> Self {
        Self {
            text,
            speaker_id: speaker,
            speaker,
            language_id: language,
            language,
        }
    }
}

fn wav_duration(bytes: &[u8]) -> Result<Duration> {
    let reader =
        hound::WavReader::new(Cursor::new(bytes)).context("failed to parse Coqui WAV output")?;
    let spec = reader.spec();
    let samples = reader.duration() as f64;
    let denominator = (spec.sample_rate as f64 * spec.channels as f64).max(1.0);
    Ok(Duration::from_secs_f64(samples / denominator))
}
