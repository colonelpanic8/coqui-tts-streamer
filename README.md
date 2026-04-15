# `coqui-tts-streamer`

Incremental article reader for a local Coqui TTS server.

## What it does
- reads text from stdin or `--file`
- segments long text automatically
- synthesizes ahead of playback
- plays continuously with bounded buffering
- optionally shows a TUI with playback/generation progress

## Usage

Headless from stdin:

```bash
cat article.txt | cargo run -- --json-events
```

Headless from a file:

```bash
cargo run -- --file article.txt
```

With the TUI:

```bash
cargo run -- --file article.txt --tui
```

Try one of the bundled fixtures:

```bash
cargo run -- --file fixtures/city-layers.txt --tui
cargo run -- --file fixtures/quote-heavy-thread.txt --tui
```

Custom Coqui host / speaker:

```bash
cargo run -- --file article.txt --host http://[::1]:11115 --speaker p376
```

Strict sentence-based segmentation:

```bash
cargo run -- --file article.txt --segment-mode sentence
```

More aggressive startup buffering / synthesis fan-out:

```bash
cargo run -- --file article.txt --prebuffer-secs 12 --max-buffered-secs 12 --synthesis-concurrency 3
```

## Controls
- `space`: pause/resume
- `j` / `k`: scroll the segment list
- `f`: jump back to follow mode
- `q`: quit

## Workspace layout
- `crates/core`: domain types, ports, segmenter, coordinator
- `crates/tts-coqui`: Coqui HTTP adapter
- `crates/audio-output`: small no-op playback adapter for testing
- `crates/audio-rodio`: process-backed playback adapter using `ffplay`
- `crates/ui-tui`: optional terminal UI
- `crates/app`: CLI and runtime wiring
- `fixtures/`: realistic article text for manual TUI runs and future tests

## Notes
- The default segmenter is paragraph-aware: it splits into sentences first, then packs nearby sentences together within each paragraph up to the configured limits.
- `--segment-mode sentence` keeps each detected sentence as its own segment, except when a single sentence must still be split to satisfy `--max-chars`.
- Playback now waits for a fuller default startup buffer before beginning, so the reader does more work up front and is less likely to stall mid-stream.
- By default there is no post-start synthesis cap; pass `--max-buffered-secs` only when you want to limit how far ahead generation runs.
- The pipeline can keep multiple synthesis requests in flight with `--synthesis-concurrency`, though the best value still depends on how the local Coqui server behaves.
- The current playback adapter uses `ffplay` instead of `rodio` so the project builds cleanly outside a Nix shell with ALSA development headers.
- The TUI is exact at the segment level. It does not attempt word-level timing.
