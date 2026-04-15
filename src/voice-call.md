# Voice Call Flow Analysis

## Overview

The voice call mode is Wraith's hands-free interaction loop. This document traces the complete flow, identifies state transitions, and performs root cause analysis on known and suspected issues.

**Last updated:** 2026-04-15  
**Status:** 4 issues fixed, 3 pending

---

## Complete Flow Diagram

```
┌─────────────────────────────────────────────────────────────────────────────┐
│                              VOICE CALL INITIATION                          │
└─────────────────────────────────────────────────────────────────────────────┘
                                       │
                                       ▼
┌─────────────────────────────────────────────────────────────────────────────┐
│  User clicks "Voice Call" button                                            │
│  → btnVoiceCall click handler                                               │
│  → if (!isVoiceCallActive) startVoiceCall()                                │
└─────────────────────────────────────────────────────────────────────────────┘
                                       │
                                       ▼
┌─────────────────────────────────────────────────────────────────────────────┐
│  startVoiceCall()                                                          │
│  ┌───────────────────────────────────────────────────────────────────────┐  │
│  │ 1. isVoiceCallActive = true                                           │  │
│  │ 2. btnVoiceCall.classList.add('active')                                │  │
│  │ 3. voiceCallUI.classList.add('active')                                │  │
│  │ 4. setStatus('Voice call active', true)                                │  │
│  │ 5. startVoiceCallLoop(false)                                           │  │
│  └───────────────────────────────────────────────────────────────────────┘  │
└─────────────────────────────────────────────────────────────────────────────┘
                                       │
                                       ▼
┌─────────────────────────────────────────────────────────────────────────────┐
│  startVoiceCallLoop(fromInterruption: boolean)                              │
│  ┌───────────────────────────────────────────────────────────────────────┐  │
│  │ 1. isProcessingCallQuery = false                                       │  │
│  │ 2. await stopMediaRecorderAndWait() ← FIXED: now awaits properly     │  │
│  │ 3. if (analyserNode) analyserNode.disconnect() ← ISSUE #2            │  │
│  │ 4. transcriptionPreview.textContent = 'Listening...'                    │  │
│  │ 5. if (fromInterruption) appendMessage('user', '[Interrupted]')          │  │
│  │ 6. attachMic()                                                         │  │
│  │ 7. if (!started) endVoiceCall()                                        │  │
│  │ 8. isRecordingCall = true                                              │  │
│  │ 9. lastSpeakTimestamp = Date.now()                                     │  │
│  │ 10. mediaRecorder.start()                                              │  │
│  │ 11. vadRafId = requestAnimationFrame(tickVAD)                          │  │
│  └───────────────────────────────────────────────────────────────────────┘  │
└─────────────────────────────────────────────────────────────────────────────┘
                                       │
                                       ▼
┌─────────────────────────────────────────────────────────────────────────────┐
│  tickVAD()  ← Runs every animation frame (~60fps)                           │
│  ┌───────────────────────────────────────────────────────────────────────┐  │
│  │ 1. if (!isVoiceCallActive) return                                      │  │
│  │ 2. Get time-domain data from analyserNode                              │  │
│  │ 3. Compute RMS volume                                                  │  │
│  │ 4. Update waveform bars (visual feedback) ← ISSUE #7: uses Math.random│  │
│  │ 5. TIMING BRANCH:                                                      │  │
│  │    a) if (isRecordingCall && !isProcessingCallQuery)                   │  │
│  │       → if RMS > SPEAKING_THRESHOLD: update lastSpeakTimestamp        │  │
│  │       → else if silent > SILENCE_THRESHOLD: stopRecordingAndProcess()  │  │
│  │       → ISSUE #5: No minimum recording duration check                  │  │
│  │    b) if (!isRecordingCall && currentAudio && !paused)                │  │
│  │       → if RMS > INTERRUPT_THRESHOLD: pause audio, restart loop        │  │
│  │ 6. Schedule next frame: requestAnimationFrame(tickVAD)                  │  │
│  └───────────────────────────────────────────────────────────────────────┘  │
└─────────────────────────────────────────────────────────────────────────────┘
                                       │
                     ┌─────────────────┴─────────────────┐
                     │                                   │
                     ▼                                   ▼
┌───────────────────────────────┐     ┌───────────────────────────────────────┐
│   SILENCE DETECTED            │     │   INTERRUPT DETECTED                 │
│   stopRecordingAndProcess()   │     │   (during AI speech)                  │
└───────────────────────────────┘     └───────────────────────────────────────┘
                     │                                   │
                     ▼                                   │
┌─────────────────────────────────────────────────────────────────────────────┐
│  stopRecordingAndProcess()                                                  │
│  ┌───────────────────────────────────────────────────────────────────────┐  │
│  │ 1. isProcessingCallQuery = true                                       │  │
│  │ 2. isRecordingCall = false                                            │  │
│  │ 3. transcriptionPreview.textContent = 'Processing...'                  │  │
│  │ 4. setStatus('Processing...', true)                                    │  │
│  │ 5. mediaRecorder.onstop = async () => {                               │  │
│  │    a. Convert audio chunks to blob → base64                           │  │
│  │    b. POST to /transcribe                                              │  │
│  │    c. if (empty/failed): startVoiceCallLoop(false), return            │  │
│  │    d. appendMessage('user', text)                                     │  │
│  │    e. POST to /chat                                                    │  │
│  │    f. if (error): appendMessage('ai', error), restart loop            │  │
│  │    g. appendMessage('ai', response)                                   │  │
│  │    h. POST to /synthesize                                              │  │
│  │    i. if (success): create Audio, play, set onended handler          │  │
│  │       → onended: currentAudio = null, startVoiceCallLoop(false)       │  │
│  │    j. if (failed): startVoiceCallLoop(false)                          │  │
│  │ }                                                                       │  │
│  │ 6. stopMediaRecorder()                                                 │  │
│  └───────────────────────────────────────────────────────────────────────┘  │
└─────────────────────────────────────────────────────────────────────────────┘
                                       │
                                       ▼
┌─────────────────────────────────────────────────────────────────────────────┐
│  END CALL                                                                  │
│  User clicks "End Call" or error occurs                                    │
│  ┌───────────────────────────────────────────────────────────────────────┐  │
│  │ endVoiceCall() [FIXED - full cleanup]:                                 │  │
│  │ 1. isVoiceCallActive = false                                          │  │
│  │ 2. isRecordingCall = false                                            │  │
│  │ 3. isProcessingCallQuery = false                                      │  │
│  │ 4. btnVoiceCall.classList.remove('active')                            │  │
│  │ 5. voiceCallUI.classList.remove('active')                            │  │
│  │ 6. pause & clear currentAudio                                          │  │
│  │ 7. resetAllAudioPlayers()                                             │  │
│  │ 8. stopMediaRecorder()                                                │  │
│  │ 9. cancelAnimationFrame(vadRafId)                                     │  │
│  │ 10. disconnect analyserNode, set to null                              │  │
│  │ 11. stop all media stream tracks, set to null                         │  │
│  │ 12. close audioContext                                                 │  │
│  │ 13. disconnect microphoneSource, set to null                         │  │
│  │ 14. clear mediaRecorder handlers, set to null                         │  │
│  │ 15. clear audioChunks                                                  │  │
│  │ 16. setStatus('', false)                                               │  │
│  └───────────────────────────────────────────────────────────────────────┘  │
└─────────────────────────────────────────────────────────────────────────────┘
```

