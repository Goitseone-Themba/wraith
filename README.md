# Wraith

> A lightweight, offline-first voice-controlled AI assistant for Linux.

Wraith is a local AI daemon designed to live in the background, ready to assist with coding, note-taking, automation, and general tasks—all through voice commands. No internet required. No data leaves your machine.

## Vision

An always-available AI companion that runs entirely offline on modest hardware. Speak your thoughts, debug code, automate scripts, and capture ideas without touching a keyboard. Built for privacy, efficiency, and the hacker who wants their environment to *listen*.

## Current Features

- **Voice Input → AI Response → Voice Output**: Full voice-in, voice-out conversation loop
- **Voice Activity Detection**: Hands-free operation with silence detection to trigger queries
- **Interruption Support**: Kill AI speech mid-playback with a loud noise or voice spike
- **Text Input Fallback**: Type messages when speaking isn't convenient
- **Local LLM**: Powered by LM Studio with models up to 4B parameters
- **opencode Engine**: Chat with the opencode agent (tools, sessions, permissions) from the same UI

## Architecture

```
┌─────────────┐     ┌─────────────┐     ┌─────────────┐
│   Browser   │────▶│  Rust/Axum  │────▶│  LM Studio  │
│   (WebUI)   │◀────│   Server    │◀────│  (LLM API)  │
└─────────────┘     └──────┬──────┘     └─────────────┘
                           │
                           ├──────────────────────────────┐
                           │                              │
                           ▼                              ▼
                    ┌─────────────┐                ┌─────────────┐
                    │  opencode   │                │  voxtype /  │
                    │ serve (mgr) │                │  piper /    │
                    │  HTTP+SSE   │                │  ffmpeg     │
                    └──────┬──────┘                └─────────────┘
                           │
                           ▼
                 agents, tools, permissions,
                 sessions, PTY workers
```

## Prerequisites

### System Dependencies

```bash
# Arch Linux / Manjaro
sudo pacman -S ffmpeg

# Ubuntu / Debian
sudo apt install ffmpeg
```

### External Tools

