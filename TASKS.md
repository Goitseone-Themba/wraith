# Wraith Phase 2 Tasks

Last updated: 2026-04-13

## Immediate (Phase 2)

| # | Task | Status | PR |
|---|------|--------|-----|
| 1 | Fix MediaRecorder lifecycle (handler stacking) | ✅ Done | #18 |
| 2 | Add Android permission handling | ✅ Done | #16 |
| 3 | Add auto-play to text input mode | ✅ Done | #14 |
| 4 | Add cleanup on endCall (stop stream, close context) | 🔄 In Progress | |
| 5 | Configuration file (`.wraith.toml` or env vars) | ⬜ Pending | |
| 6 | Graceful degradation when external tools fail | ⬜ Pending | |
| 7 | Configurable VAD thresholds | ⬜ Pending | |

## Short Term

| # | Task | Status |
|---|------|--------|
| 8 | Add minimum recording duration before silence detection | ⬜ Pending |
| 9 | Fix analyserNode/microphoneSource reuse | ⬜ Pending |
| 10 | Improve waveform visualization | ⬜ Pending |

## Long Term

| # | Task | Status |
|---|------|--------|
| 11 | Consider WebRTC VAD for better voice detection | ⬜ Pending |
| 12 | Add conversation history persistence | ⬜ Pending |
| 13 | Shell command integration | ⬜ Pending |

## Completed (Previous Phases)

- Browser-based WebUI
- Voice input via Web Audio API
- VAD with silence detection
- STT → LLM → TTS pipeline
- Audio interruption support
- HTTPS support for mobile microphone access