---

## State Variables

| Variable | Purpose | States |
|----------|---------|--------|
| `isVoiceCallActive` | Overall call status | `false` → `true` → `false` |
| `isRecordingCall` | Currently capturing audio | `false` → `true` → `false` |
| `isProcessingCallQuery` | Pipeline running (STT→LLM→TTS) | `false` → `true` → `false` |
| `currentAudio` | Reference to playing AI response | `null` → `HTMLAudioElement` → `null` |
| `lastSpeakTimestamp` | Tracks last detected speech | Updated on RMS > threshold |
| `globalMediaStream` | Persistent mic stream | Created once, reused |
| `mediaRecorder` | Audio capture instance | Recreated each loop cycle |
| `audioChunks` | Accumulated audio buffers | Cleared on each `attachMic()` call |

---

## Issue Tracker

### ✅ FIXED: Issue 1 - MediaRecorder Handler Stacking

**Severity:** High  
**Location:** `startVoiceCallLoop()` → `attachMic()` → `mediaRecorder`

**Problem:**
```javascript
// Old: mediaRecorder recreated each loop, old onstop could fire
mediaRecorder = new MediaRecorder(globalMediaStream, { mimeType: 'audio/webm' });
```

**Fix Applied:** `stopMediaRecorderAndWait()` (lines 715-737)
```javascript
function stopMediaRecorderAndWait() {
    return new Promise((resolve) => {
        if (!mediaRecorder || mediaRecorder.state === 'inactive') {
            resolve();
            return;
        }
        
        const previousOnStop = mediaRecorder.onstop;
        mediaRecorder.onstop = (e) => {
            if (previousOnStop) previousOnStop(e);
            resolve();
        };
        
        mediaRecorder.stop();
        
        // Safety timeout
        setTimeout(() => resolve(), 100);
    });
}
```