| Tool | Purpose | Install |
|------|---------|---------|
| **voxtype** | Speech-to-text | [voxtype](https://github.com/taylor-vann/voxtype) |
| **piper-tts** | Text-to-speech | [piper](https://github.com/rhasspy/piper) |
| **opencode** (optional) | Agent engine with tools | [opencode.ai](https://opencode.ai/docs/) — Wraith detects it and can install it from the UI |

> opencode is optional. Without it, Wraith keeps using the LM Studio engine.
> When it is missing, switching to the opencode engine shows an install card
> with platform-specific commands and a one-click installer (local requests only).

### Models

- **LLM**: Any GGUF model via [LM Studio](https://lmstudio.ai/) (1B-4B recommended for your hardware)
- **TTS Voice**: `en_US-libritts_r-high.onnx` (included in piper) or custom .onnx voices

## Quick Start

1. Install prerequisites above (opencode is optional; 2. below)
2. Start LM Studio and load your preferred model (defaults to `liquid/lfm2.5-1.2b`)
3. Run the server:

```bash
cargo run --release
```

TLS certificates are generated automatically on first run (stored under
`~/.config/wraith/tls/`). To use your own, place `cert.pem` and `key.pem` in the
project directory or point `[security] cert_path`/`key_path` at them.

4. Open [https://localhost:2026](https://localhost:2026) in your browser (accept the self-signed certificate warning)

5. For mobile access, use `https://YOUR_IP:2026`

6. Optional: switch the engine to **OPENCODE** in the header; if opencode is not
   installed, the UI shows install instructions and a one-click installer.

## Directory Structure

```
wraith/
├── Cargo.toml              # Rust package config
├── README.md               # This file
├── .wraith.toml.example    # Config template
├── tests/playwright.test.js
└── src/
    ├── main.rs             # Server, routes, config
    ├── index.html          # Browser WebUI
    ├── opencode.rs         # Managed opencode serve (HTTP + SSE)
    ├── voice.rs            # Streaming voice call
    ├── speech.rs           # TTS (piper)
    ├── stt.rs              # STT (voxtype)
    ├── agent.rs            # Voice agent install + workers
    ├── tools.rs            # Tool / permission helpers
    ├── tls.rs              # Auto self-signed TLS
    └── tests.rs            # Unit tests
```

## Usage Modes

### Engines

The **LOCAL / OPENCODE** switch in the header picks the chat backend:

- **LOCAL** — the LM Studio model. Fully offline, no tools.
- **OPENCODE** — a managed `opencode serve` process, bound to `127.0.0.1` with a
  random password. Replies stream into the chat, tool calls render as cards,
  permission requests open a modal (Allow once / Always / Reject), and the
  opencode bar provides session list/resume, abort, and delete.

Text chat uses opencode's streaming event API. Voice calls use the blocking API
(speak → transcribe → opencode turn → synthesize); streaming voice narration for
opencode is on the roadmap.

### Streaming voice call (opencode)

With the opencode engine selected, **Voice Call** opens a WebSocket session:

- Microphone audio is captured in an AudioWorklet, downsampled to 16 kHz PCM and
  streamed to the server; voice activity detection endpoints the turn.
- opencode's reply is chunked into sentences and spoken as it streams, through a
  persistent piper process (`--output_raw`), so speech starts before the answer
  is complete.
- Tool activity is narrated ("Running a command.", "Reading a file."), and
  retries are announced instead of silence.
- **Barge-in**: talking over the assistant flushes playback immediately. Set
  `[voice] interrupt_aborts = true` to also abort the running turn.
- Voice commands: "speak faster", "speak slower", "normal speed", "stop".
- Permissions and questions are spoken; answer with "yes", "no", "always", or an
  option label.
- `[voice] debug = true` logs per-turn latency and exposes `GET /voice/metrics`
  (time to first token/audio).

Headphones are recommended for true full-duplex on a laptop (browser echo
cancellation can only do so much through speakers).

#### Opt-in voice agent

The **Voice Agent** button installs two managed files into the project:

- `.opencode/agents/wraith-voice.md` — a voice-first agent (short spoken
  replies, `question: allow` so it can ask clarifying questions by voice).
- `.opencode/tools/wraith.ts` — `wraith_say` (speak a milestone mid-task) and
  `wraith_worker` (spawn a background worker).

Install/remove is loopback-only, refuses to overwrite unmanaged files, and
detects its own `managed-by: wraith` marker.

#### Background workers

The **Workers** button opens a panel that spawns parallel opencode sessions
("Worker: …") on the managed server, with live busy/idle status, abort and
delete. Workers share the project directory; use separate git worktrees for
file-editing work. tmux/PTY workers are a future backend for jobs that must
outlive Wraith.

### Voice Call (Hands-Free)

Click **START VOICE CALL** and speak naturally. Wraith will:

1. Listen for your voice
2. Detect 3 seconds of silence as end-of-query
3. Transcribe, query the AI, and speak the response
4. Wait for your next query

**Interrupting**: Say "stop" or make a loud noise to interrupt AI speech and start a new query.

### Text Input

Type in the text area and click **SEND MESSAGE** for a standard request/response cycle with audio playback.

### STT → Edit → Send

Record audio with **RECORD STT**, edit the transcription in the text field, then send manually. Useful for correcting errors before submission.

## Roadmap

### Phase 1: Core Voice Loop ✓

- [x] Browser-based WebUI
- [x] Voice input via Web Audio API
- [x] VAD with silence detection
- [x] STT → LLM → TTS pipeline
- [x] Audio interruption support

### Phase 2: System Refinement

- [x] ~~Configuration file~~ (`.wraith.toml` or env vars) — **done**
- [x] ~~Better error handling and user feedback~~ — **partially done** (`handleMicError()`)
- [ ] Graceful degradation when external tools fail — **pending**
- [ ] Configurable VAD thresholds (silence duration, volume sensitivity) — **pending**
- [ ] Model/tool path validation on startup — **pending**

#### Phase 2 Progress (2026-04-15)

| Task | Status | PR |
|------|--------|-----|
| MediaRecorder lifecycle fix | ✅ Done | #18 |
| Android permission handling | ✅ Done | #16 |
| Text input auto-play | ✅ Done | #14 |
| Cleanup on end call | ✅ Done | #2 |

### Phase 3: Persistence & Memory

- [ ] Multi-turn conversation history within session
- [ ] Session memory across restarts (persist chat history to disk)
- [ ] Context window management for long conversations
- [ ] Clear history command

### Phase 4: Shell Integration

> Planned implementation:
> 1. Add `/shell` endpoint accepting `{ "command": "string" }`
> 2. Execute via `std::process::Command`, return stdout/stderr
> 3. Integrate into chat system prompt with patterns:
>    - "run `cargo build`"
>    - "show git status"
>    - "list files in src"
> 4. Safety: commands return output, user confirms destructive actions
> 5. High-level aliases: "commit my changes" → `git add -A && git commit -m "..."`

- [ ] `/shell` endpoint with command execution
- [ ] Safety confirmations for destructive commands
- [ ] Natural language to shell command mapping
- [ ] Git operation aliases

### Phase 5: Coding Context

- [ ] Project-aware context loading (read directory structure, key files)
- [ ] Multi-file editing suggestions via voice
- [ ] Git operation commands ("commit my changes", "show diff")

### Phase 6: Tighter Integration

- [ ] System clipboard read/write
- [ ] File read/write operations via voice
- [ ] IDE/editor plugin for inline suggestions

## Hardware Target

Tested on:
- **CPU**: Intel i5-12500H (6 cores)
- **GPU**: Intel Iris Xe (integrated, not utilized)
- **RAM**: 16GB DDR4
- **OS**: Linux

Models should target **1B-4B parameters** for responsive real-time interaction on this hardware.

## Configuration

Wraith uses a configuration file (`.wraith.toml`) with environment variable overrides.

### Config File Location (checked in order)

1. `$WRAITH_CONFIG` environment variable
2. `~/.config/wraith/wraith.toml` (recommended)
3. `.wraith.toml` in project directory

### Configuration Options

```toml
[server]
host = "0.0.0.0"
port = 2026

[llm]
model = "liquid/lfm2.5-1.2b"
endpoint = "http://localhost:1234/v1/chat/completions"

[tts]
model = "/home/goitseone/piper-voices/en_US-libritts_r-high.onnx"

[stt]
executable = "voxtype"

# NOTE: `[vad]` settings are planned/documented configuration, but are not yet
# wired into the current browser/frontend VAD implementation. The WebUI still
# uses built-in thresholds for silence/speaking/interrupt detection.
[vad]
silence_threshold_ms = 3000
volume_threshold_speaking = 5.0
volume_threshold_interrupt = 8.0
min_recording_duration_ms = 500

[security]
cert_path = "cert.pem"
key_path = "key.pem"
auto_tls = true          # generate a self-signed cert when files are missing

# opencode engine (optional). When `directory` is empty, Wraith uses its own
# working directory. `port = 0` picks a random local port for the managed server.
[opencode]
executable = "opencode"
directory = ""
agent = ""              # optional agent name, e.g. "build"
model = ""              # optional provider/model, e.g. "opencode/gpt-5.1-codex"
port = 0
auto_start = false
extra_args = []
```

### Environment Variables

All settings can be overridden via environment variables, except the VAD values
which are currently documented for planned support and are not yet wired into
the browser/frontend VAD implementation:

| Variable | Description |
|----------|-------------|
| `WRAITH_HOST` | Server bind address |
| `WRAITH_PORT` | Server port |
| `WRAITH_LLM_MODEL` | LLM model name |
| `WRAITH_LLM_ENDPOINT` | LLM API endpoint |
| `WRAITH_TTS_MODEL` | TTS model path |
| `WRAITH_STT_EXECUTABLE` | STT executable name |
| `WRAITH_VAD_SILENCE_MS` | Planned: silence threshold (ms); not yet hooked up to frontend VAD |
| `WRAITH_VAD_VOLUME_SPEAKING` | Planned: speaking volume threshold; not yet hooked up to frontend VAD |
| `WRAITH_VAD_VOLUME_INTERRUPT` | Planned: interrupt volume threshold; not yet hooked up to frontend VAD |
| `WRAITH_VAD_MIN_RECORDING_MS` | Planned: minimum recording duration (ms); not yet hooked up to frontend VAD |
| `WRAITH_CERT_PATH` | TLS certificate path |
| `WRAITH_KEY_PATH` | TLS key path |
| `WRAITH_AUTO_TLS` | Generate self-signed certificates when missing |
| `WRAITH_OPENCODE_EXECUTABLE` | opencode executable name/path |
| `WRAITH_OPENCODE_DIRECTORY` | Working directory for the managed opencode server |
| `WRAITH_OPENCODE_AGENT` | Default opencode agent for Wraith sessions |
| `WRAITH_OPENCODE_MODEL` | Default `provider/model` for Wraith sessions |
| `WRAITH_OPENCODE_PORT` | Fixed port for the managed server (0 = random) |
| `WRAITH_OPENCODE_AUTO_START` | Start the managed server at boot (`true`/`1`) |
| `WRAITH_OPENCODE_EXTRA_ARGS` | Extra args passed to `opencode serve` |
| `WRAITH_VOICE_SILENCE_MS` | Voice-call end-of-speech timeout |
| `WRAITH_VOICE_MIN_SPEECH_MS` | Minimum speech before ending a turn |
| `WRAITH_VOICE_SPEED` | TTS rate (0.5–2.0) |
| `WRAITH_VOICE_NARRATION` | `off`, `tools` or `thoughts` |
| `WRAITH_VOICE_INTERRUPT_ABORTS` | Barge-in aborts the running turn |
| `WRAITH_VOICE_PREWARM` | Pre-warm piper at call start |
| `WRAITH_VOICE_DEBUG` | Log per-turn latency and expose `/voice/metrics` |
| `WRAITH_CONFIG` | Custom config file path |

## Contributing

Contributions welcome. This is a personal project that's grown into something useful—bug reports, feature ideas, and PRs are appreciated.

## License

MIT

---

*Wraith: Your voice-controlled AI daemon. Offline. Local. Always listening.*
