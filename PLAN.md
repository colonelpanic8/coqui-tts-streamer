# `coqui-tts-streamer` Plan

## Goal
Build a local article reader for Coqui TTS that can:

- accept long text from stdin or a file,
- segment it automatically,
- synthesize ahead of playback,
- play continuously with bounded buffering,
- optionally render a TUI that follows playback and shows synthesis progress.

The TUI must be optional and uncoupled from the playback engine. The core reader should still work headless.

## Modularity requirements
This should be designed so that the major subsystems can be replaced independently. That is a hard requirement, not a cleanup pass for later.

Rules:

- the core domain layer must not depend on Coqui, `rodio`, `ratatui`, or temp-file details
- the TUI must be a read-only consumer of app state and events
- playback must depend on an abstract audio output interface, not directly on the segmentation or TUI code
- the Coqui client must implement a generic TTS engine interface so another backend can be swapped in later
- segmentation must be pure and deterministic, with no network, playback, or UI coupling
- orchestration should depend on traits and message types, not concrete backends
- shared state should flow through explicit events and snapshots, not ad hoc cross-module mutation

If a module cannot be tested or reasoned about in isolation, the design is too coupled.

## What already exists
There are existing pieces we can reuse:

- the local Coqui HTTP server,
- simple chunked wrappers such as `coqui-read`,
- terminal UI crates such as `ratatui`,
- audio playback crates such as `rodio`.

There is not an existing tool in this setup that does all of the following well:

- incremental synthesis with backpressure,
- continuous playback from a bounded queue,
- playback-aware text highlighting,
- generation-progress visualization.

That justifies building a dedicated application instead of extending the shell wrapper indefinitely.

## Scope

### In scope for v1
- stdin/file input
- automatic segmentation
- continuous playback with queueing
- ahead-of-playback synthesis
- optional TUI
- pause/resume
- basic progress reporting

### Explicitly out of scope for v1
- voice cloning
- exact word-level timestamps
- article extraction from the web
- resumable persistent library/archive features

The important constraint here is that Coqui does not give us true word timings. That means the UI can be exact at the segment level in v1, but only approximate within a segment.

## Product shape
This should start as a small Rust workspace, not a monolith. That forces real boundaries early.

- `crates/core`
  - domain types
  - traits / ports
  - segmentation
  - queue policy
  - state machine
  - events
- `crates/tts-coqui`
  - Coqui HTTP adapter
- `crates/audio-output`
  - audio output traits plus shared playback abstractions
- `crates/audio-rodio`
  - `rodio` implementation of the audio output port
- `crates/ui-tui`
  - optional `ratatui` frontend
- `crates/app`
  - CLI
  - config loading
  - runtime wiring

If this feels slightly heavier up front, that is acceptable. It is cheaper than peeling apart a coupled single-crate prototype later.

## Architecture

### 1. Input
Input should come from:

- `stdin` by default
- `--file <path>` for file input

The app reads the entire article text once at startup, normalizes line endings and whitespace, and records stable text offsets for later highlighting.

### 2. Segmentation
The segmenter should operate in two passes:

1. split into paragraphs
2. split paragraphs into sentence-like units and then pack them into synthesis segments

Segment construction rules:

- prefer paragraph boundaries
- target a configurable size range instead of a single hard limit
- avoid tiny trailing fragments
- keep stable segment IDs and byte ranges into the original text

Each segment should carry:

- `segment_id`
- `text`
- `start_offset`
- `end_offset`
- `char_len`
- `estimated_duration_hint`

`estimated_duration_hint` starts as a heuristic and is replaced with actual audio duration once synthesis finishes.

### 3. Synthesis pipeline
The pipeline should be driven by queue watermarks, not just by raw segment count.

Core idea:

- maintain a playback queue measured in estimated seconds and segment count
- keep synthesizing while buffered audio is below a high watermark
- pause synthesis when buffered audio is above the high watermark
- begin playback only after a low watermark is satisfied

Why this matters:

- segment count is a poor proxy for playback continuity
- short and long segments can differ a lot in audio duration

For v1, use a single synthesis worker. That is the right default for a single local Coqui server process. Parallel synthesis can be added later if the backend changes.

### 4. Playback
Playback should use an in-process queue instead of shelling out to `ffplay` per segment.

Recommended v1 choice:

- `rodio` sink fed with synthesized WAV data in sequence

Why:

- avoids process spawn overhead per chunk
- reduces gaps between chunks
- keeps playback state in-process, which is necessary for the TUI

The playback layer should expose events:

- `SegmentQueued`
- `SegmentPlaybackStarted`
- `SegmentPlaybackFinished`
- `PlaybackPaused`
- `PlaybackResumed`
- `PlaybackStarved`

### 5. State and events
All user-visible state should come from a single shared state model updated by events.

Important fields:

- `total_segments`
- `next_segment_to_generate`
- `next_segment_to_play`
- `current_segment`
- `buffered_segments`
- `buffered_audio_secs`
- `generation_failures`
- `playback_state`

The TUI should subscribe to this state indirectly through an event channel. That keeps it uncoupled from playback and makes it easy to provide a headless JSON/events mode too.

## Port boundaries
The important interfaces should be explicit from the beginning.

Core ports:

- `TextSource`
  - yields source text and metadata
