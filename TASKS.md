# Wraith Development Tasks

Last updated: 2026-09-15

## Status Overview

| Phase | Status | Tasks |
|-------|--------|-------|
| Phase 1 | ✅ Complete | Core voice loop |
| Phase 2 | ✅ Complete | System refinement |
| Phase 3 | 🔄 In Progress | Persistence & Memory |
| Phase 4 | ⬜ Pending | Shell Integration |
| Phase 5+ | ⬜ Pending | Coding Context |
| opencode | 🔄 In Progress | Agent engine integration |

---

## Phase 3: Persistence & Memory (In Progress)

### Task 1: Multi-turn Conversation History

**Priority:** HIGH  
**Description:** LLM context within current session (conversation history passed to LLM).

**Implementation:**
- Maintain message history array in frontend
- Include history in chat API requests to LLM
- Clear history on voice call start (fresh conversation)
- Limit history to last N messages (prevent context overflow)

---

### Task 2: Session Memory Storage

**Priority:** HIGH  
**Description:** Persist chat history to disk for continuity across server restarts.

**Implementation:**
- Save chat history to JSON file (`~/.config/wraith/history.json`)
- Load history on server startup
- Sync after each message exchange

---

### Task 3: Clear History Command

**Priority:** MEDIUM  
**Description:** Allow user to clear conversation history.

**Implementation:**
- Add "clear" button to UI
- Button clears history array and storage

---

## Phase 4: Shell Integration (Pending)

### Task 4: Shell Command Endpoint

**Priority:** MEDIUM  
**Description:** Allow LLM to execute shell commands.

**Implementation:**
- Add `/shell` POST endpoint
- Execute command via `std::process::Command`
- Return stdout/stderr
- Implement safety: whitelist commands, require confirmation for destructive

---

### Task 5: Natural Language Command Mapping

**Priority:** LOW  
**Description:** Map natural language to shell commands.

**Implementation:**
- Parse intent from user message
- Map to shell commands ("run cargo build" → `cargo build`)

---

## opencode Integration (In Progress)

### Phase 1: Text engine + install UX — ✅ Complete

- [x] `[opencode]` config + env overrides
- [x] Managed `opencode serve` child process (loopback, random password/port,
  health checks, restart-on-demand, SIGINT/SIGTERM cleanup, Linux parent-death signal)
- [x] HTTP adapter (sessions, messages, abort, permissions, questions, SSE events)
- [x] WebUI engine switcher, streaming replies, tool cards, reasoning blocks
- [x] Permission modal + question modal
- [x] Session list / resume / delete
- [x] Toolchain status and one-click opencode install (loopback-only, streamed log)
- [x] Voice loop uses opencode via blocking message API
- [x] Rust unit + mock-server integration tests
- [x] Playwright suite for the opencode UI

### Phase 2: Realtime voice — ✅ Core complete (2026-09-15)

- [x] WebSocket voice endpoint (`/ws/voice`) with PCM in/out
- [x] Streaming TTS (persistent piper `--output_raw`) spoken sentence-by-sentence
- [x] Full-duplex barge-in (flush playback; optional turn abort)
- [x] TTS speed control (`--length_scale`) and tool narration
- [x] Voice permission/question answering ("yes" / "no" / "always", option labels)
- [x] Per-turn latency metrics (`/voice/metrics`)
- [ ] Streaming STT partials (deferred; requires a streaming backend such as
      whisper.cpp — endpointing + streaming replies already cover the latency
      budget on this machine)

### Phase 3: Conversational conductor — ⬜ Pending

- [ ] Voice-optimized agent prompt (zero-install via `system` override)
- [ ] Optional project-local agent/tool files for agent-initiated speech
- [ ] Rate-limit/retry narration

### Phase 4: Orchestration — ⬜ Pending

- [ ] Parallel worker sessions on one server
- [ ] Optional PTY/tmux workers with worktree isolation
- [ ] Worker dashboard and narration

---

## Phase 5+: Future Features

- Project-aware context loading
- Multiple file editing suggestions
- Clipboard read/write
- File read/write via voice
- IDE plugin

---

*Wraith: Your voice-controlled AI daemon. Offline. Local. Always listening.*