**Status:** ✅ RESOLVED in PR #18

---

### ⚠️ ISSUE 2: analyserNode Disconnect Before Attach

**Severity:** Medium  
**Location:** `startVoiceCallLoop()` line 929

**Problem:**
```javascript
async function startVoiceCallLoop(fromInterruption) {
    // ...
    if (analyserNode) analyserNode.disconnect();  // ← Disconnected
    // ...
    const started = await attachMic();  // ← Will reconnect
    // ...
    microphoneSource.connect(analyserNode);
}
```

The code disconnects then immediately reconnects. More critically, `attachMic()` creates a NEW `microphoneSource` each time:
```javascript
if (microphoneSource) microphoneSource.disconnect();
microphoneSource = audioContext.createMediaStreamSource(globalMediaStream);
```

**Impact:** Wasteful resource churn during rapid interruptions.

**Fix needed:** Create source once, only connect/disconnect.

**Status:** ⬜ PENDING

---

### ✅ PARTIALLY FIXED: Issue 3 - Race Condition in `stopRecordingAndProcess()`

**Severity:** Medium  
**Location:** `stopRecordingAndProcess()` → `mediaRecorder.onstop`

**Problem:**
```javascript
mediaRecorder.onstop = async () => { /* async processing */ };
stopMediaRecorder();  // onstop fires LATER
```

**Mitigation:** `stopMediaRecorderAndWait()` chains handlers properly.

**Remaining risk:** If `startVoiceCallLoop()` is called before `onstop` fires (error path), old recorder state could conflict.

**Status:** ⚠️ MITIGATED but not fully resolved

---

### ✅ FIXED: Issue 4 - Android Microphone Permissions Not Handled

**Severity:** High  
**Location:** `attachMic()` → `navigator.mediaDevices.getUserMedia()`

**Problem:** No MIME type fallback, no error handling for mobile browsers.

**Fix Applied:**
1. `getSupportedMimeType()` tries multiple codecs in order:
   - `audio/webm;codecs=opus` (Chrome, Firefox)
   - `audio/webm`, `audio/ogg`, `audio/mp4`, `audio/wav` (fallbacks)

2. `handleMicError()` provides user-friendly messages for:
   - `NotAllowedError` / `PermissionDeniedError`
   - `NotFoundError` / `DevicesNotFoundError`
   - `NotReadableError` / `TrackStartError`
   - `OverconstrainedError`

**Status:** ✅ RESOLVED in PR #16

---

### ⚠️ ISSUE 5: No Debouncing on Silence Detection

**Severity:** Medium  
**Location:** `tickVAD()`

**Problem:**
```javascript
if (silent > SILENCE_MS_THRESHOLD) {
    stopRecordingAndProcess();
}
```

No minimum recording duration check. Saying "he-" then pausing 3 seconds sends a partial query.

**Fix needed:** Track total "speech time" during recording. Only trigger on silence if user spoke for at least ~1 second total.

**Status:** ⬜ PENDING

---

### ✅ FIXED: Issue 6 - `sendMessage()` Audio Not Auto-Playing

**Severity:** Low (User Experience)  
**Location:** `sendMessage()`

**Problem:** In text input mode, audio was never auto-played.

**Fix Applied:** Added auto-play in `sendMessage()` (lines 787-795):
```javascript
if (audioB64) {
    const audio = new Audio(`data:audio/wav;base64,${audioB64}`);
    currentAudio = audio;
    audio.play();
    
    audio.onended = () => {
        currentAudio = null;
    };
}
```

**Status:** ✅ RESOLVED in PR #14

---

### ⚠️ ISSUE 7: Waveform Bars Use Random Height

**Severity:** Low (Visual)  
**Location:** `tickVAD()`

