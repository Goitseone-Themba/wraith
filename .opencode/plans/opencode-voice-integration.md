# Wraith × opencode — Realtime Voice Integration Plan

**Date:** 2026-09-15
**Status:** Phase 0–1 complete, Phase 2+ pending
**Goal:** Full-duplex spoken conversation with the opencode agent from the Wraith
WebUI, with opencode able to use its tools on this machine, plus a portable Rust
binary and UI-guided opencode installation.

Legend: ✅ done · 🔄 in progress · ⬜ pending

---

## 1. Objectives and latency targets

Interact with opencode through the existing Wraith UI, by voice or text, with a
conversation that feels like a phone call rather than a request/response form.

| Metric | Target |
|---|---|
| Partial transcript after speech | < 400 ms |
| Endpoint → first audio out | ≤ 1.2 s |
| Barge-in silence | ≤ 150 ms |
| First spoken acknowledgement | ~instant (pre-synthesized clips) |

Constraints and assumptions:

- Delivery form: browser WebUI served by the Rust binary (phone access preserved).
- True duplex needs headphones on a laptop; browser AEC with speakers is imperfect.
- Provider latency (opencode Go / free Zen) is variable — never block on it, and
  speak retry/rate-limit states instead of going silent.

---

## 2. Locked decisions

1. **Realtime full duplex** is the north star: mic stays hot while the assistant
   speaks; barge-in stops narration and optionally aborts the turn.
2. **Browser WebUI in the Rust binary.** One self-contained server; no native GUI.
3. **Install UX:** detect opencode and show platform-specific commands with a
   copy button; one-click install runs the official installer server-side with an
   explicit confirmation and is **loopback-only**. Never silent-install.
4. **Selectable engines:** LM Studio (local/offline) and opencode (agent with
   tools) coexist; the existing `/chat` pipeline stays as fallback.
