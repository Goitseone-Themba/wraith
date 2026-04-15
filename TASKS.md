# Wraith Phase 2 Tasks

Last updated: 2026-04-15

## Status Overview

| Category | Completed | Pending |
|----------|-----------|---------|
| Voice Call Bug Fixes | 4 | 3 |
| Configuration & Config | 1 | 0 |
| VAD Improvements | 1 | 2 |
| Resilience | 0 | 1 |

---

## Phase 2: System Refinement

### Completed ✓

| # | Task | Fixed In | Notes |
|---|------|---------|-------|
| 1 | Fix MediaRecorder lifecycle (handler stacking) | `stopMediaRecorderAndWait()` | PR #18 |
| 2 | Add Android permission handling | `getSupportedMimeType()` + `handleMicError()` | PR #16 |
| 3 | Add auto-play to text input mode | `sendMessage()` | PR #14 |
| 4 | Add cleanup on endCall (stop stream, close context) | `endVoiceCall()` | PR #2 |

### Pending

| # | Task | Priority | Notes |
|---|------|----------|-------|
| 5 | ~~**Configuration file**~~ (`.wraith.toml` or env vars) | ✅ Done | See `.wraith.toml.example` |
| 6 | **Graceful degradation** when external tools fail | HIGH | voxtype/piper/ffmpeg WILL fail |
| 7 | ~~**Configurable VAD thresholds**~~ | ✅ Done | Via `.wraith.toml` [vad] section |
| 8 | **Minimum recording duration** before silence detection | MEDIUM | Prevents accidental triggers |
| 9 | **Fix analyserNode/microphoneSource reuse** | LOW | Avoid disconnect/reconnect churn |
| 10 | **Improve waveform visualization** | LOW | Use real frequency data, not Math.random() |

---

## Priority Queue for Phase 2 Completion

### 1. Configuration File (HIGH)
**Why first:** Everything else becomes configurable once this exists.

```toml
# .wraith.toml (proposed location: ~/.config/wraith/wraith.toml)
[server]
port = 2026
host = "0.0.0.0"

[llm]
model = "liquid/lfm2.5-1.2b"
endpoint = "http://localhost:1234/v1/chat/completions"

[tts]
model = "/path/to/voice.onnx"

[stt]
executable = "voxtype"

[vad]
silence_threshold_ms = 3000
volume_threshold_speaking = 5.0
volume_threshold_interrupt = 8.0
min_recording_duration_ms = 500  # NEW
```

**Accept:** `--config <path>` or `WRAITH_CONFIG` env var

### 2. Graceful Degradation (HIGH)
**Why:** External tools (voxtype, piper, ffmpeg) WILL fail eventually.

Current behavior:
```
piper-tts fails → HTML error span returned → displayed to user
```

Better behavior:
```
1. Detect tool failure before user sees raw error
2. Log detailed error to console
3. Fallback: return text-only response with notice
4. Never crash the call loop
```

Implementation:
- [ ] Check tool existence on startup (not just when called)
- [ ] Wrap each external command in Result handling
- [ ] Add retry logic with exponential backoff (1 retry)
- [ ] Return meaningful error messages to frontend
- [ ] Continue voice loop on failure (don't hang)

### 3. Configurable VAD Thresholds (MEDIUM)
**Why:** 3 seconds is hardcoded. Users with slow speech need more time.

Changes needed:
- Read from config file or env vars
- Expose via frontend (settings panel?) or at least document
- Minimum: `SILENCE_THRESHOLD`, `VOLUME_THRESHOLD_SPEAKING`, `VOLUME_THRESHOLD_INTERRUPT`
- NEW: `MIN_RECORDING_DURATION` (prevent breath/hiccup triggers)

### 4. Minimum Recording Duration (MEDIUM)
**Why:** 3s silence after saying "he-" triggers a partial query.

Current flow:
```
Speak "hello" (0.5s) → pause 3s → sent as query
```

Better flow:
```
Speak "hello" (0.5s) → pause 3s → check: did user speak for at least 1s total?
→ No: ignore, continue listening
→ Yes: process query
```

Implementation:
- Track total "speech time" during recording session
- Only trigger on silence if `speechTime >= MIN_RECORDING_MS`
- Reset speechTime on each new speaking segment

### 5. analyserNode/microphoneSource Reuse (LOW)
**Why:** Creates new source every loop iteration. Wasteful and could cause issues during rapid interruptions.

Current:
```javascript
// Called every startVoiceCallLoop()
if (microphoneSource) microphoneSource.disconnect();
microphoneSource = audioContext.createMediaStreamSource(globalMediaStream);
```

Better:
```javascript
// Create once in attachMic()
if (!microphoneSource) {
    microphoneSource = audioContext.createMediaStreamSource(globalMediaStream);
}
// Just connect/disconnect as needed
```

---

## Deferred to Phase 3

| # | Task | Notes |
|---|------|-------|
| 11 | Multi-turn conversation history within session | Requires LLM context window management |
| 12 | Session memory across restarts | Persist chat history to disk |
| 13 | Clear history command | Simple UI addition |

## Deferred to Phase 4

| # | Task | Notes |
|---|------|-------|
| 14 | Shell command integration | `/shell` endpoint |
| 15 | Safety confirmations for destructive commands | rm, dd, etc. |
| 16 | Natural language to shell command mapping | "run cargo build" |

## Deferred to Phase 5+

- Project-aware context loading
- Multi-file editing suggestions
- Clipboard read/write
- File read/write via voice
- IDE plugin

---

## Testing Checklist

Before marking Phase 2 complete, verify:

- [ ] Voice call survives voxtype failure
- [ ] Voice call survives piper-tts failure
- [ ] Voice call survives ffmpeg failure
- [ ] Config file overrides hardcoded paths
- [ ] Config file missing → use sensible defaults (don't crash)
- [ ] VAD thresholds adjustable via config
- [ ] Minimum recording duration prevents accidental triggers
- [ ] Waveform shows real frequency data
- [ ] analyserNode reused (no disconnect/reconnect churn)

---

*Phase 2 Goal: Make Wraith resilient, configurable, and comfortable to use.*
