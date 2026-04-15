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

## Architecture

```
┌─────────────┐     ┌─────────────┐     ┌─────────────┐
│   Browser   │────▶│  Rust/Axum  │────▶│  LM Studio  │
│   (WebUI)   │◀────│   Server    │◀────│  (LLM API)  │
└─────────────┘     └──────┬──────┘     └─────────────┘
                           │
              ┌────────────┼────────────┐
              ▼            ▼            ▼
         ┌─────────┐ ┌─────────┐ ┌─────────┐
         │ voxtype │ │  ffmpeg │ │ piper   │
         │  (STT)  │ │(format) │ │  (TTS)  │
         └─────────┘ └─────────┘ └─────────┘
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

### Models

- **LLM**: Any GGUF model via [LM Studio](https://lmstudio.ai/) (1B-4B recommended for your hardware)
- **TTS Voice**: `en_US-libritts_r-high.onnx` (included in piper) or custom .onnx voices

## Quick Start

1. Install prerequisites above
2. Start LM Studio and load your preferred model (defaults to `liquid/lfm2.5-1.2b`)
3. Generate TLS certificates:

```bash
openssl req -x509 -newkey rsa:4096 -keyout key.pem -out cert.pem -days 365 -nodes
```

4. Run the server:

```bash
cargo run --release
```

5. Open [https://localhost:2026](https://localhost:2026) in your browser (accept the self-signed certificate warning)

6. For mobile access, use `https://YOUR_IP:2026`

## Directory Structure

```
wraith/
├── Cargo.toml          # Rust package config
├── README.md           # This file
└── src/
    ├── main.rs         # Server logic (Axum routes, STT/LLM/TTS integration)
    └── index.html      # Browser WebUI
```

## Usage Modes

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

[vad]
silence_threshold_ms = 3000
volume_threshold_speaking = 5.0
volume_threshold_interrupt = 8.0
min_recording_duration_ms = 500

[security]
cert_path = "cert.pem"
key_path = "key.pem"
```

### Environment Variables

All settings can be overridden via environment variables:

| Variable | Description |
|----------|-------------|
| `WRAITH_HOST` | Server bind address |
| `WRAITH_PORT` | Server port |
| `WRAITH_LLM_MODEL` | LLM model name |
| `WRAITH_LLM_ENDPOINT` | LLM API endpoint |
| `WRAITH_TTS_MODEL` | TTS model path |
| `WRAITH_STT_EXECUTABLE` | STT executable name |
| `WRAITH_VAD_SILENCE_MS` | Silence threshold (ms) |
| `WRAITH_VAD_VOLUME_SPEAKING` | Speaking volume threshold |
| `WRAITH_VAD_VOLUME_INTERRUPT` | Interrupt volume threshold |
| `WRAITH_VAD_MIN_RECORDING_MS` | Minimum recording duration (ms) |
| `WRAITH_CERT_PATH` | TLS certificate path |
| `WRAITH_KEY_PATH` | TLS key path |
| `WRAITH_CONFIG` | Custom config file path |

## Contributing

Contributions welcome. This is a personal project that's grown into something useful—bug reports, feature ideas, and PRs are appreciated.

## License

MIT

---

*Wraith: Your voice-controlled AI daemon. Offline. Local. Always listening.*