**Problem:**
```javascript
bars.forEach((bar, i) => {
    const h = Math.max(4, rms * (0.5 + Math.random() * 0.5) * 20);
    bar.style.height = `${h}px`;
});
```

`Math.random()` makes the waveform look active but fake. All bars get the same random height.

**Fix needed:** Use actual frequency data:
```javascript
const freqData = new Uint8Array(analyserNode.frequencyBinCount);
analyserNode.getByteFrequencyData(freqData);
bars.forEach((bar, i) => {
    // Sample from freqData instead of random
    const h = Math.max(4, (freqData[i * 4] / 255) * 40);
    bar.style.height = `${h}px`;
});
```

**Status:** ⬜ PENDING

---

### ✅ FIXED: Issue 8 - No Cleanup on `endVoiceCall()`

**Severity:** Low  
**Location:** `endVoiceCall()`

**Problem:** Missing cleanup on end call.

**Fix Applied:** Full cleanup in `endVoiceCall()` (lines 868-918):
- Pauses and clears `currentAudio`
- Calls `resetAllAudioPlayers()`
- Stops all media stream tracks
- Closes `audioContext`
- Disconnects `analyserNode` and `microphoneSource`
- Clears `mediaRecorder` handlers
- Clears `audioChunks`

**Status:** ✅ RESOLVED in PR #2

---

## Flow Summary Table

| Stage | Trigger | Duration | Success Path | Error Path |
|-------|---------|----------|--------------|------------|
| Init | Click "Voice Call" | ~100ms | Mic granted, VAD starts | Error shown, call ends |
| Listen | VAD starts | Until silence | RMS threshold met | Timeout (none) |
| Silence Detection | RMS < threshold | 3s (hardcoded) | `stopRecordingAndProcess()` | False trigger (Issue 5) |
| Transcribe | Recording stops | ~1-3s | Text returned | Loop restarts |
| Chat | Text ready | Variable | Response returned | Error message, loop restarts |
| Synthesize | Response ready | ~1-5s | Audio generated | Loop restarts |
| Playback | Audio ready | Until done | Loop restarts | Loop restarts |
| Interrupt | Loud noise | Instant | Loop restarts | (None) |
| End Call | User clicks / error | Instant | Full cleanup | (None) |

---

## Recommendations

### Immediate (Phase 2) - Updated for 2026-04-15

| Priority | Task | Status |
|----------|------|--------|
| ~~HIGH~~ | ~~Configuration file (`.wraith.toml` or env vars)~~ | ✅ Done |
| HIGH | Graceful degradation when external tools fail | ⬜ Pending |
| MEDIUM | Configurable VAD thresholds | ⬜ Pending |
| MEDIUM | Minimum recording duration check | ⬜ Pending |
| LOW | analyserNode/microphoneSource reuse | ⬜ Pending |
| LOW | Improve waveform visualization | ⬜ Pending |

**Note:** VAD thresholds are not yet wired through to the frontend voice activity detection logic. The current implementation in `src/index.html` still uses hardcoded values. The configuration below reflects the intended `.wraith.toml` shape once that wiring is implemented:
```toml
[vad]
silence_threshold_ms = 3000
volume_threshold_speaking = 5.0
volume_threshold_interrupt = 8.0
min_recording_duration_ms = 500
```

### Short Term (Phase 2)

| Task | Status |
|------|--------|
| WebRTC VAD for better voice detection | ⬜ Pending |

### Long Term (Phase 3+)

| Task | Status |
|------|--------|
| Multi-turn conversation history | ⬜ Pending |
| Session memory across restarts | ⬜ Pending |
| Shell command integration | ⬜ Pending |

---

## Appendix: Issue Status History

| Issue | Reported | Fixed | PR |
|-------|----------|-------|-----|
| #1 MediaRecorder Handler Stacking | 2026-04-13 | 2026-04-13 | #18 |
| #2 analyserNode Disconnect | 2026-04-13 | - | - |
| #3 Race Condition | 2026-04-13 | 2026-04-13 | #18 (mitigated) |
| #4 Android Permissions | 2026-04-13 | 2026-04-13 | #16 |
| #5 No Debouncing | 2026-04-13 | - | - |
| #6 Text Mode Auto-Play | 2026-04-13 | 2026-04-13 | #14 |
| #7 Waveform Random | 2026-04-13 | - | - |
| #8 No Cleanup | 2026-04-13 | 2026-04-13 | #2 |
