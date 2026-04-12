# Voice Call Flow Analysis

## Overview

The voice call mode is Wraith's hands-free interaction loop. This document traces the complete flow, identifies state transitions, and performs root cause analysis on known and suspected issues.

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
│  │ 2. stopMediaRecorder()  ← Stops any running recorder                   │  │
│  │ 3. if (analyserNode) analyserNode.disconnect() ← PROBLEM POINT        │  │
│  │ 4. transcriptionPreview.textContent = 'Listening...'                    │  │
│  │ 5. if (fromInterruption) appendMessage('user', '[Interrupted]')          │  │
│  │ 6. attachMic()  ← PROBLEM POINT: creates new MediaRecorder              │  │
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
│  │ 4. Update waveform bars (visual feedback)                              │  │
│  │ 5. TIMING BRANCH:                                                      │  │
│  │    a) if (isRecordingCall && !isProcessingCallQuery)                   │  │
│  │       → if RMS > SPEAKING_THRESHOLD: update lastSpeakTimestamp         │  │
│  │       → else if silent > SILENCE_THRESHOLD: stopRecordingAndProcess()  │  │
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
│  │ 5. mediaRecorder.onstop = async () => {  ← CRITICAL: async handler   │  │
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
│  │ endVoiceCall():                                                        │  │
│  │ 1. isVoiceCallActive = false                                          │  │
│  │ 2. isRecordingCall = false                                            │  │
│  │ 3. isProcessingCallQuery = false                                      │  │
│  │ 4. btnVoiceCall.classList.remove('active')                            │  │
│  │ 5. voiceCallUI.classList.remove('active')                            │  │
│  │ 6. stopMediaRecorder()                                                │  │
│  │ 7. cancelAnimationFrame(vadRafId)                                     │  │
│  │ 8. setStatus('', false)                                               │  │
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

## Root Cause Analysis

### Issue 1: MediaRecorder Handler Stacking

**Severity:** High  
**Location:** `startVoiceCallLoop()` → `attachMic()` → `mediaRecorder`

**Problem:**
```javascript
// attachMic() is called on EVERY loop iteration
mediaRecorder = new MediaRecorder(globalMediaStream, { mimeType: 'audio/webm' });
mediaRecorder.ondataavailable = e => { /* ... */ };
```

The `mediaRecorder` object is recreated each time `startVoiceCallLoop()` runs. However, the OLD recorder's `onstop` handler may still be pending. This creates a scenario where:

1. `startVoiceCallLoop()` creates Recorder A, starts it
2. Silence detected → `stopRecordingAndProcess()` sets `onstop` on A
3. VAD loop calls `startVoiceCallLoop()` again (from `audio.onended`)
4. `attachMic()` creates NEW Recorder B, replaces variable
5. OLD Recorder A's `onstop` fires → uses stale `audioChunks` array
6. NEW Recorder B is also running → multiple recorders capturing

**Evidence:** Audio chunks from previous sessions appearing in transcription, or "Processing..." showing briefly before "Listening..." despite no speech.

**Fix:** Either:
- Store recorder reference and call `.stop()` before creating new one
- Use a single recorder for entire call session
- Clear `audioChunks` atomically before starting new recording

---

### Issue 2: analyserNode Disconnect Before Attach

**Severity:** Medium  
**Location:** `startVoiceCallLoop()` line 787

**Problem:**
```javascript
async function startVoiceCallLoop(fromInterruption) {
    isProcessingCallQuery = false;
    stopMediaRecorder();
    if (analyserNode) analyserNode.disconnect();  // ← Disconnected
    // ...
    const started = await attachMic();  // ← Will reconnect
```

The code disconnects then immediately reconnects the analyser. This creates a brief window where VAD could fire from stale data or miss the start of speech. More critically, `attachMic()` does this:

```javascript
if (microphoneSource) microphoneSource.disconnect();
microphoneSource = audioContext.createMediaStreamSource(globalMediaStream);
```

It creates a NEW `microphoneSource` each time, even though `globalMediaStream` is the same. This is wasteful and could cause issues if called rapidly (e.g., during audio interruption).

**Fix:** Create the source once. Only connect/disconnect the existing source.

---

### Issue 3: Race Condition in `stopRecordingAndProcess()`

**Severity:** Medium  
**Location:** `stopRecordingAndProcess()` → `mediaRecorder.onstop`

**Problem:**
```javascript
mediaRecorder.onstop = async () => {
    // This runs asynchronously after stopMediaRecorder() returns
    const blob = new Blob(audioChunks, { type: 'audio/webm' });
    const reader = new FileReader();
    reader.readAsDataURL(blob);
    reader.onloadend = async () => {
        const b64 = reader.result.split(',')[1];
        const text = await transcribe(b64);
        // ...
    };
};
stopMediaRecorder();  // onstop fires LATER
```

The function sets an async handler and returns. If `startVoiceCallLoop()` is called again before `onstop` fires (e.g., error path calling `startVoiceCallLoop(false)`), the old recorder is still running.

**Fix:** Await the stop explicitly, or track recorder state more carefully.

---

### Issue 4: Android Microphone Permissions Not Handled

**Severity:** High  
**Location:** `attachMic()` → `navigator.mediaDevices.getUserMedia()`

**Status:** ✅ FIXED

**Problem:**
```javascript
globalMediaStream = await navigator.mediaDevices.getUserMedia({
    audio: { echoCancellation: true, noiseSuppression: true, autoGainControl: true }
});
```

This didn't work reliably on Android because:
1. No explicit permission handling - browser may reject silently
2. No MIME type fallback for `MediaRecorder` (Android supports different codecs)
3. No graceful error message to user
4. `getUserMedia()` constraints aren't mobile-optimized

**Android Browser Requirements:**
- Chrome on Android needs specific constraints
- Safari on iOS has completely different requirements
- Some Android devices need `audio/Mp4` or `audio/ogg` instead of `audio/webm`

**Fix Applied:**
- Added `getSupportedMimeType()` to try multiple codecs in order:
  - `audio/webm;codecs=opus` (Chrome, Firefox)
  - `audio/webm`, `audio/ogg`, `audio/mp4`, `audio/wav` (fallbacks)
- Added `handleMicError()` with user-friendly messages for:
  - `NotAllowedError` / `PermissionDeniedError`
  - `NotFoundError` / `DevicesNotFoundError`
  - `NotReadableError` / `TrackStartError`
  - `OverconstrainedError`

---

### Issue 5: No Debouncing on Silence Detection

**Severity:** Low-Medium  
**Location:** `tickVAD()`

**Problem:**
```javascript
if (silent > SILENCE_MS_THRESHOLD) {
    stopRecordingAndProcess();
}
```

Once `SILENCE_MS_THRESHOLD` (3 seconds) is reached, `stopRecordingAndProcess()` fires immediately. There's no debounce—if the user pauses for breath mid-sentence longer than 3 seconds, their query gets sent prematurely.

**Fix:** Add minimum recording duration check, or require a minimum amount of "speech time" before accepting silence as end-of-query.

---

### Issue 6: `sendMessage()` Audio Not Auto-Playing

**Severity:** Low (User Experience)  
**Location:** `sendMessage()` → `appendMessage()`

**Problem:**
In text input mode, when the AI responds:
```javascript
setStatus('Speaking...', true);
const audioB64 = await synthesize(response);
appendMessage('ai', response, audioB64);
```

`appendMessage()` creates an `audio-mini` player that requires a click. The audio is never auto-played. Compare to voice call mode:
```javascript
const audio = new Audio(`data:audio/wav;base64,${audioB64}`);
currentAudio = audio;
audio.play();  // ← Auto-played
```

**Fix:** Auto-play the first AI audio response in text mode, similar to voice call mode.

---

### Issue 7: Waveform Bars Use Random Height

**Severity:** Low (Visual)  
**Location:** `tickVAD()`

**Problem:**
```javascript
bars.forEach((bar, i) => {
    const h = Math.max(4, rms * (0.5 + Math.random() * 0.5) * 20);
    bar.style.height = `${h}px`;
});
```

The `Math.random()` makes the waveform look "active" but it's fake—it doesn't represent actual frequency data. Also, all bars get the same random height, making it look like a pulsing blob rather than a real waveform.

**Fix:** Use actual frequency data from `getByteFrequencyData()` for more realistic visualization, or remove randomness if keeping it purely decorative.

---

### Issue 8: No Cleanup on `endVoiceCall()`

**Severity:** Low  
**Location:** `endVoiceCall()`

**Problem:**
```javascript
function endVoiceCall() {
    isVoiceCallActive = false;
    isRecordingCall = false;
    isProcessingCallQuery = false;
    btnVoiceCall.classList.remove('active');
    voiceCallUI.classList.remove('active');
    stopMediaRecorder();
    if (vadRafId) cancelAnimationFrame(vadRafId);
    setStatus('', false);
}
```

Missing cleanup:
- `globalMediaStream` is not stopped (mic stays "in use")
- `audioContext` is not closed
- `currentAudio` is not stopped if playing

**Fix:** Stop all streams, close audio context, pause any playing audio.

---

## Flow Summary Table

| Stage | Trigger | Duration | Success Path | Error Path |
|-------|---------|----------|--------------|------------|
| Init | Click "Voice Call" | ~100ms | Mic granted, VAD starts | Error shown, call ends |
| Listen | VAD starts | Until silence | RMS threshold met | Timeout (none) |
| Silence Detection | RMS < threshold | 3s | `stopRecordingAndProcess()` | False trigger (Issue 5) |
| Transcribe | Recording stops | ~1-3s | Text returned | Loop restarts |
| Chat | Text ready | Variable | Response returned | Error message, loop restarts |
| Synthesize | Response ready | ~1-5s | Audio generated | Loop restarts |
| Playback | Audio ready | Until done | Loop restarts | Loop restarts |
| Interrupt | Loud noise | Instant | Loop restarts | (None) |

---

## Recommendations

### Immediate (Phase 2)
1. Fix MediaRecorder lifecycle (Issue 1, 3)
2. Add Android permission handling (Issue 4)
3. Add auto-play to text input mode (Issue 6)
4. Add cleanup on endCall (Issue 8)

### Short Term
5. Add minimum recording duration before silence detection
6. Fix analyserNode/microphoneSource reuse
7. Improve waveform visualization

### Long Term
8. Consider WebRTC VAD for better voice detection
9. Add conversation history persistence
10. Shell command integration
