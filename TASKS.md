# Wraith Development Tasks

Last updated: 2026-04-20

## Status Overview

| Phase | Status | Tasks |
|-------|--------|-------|
| Phase 1 | ✅ Complete | Core voice loop |
| Phase 2 | ✅ Complete | System refinement |
| Phase 3 | 🔄 In Progress | Persistence & Memory |
| Phase 4 | ⬜ Pending | Shell Integration |
| Phase 5+ | ⬜ Pending | Coding Context |

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

## Phase 5+: Future Features

- Project-aware context loading
- Multiple file editing suggestions
- Clipboard read/write
- File read/write via voice
- IDE plugin

---

*Wraith: Your voice-controlled AI daemon. Offline. Local. Always listening.*