- `Segmenter`
  - turns normalized text into `Segment`s
- `TtsEngine`
  - synthesizes a `Segment` into audio bytes plus metadata
- `AudioOutput`
  - accepts decoded audio or audio chunks for playback
- `EventSink`
  - consumes `AppEvent`s
- `StateObserver`
  - receives state snapshots or diffs

The `app` crate wires these together. The `core` crate owns the traits and the domain types. Adapters implement the traits outside the core crate.

## Dependency direction
Dependencies should only point inward:

- `core` depends on nothing UI/backend-specific
- `tts-coqui` depends on `core`
- `audio-output` depends on `core`
- `audio-rodio` depends on `audio-output` and `core`
- `ui-tui` depends on `core`
- `app` depends on all adapters and performs composition

Nothing should depend on `app`, and `core` should never import an adapter crate.

## Data model
The main shared types should be stable and backend-agnostic:

- `Document`
- `Segment`
- `AudioChunk`
- `PlaybackCursor`
- `PipelineMetrics`
- `AppEvent`
- `AppState`
- `ReaderCommand`

These types should not contain UI widgets, HTTP request types, or concrete audio backend handles.

## Concurrency model
The concurrency design should also preserve modularity:

- one coordinator task owns pipeline state
- synthesis worker tasks communicate only through typed channels
- playback reports events back through typed channels
- the TUI never mutates pipeline internals directly

That gives us one place where ordering and buffering behavior is decided, instead of scattering it across adapters.

### 6. TUI
The TUI is optional. The reader must not depend on it.

Suggested stack:

- `ratatui`
- `crossterm`

Initial layout:

- top status bar
  - Coqui host
  - current state
  - buffered seconds
  - generated / played / total
- main text pane
  - article text
  - current segment highlighted
  - synthesized-but-not-yet-played region visually distinct
- footer
  - controls
  - transient errors/warnings

### Highlighting behavior
V1 should highlight at segment granularity:

- played text
- current segment
- generated ahead-of-playback
- not-yet-generated text

If we want finer-grained movement inside the current segment, it should be an estimate based on elapsed playback time versus segment audio duration. That should be treated as approximate UI polish, not truth.

## Error handling

### Coqui failures
- retry a segment a small number of times
- surface failures in the status area
- stop hard only if the queue can no longer recover

### Playback failures
- fail the current segment clearly
- preserve enough metadata to retry or skip

### Shutdown
- Ctrl-C should stop playback cleanly
- temp artifacts should be cleaned up
- the TUI should restore the terminal state correctly

## Rough code structure

- `crates/core/src/document.rs`
  - `Document`
  - source offsets
- `crates/core/src/segment.rs`
  - `Segment`
  - segment metadata
- `crates/core/src/segmenter.rs`
  - normalization
  - segmentation policy
- `crates/core/src/events.rs`
  - `AppEvent`
  - `ReaderCommand`
- `crates/core/src/state.rs`
  - `AppState`
  - state transitions
- `crates/core/src/pipeline.rs`
  - coordinator
  - watermarks
  - queue policy
- `crates/core/src/ports.rs`
  - `TtsEngine`
  - `AudioOutput`
  - observer traits
- `crates/tts-coqui/src/lib.rs`
  - Coqui adapter
- `crates/audio-output/src/lib.rs`
  - playback abstractions
- `crates/audio-rodio/src/lib.rs`
  - rodio adapter
- `crates/ui-tui/src/lib.rs`
  - terminal UI
- `crates/app/src/main.rs`
  - wiring and CLI

## Suggested crates
- `tokio` for concurrency
- `reqwest` for the Coqui client
- `serde` and `serde_json` for request/events
- `anyhow` or `eyre` for errors
- `rodio` for playback
- `ratatui` and `crossterm` for the TUI
- `unicode-segmentation` only if text handling needs it

## Testing strategy
Modularity should show up in testing too.

- unit test segmentation in `core` with no network or audio backend
- test state transitions in `core` from event sequences
- test pipeline behavior with fake `TtsEngine` and fake `AudioOutput`
- test TUI rendering from synthetic `AppState` snapshots
- keep adapter tests local to adapter crates

If integration tests require the whole stack, that is fine, but they should be additive. The core logic should already be covered without a live Coqui server.

## Milestones

### Milestone 1: headless MVP
- stdin/file input
- segmentation
- single synthesis worker
- in-process playback queue
- textual status output

### Milestone 2: optional TUI
- subscribe to app events
- show segment-level highlighting
- show generation lead/lag
- pause/resume and quit

### Milestone 3: refinement
- better duration heuristics
- estimated intra-segment cursor motion
- configurable watermarks
- optional skip/back controls

## Implementation order
1. scaffold the workspace and crate boundaries
2. define `Segment`, `AudioChunk`, `AppEvent`, `AppState`, and the core ports
3. implement segmentation in `core`
4. implement a fake `TtsEngine` and fake `AudioOutput` first
5. wire the coordinator against the trait interfaces
6. implement the Coqui adapter
7. implement the `rodio` adapter
8. add headless progress output
9. add the TUI as a subscriber

## Immediate next step
After signoff on this plan, implement Milestone 1 first. That is the minimum version that proves the architecture is correct. The TUI should come after the playback pipeline is stable.