5. **Permissions:** default `ask`. Wraith speaks the request ("I want to run X —
   yes or no?") and mirrors it in a modal; replies go to opencode's permission
   API. No `--auto`.
6. **Voice persona is zero-install:** injected per prompt via the prompt `system`
   override. Opt-in project-local agent/tool files (`.opencode/agents`,
   `.opencode/tools`) only when agent-initiated speech/questions are wanted.
7. **Orchestration order:** opencode-native subagents first, then parallel
   sessions on the one Wraith-managed server; PTY/tmux workers only for jobs that
   must outlive Wraith or be attached from a terminal. Git worktrees for
   file-editing workers.
8. **Security model:** the managed `opencode serve` binds `127.0.0.1` with a
   random per-run password; install/worker-spawn endpoints are loopback-only; a
   Wraith API token for remote LAN access is still an open follow-up.

---

## 3. Architecture

```
Browser (Wraith WebUI + AudioWorklets)
  │ WSS /ws/voice : JSON control + binary mic PCM ↔ binary TTS PCM   (Phase 2)
  │ SSE /opencode/events : filtered opencode event bus               (Phase 1)
  ▼
Wraith (Rust/axum)
  ├─ Voice state machine: idle→listening→working→speaking→awaiting_permission
  ├─ STT adapter: voxtype (now) | whisper.cpp streaming partials (Phase 2)
  ├─ Endpointer: adaptive silence + min-speech + optional semantic check
  ├─ TTS service: persistent piper, --output_raw, speed via --length_scale
  ├─ Engine: Local (LM Studio) | Opencode
  │     └─ supervises `opencode serve` (127.0.0.1, random port + password)
  │        ├─ SSE /event → event bus → browser
  │        ├─ sessions, prompt_async / blocking message, abort
  │        ├─ permissions and questions (voice/UI replies)
  │        └─ worker manager (parallel sessions; PTY/tmux later)
  ├─ Bridge API for opencode tools: /agent/say, /agent/ask (Phase 3)
  └─ Toolchain status/installer: opencode, piper, ffmpeg, voxtype, tmux
```

---

## 4. Phase 0 — hygiene — ✅ done (2026-09-15)

- Fixed `#[cfg(test)] mod tests;` pointing at a missing `src/tests.rs`; created
  the module with unit + mock-server integration tests.
- Removed the broken Rust `playwright` dev-dependency that prevented `cargo test`
  from compiling (the real browser tests run via Node).
- Wired the previously dead `get_history_handler` as `GET /history`; the build is
  warning-free.
- Added `SseParser`, URL parsing, prompt-body and text-extraction helpers with
  unit coverage.

---

## 5. Phase 1 — opencode engine, text-first — ✅ done (2026-09-15)

### Backend

- `src/opencode.rs`
  - `OpencodeManager`: spawns and supervises `opencode serve --hostname
    127.0.0.1 --port <random>` with a random `OPENCODE_SERVER_PASSWORD`,
    HTTP basic auth on every adapter call, health checks, lazy restart, event
    pump with reconnect.
  - Robust shutdown: SIGINT/SIGTERM handler stops the child; Linux
    `PR_SET_PDEATHSIG(SIGTERM)` prevents orphans if Wraith dies abruptly.
  - Tolerant SSE parsing of both `Event` and `GlobalEvent { directory, payload }`
    frames; broadcast fan-out to browser clients.
  - HTTP adapter: list/create/delete sessions, message history, `prompt_async`,
    blocking message, abort, permission reply (v1 route with v2 fallback),
    questions list/reply/reject.
- `src/tools.rs`
  - Toolchain detection: opencode (with PATH fallbacks such as `~/.opencode/bin`,
    `~/.local/bin`, mise shims), piper-tts, STT executable, ffmpeg, tmux.
  - Install methods per platform (official script, npm, brew, mise; pacman listed
    as copy-only) with availability probing.
  - One-click install: whitelisted commands only, loopback-only guard, streamed
    output via SSE, exit code surfaced to the UI.
- `src/main.rs` routes
  - `/opencode/status`, `/start`, `/stop`, `/events` (SSE)
  - `/opencode/sessions` (GET/POST), `/opencode/sessions/{id}` (DELETE),
    `/opencode/sessions/{id}/messages` (GET)
  - `/opencode/prompt`, `/opencode/message` (blocking, used by voice),
    `/opencode/abort`, `/opencode/permission`, `/opencode/questions`,
    `/opencode/question`, `/opencode/question/reject`
  - `/api/tools/status`, `/api/tools/install`, `/api/tools/install/stream`
  - `/history`
- Config: `[opencode] executable, directory, agent, model, port, auto_start,
  extra_args` plus `WRAITH_OPENCODE_*` env overrides; printed in the startup
  banner.

### Frontend (`src/index.html`)

- **Engine switch** LOCAL / OPENCODE, persisted in `localStorage`.
- opencode bar: status (version · running/stopped), session label, Sessions /
  New / Stop.
- Streaming replies from `message.part.updated`: text parts, collapsible
  reasoning, tool cards with pending/running/done/error states.
- Permission modal (Allow once / Always / Reject) and native question modal
  (single/multi select, custom answer, Skip), wired to the opencode APIs.
- Session list with resume (replays history) and delete.
- Install card when opencode is missing: recommended command, copy button,
  per-method buttons, live install log, re-check.
- Voice call loop works with either engine; opencode turns use the blocking
  message endpoint until Phase 2 streaming lands.

### Verification evidence

- `cargo test`: 12/12 (includes adapter integration tests against an in-process
  mock opencode server; asserts basic auth and request payloads).
- Playwright: 197 checks, 194 pass. The 3 failures are pre-existing and in
  untouched code (`escapeHtml` case, `synthesize` source check, "Error messages
  are user-friendly").
- Live E2E on this machine: engine switch, SSE `session.created` propagation,
  and a real prompt ("Reply with exactly one word: pong" → `pong`) through the
  blocking path, followed by session deletion.
- Orphan bug found and fixed during testing: SIGTERM to Wraith used to leave the
  managed server running.

---

## 6. Phase 2 — realtime voice core — ⬜ pending

- `GET /ws/voice` WebSocket: control JSON plus binary PCM frames both ways.
- Browser capture: AudioWorklet → 16 kHz mono Int16 chunks; playback through an
  AudioWorklet ring buffer (keeps AEC effective and enables instant flush).
- STT adapter trait: voxtype (current) → whisper.cpp server rolling partials
  (~500 ms windows) for partial transcripts; final transcript on endpoint.
- Endpointing tuned for conversation: default silence ~650 ms, min speech
  ~350 ms, optional semantic endpoint check via the local fast lane.
- TTS: persistent piper worker(s) with `--output_raw`; sentence chunker on
  `.!?` boundaries with markdown/code/URL stripping; speech queue with
  cancellation. Speed via `--length_scale` (slider + voice commands); restart the
  worker pool per speed or keep a small pool.
- Barge-in state machine: flush TTS, emit `assistant.interrupted`; distinguish
  "stop talking" from "abort session" (`POST /session/:id/abort`).
- Permission/question voice handling: speak prompt, listen for yes/no/always or
  an option label, map to the API; timeout re-asks and never auto-approves.
- Latency instrumentation: time-to-first-partial, time-to-first-audio, barge-in
  stop latency; surfaced in a small debug panel or `/metrics`.

---

## 7. Phase 3 — conversational conductor — ⬜ pending

- Voice-optimized persona via prompt `system` override (zero-install):
  speakable output, short turns, no code read aloud, full detail stays on screen.
- Native question flow: `question.asked` / `question.v2.asked` events spoken and
  answered by voice; replies via `/question/:id/reply`.
- Optional project-local `.opencode/agents/wraith-voice.md` (permission
  `question: allow`, speech rules) and `.opencode/tools/wraith.ts`
  (`wraith_say`, later `wraith_worker`) behind an explicit UI opt-in with
  preview, managed markers, backup, and uninstall. Global install stays off by
  default.
- Rate-limit/retry narration driven by `session.status: retry`; concurrency caps
  for free Zen.
- Optional fast lane (LM Studio) for canned acknowledgements, semantic
  endpointing, and speech summaries — only if measured latency demands it.

---

## 8. Phase 4 — orchestration — ⬜ pending

- Worker model: parallel sessions on the one managed server (structured events,
  per-session abort/status, voice-addressable, attachable with
  `opencode attach`).
- Worktree isolation for workers that edit files; concurrency caps by provider.
- Optional PTY/tmux backend for long-running jobs: `POST /pty`, pty events,
  `WS /pty/:id/connect`, or plain tmux; parse exit codes/result files.
- Conductor prompt for dispatch/monitor; Wraith narrates worker milestones and
  completions; worker dashboard with terminal attach.

---

## 9. Phase 5 — portability and release — ⬜ pending

- Auto-generate self-signed TLS with `rcgen` on first run (SANs: localhost,
  hostname, detected LAN addresses), stored under `~/.config/wraith/`; keep the
  existing `cert.pem`/`key.pem` flow as an override. Removes the `openssl` step.
- Release profile: `lto`, `strip`, `opt-level = "s"`; static musl builds via a
  CI matrix (x86_64/aarch64), optional macOS/Windows.
- README/config docs kept in sync; note that piper/voxtype/ffmpeg/opencode remain
  external and that the UI detects and installs opencode.

---

## 10. Verified opencode API facts (1.18.25 on this machine)

- `opencode serve`: loopback bind, OpenAPI at `/doc`, SSE at `/event`,
  basic auth via `OPENCODE_SERVER_PASSWORD` (username `opencode`).
- Sessions: `POST /session`, `GET /session`, `DELETE /session/:id`,
  `GET /session/:id/message`, `POST /session/:id/prompt_async` (204),
  `POST /session/:id/message` (blocking), `POST /session/:id/abort`.
- Permissions: `POST /session/:id/permissions/:permissionID` with
  `{ response: "once" | "always" | "reject", remember? }`; newer builds also
  expose experimental `/permission/:id/reply`. Events: `permission.updated`,
  `permission.replied`.
- Questions: `GET /question`, `POST /question/:id/reply` `{ answers: string[][] }`,
  `POST /question/:id/reject`; events `question.asked` / `question.v2.asked`
  (and replied/rejected variants). The `question` tool is denied by default for
  most built-in agents, so enabling it requires an agent config (Phase 3).
- Streaming: `message.part.updated` with `delta`; parts include `text`,
  `reasoning`, `tool` (state `pending/running/completed/error` with `title` and
  `input`), plus step/snapshot/patch parts. `session.status` is
  `idle | busy | retry`; `session.idle` marks turn completion.
- Workers/terminals: `POST /pty`, `GET /pty`, `GET /pty/:id`,
  `WS /pty/:id/connect`, events `pty.created/updated/exited/deleted`.

---

## 11. Risks and open questions

- **Echo/duplex quality** on speakers; recommend headphones, consider ducking TTS
  while the user speaks.
- **API drift:** opencode's server surface evolves; keep all calls behind
  `src/opencode.rs`, feature-detect via `/doc`, and fall back gracefully.
- **CPU contention** on the i5-12500H (whisper + piper + opencode + LSP): tiny/
  base models, bounded threads, measure before enabling partials.
- **Remote access:** Wraith is served on `0.0.0.0` for phone use; prompting the
  agent over LAN currently shares the existing trust model. A Wraith API token is
  the next security work item (install stays loopback-only meanwhile).
- **Session mapping:** one Wraith conversation maps to one opencode session per
  engine; cross-engine history is not unified. New session resets context.
- **Voice with opencode is blocking** until Phase 2, so long agent turns are
  silent on the voice path today.
- **Two-lane design** (fast local model for banter/fillers) is deferred until
  measurements show streaming opencode is not enough.
- **Question tool default:** `question: deny` for built-in agents on this machine;
  Phase 3's opt-in agent file is what unlocks agent-initiated spoken questions.

---

## 12. File map

| Path | Purpose |
|---|---|
| `src/main.rs` | Axum server, routes, config, VAD config, tool install endpoints |
| `src/opencode.rs` | Managed `opencode serve` + HTTP adapter + SSE parsing/pump |
| `src/tools.rs` | Toolchain detection, install methods, streamed installer |
| `src/tests.rs` | Unit tests + mock opencode server integration tests |
| `src/index.html` | WebUI: engine switch, streaming chat, modals, install card |
| `tests/playwright.test.js` | Browser test suite (suite 20 covers the opencode UI) |
| `.wraith.toml.example` | Config reference incl. `[opencode]` |
| `README.md` | Usage, engines, configuration, roadmap |
| `TASKS.md` | Phase tracking incl. opencode phases |

---

## 13. Execution plan (Phases 2–5)

Baseline invariants for the executor:

- `cargo test` = 12/12; build warning-free; Playwright baseline **194/197**
  (pre-existing failures: `escapeHtml` case, `synthesize` source check,
  "Error messages are user-friendly").
- Managed opencode stays loopback + random password; install/worker endpoints
  loopback-only; never `--auto`, never silent-install.
- Per-phase gate: build warning-free → `cargo test` green → Playwright ≥ baseline
  → update this file and `TASKS.md` → commit.

Measured facts: piper streams raw PCM per stdin line (first sentence ~0.6 s
`lessac-medium` / ~1.0 s `libritts_r-high` including model load, later sentences
~0.25 s), both voices 22050 Hz. No whisper binaries installed. `tokio-tungstenite`
0.28 already in the lock.

### Phase 2 — realtime voice core — ✅ done (2026-09-15)

- **P2-1** ✅ `[voice]` config + env overrides (`silence_ms`, `min_speech_ms`,
  `speed`, `narration`, `interrupt_aborts`, `prewarm`, `debug`).
- **P2-2** ✅ `src/speech.rs`: `SentenceChunker` (code-fence aware), persistent
  `PiperWorker` with prewarm/restart, `write_wav`, sample-rate detection.
- **P2-3** ✅ `src/stt.rs`: ffmpeg+voxtype pipeline extracted and shared by the
  HTTP endpoint and the voice session. Streaming partials deferred (no streaming
  backend installed; endpointing covers the budget).
- **P2-4** ✅ `src/voice.rs`: session state machine, narration map, voice
  commands, permission/question keyword answers, latency metrics, hub singleton.
- **P2-5** ✅ `/ws/voice` protocol implemented; handles both `message.part.updated`
  and the newer `message.part.delta` streaming events.
- **P2-6** ✅ frontend: inline AudioWorklets (16 kHz capture, resampling player),
  client VAD, barge-in flush, overlay states, real WebSocket for the opencode
  engine (legacy loop retained for LOCAL).
- **P2-7** ✅ Rust unit tests (chunker/WAV/speed parsing) and Playwright suite 21
  with a fake `WebSocket` + deterministic capture stub.
- **P2-8** ✅ acceptance: synthesized speech through Chromium's fake audio file
  into the real capture worklet → voxtype STT → opencode → piper PCM; warm turn
  measured first token 1.67 s / first audio 2.14 s.

### Phase 3 — conversational conductor

- **P3-1** ✅ persona via prompt `system` override (VOICE_PERSONA).
- **P3-2** ✅ multi-question voice answering with staged questions.
- **P3-3** ✅ opt-in `.opencode/agents/wraith-voice.md` + `.opencode/tools/wraith.ts`
  (`wraith_say`, `wraith_worker`), managed markers, loopback-only endpoints,
  token-guarded `/agent/say` and `/agent/worker` bridge.
- **P3-4** ✅ gate evaluated: warm first audio 2.14 s < 2.5 s → fast lane
  deferred, documented.

### Phase 4 — orchestration

- **P4-1** ✅ worker sessions on one server (`/opencode/workers`), Workers panel
  (spawn/status/abort/delete/switch), `wraith_worker` tool.
- **P4-2/3** ⬜ git-worktree isolation and PTY/tmux backend deferred; workers
  share the project directory for now.

### Phase 5 — portability and release

- **P5-1** ✅ `rcgen` auto-TLS with SANs; default path `~/.config/wraith/tls/`,
  0600 key; `auto_tls` config/env.
- **P5-2** ✅ `.github/workflows/ci.yml` (build/test/clippy) and `release.yml`
  (musl x86_64/aarch64 artifacts).
- **P5-3** ✅ README/config docs and opt-in agent/worker documentation.

### Definition of done

`cargo run --release` → engine OPENCODE → Voice Call: natural conversation,
streamed speech, barge-in, voice permissions, tool narration, session
resume/delete, UI-driven opencode install, phone browser support, small binary.

