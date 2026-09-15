//! Realtime voice sessions over WebSocket: PCM in, streamed speech out,
//! opencode narration, permissions and questions by voice.

use std::collections::{HashSet, VecDeque};
use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::extract::ws::{Message, WebSocket};
use bytes::Bytes;
use futures_util::stream::SplitSink;
use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use tokio::sync::{mpsc, Mutex};
use tokio_util::sync::CancellationToken;

use crate::opencode::OpencodeManager;
use crate::speech::{self, PiperWorker, SentenceChunker};

const PCM_SAMPLE_RATE: u32 = 16000;
const MAX_PCM_BYTES: usize = PCM_SAMPLE_RATE as usize * 2 * 60;
const AUDIO_IDLE_MS: u64 = 250;

#[derive(Debug, Clone)]
pub struct VoiceSettings {
    pub silence_ms: u64,
    pub min_speech_ms: u64,
    pub speed: f64,
    pub narration: String,
    pub interrupt_aborts: bool,
    pub prewarm: bool,
    pub debug: bool,
    pub tts_model: String,
}

impl Default for VoiceSettings {
    fn default() -> Self {
        Self {
            silence_ms: 650,
            min_speech_ms: 350,
            speed: 1.0,
            narration: "tools".to_string(),
            interrupt_aborts: false,
            prewarm: true,
            debug: false,
            tts_model: String::new(),
        }
    }
}

pub struct VoiceHub {
    metrics: Mutex<VecDeque<Value>>,
    active: Mutex<Option<(uuid::Uuid, CancellationToken)>>,
    say_tx: Mutex<Option<mpsc::Sender<String>>>,
}

impl VoiceHub {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            metrics: Mutex::new(VecDeque::new()),
            active: Mutex::new(None),
            say_tx: Mutex::new(None),
        })
    }

    async fn register(&self) -> (uuid::Uuid, CancellationToken) {
        let token = CancellationToken::new();
        let id = uuid::Uuid::new_v4();
        let mut active = self.active.lock().await;
        if let Some((_, previous)) = active.take() {
            previous.cancel();
        }
        *active = Some((id, token.clone()));
        (id, token)
    }

    async fn unregister(&self, id: uuid::Uuid) {
        let mut active = self.active.lock().await;
        if let Some((current, _)) = active.as_ref() {
            if *current == id {
                *active = None;
            }
        }
    }

    pub async fn set_say(&self, sender: mpsc::Sender<String>) {
        *self.say_tx.lock().await = Some(sender);
    }

    pub async fn clear_say(&self) {
        *self.say_tx.lock().await = None;
    }

    /// Speaks text through the active voice call, used by the `wraith_say` tool.
    pub async fn say(&self, text: &str) -> Result<(), String> {
        let guard = self.say_tx.lock().await;
        match guard.as_ref() {
            Some(sender) => sender
                .send(text.to_string())
                .await
                .map_err(|_| "voice session unavailable".to_string()),
            None => Err("no active voice call".to_string()),
        }
    }

    pub async fn record_turn(&self, value: Value) {
        let mut metrics = self.metrics.lock().await;
        metrics.push_back(value);
        while metrics.len() > 20 {
            metrics.pop_front();
        }
    }

    pub async fn metrics(&self) -> Value {
        let metrics = self.metrics.lock().await;
        json!({ "turns": metrics.iter().cloned().collect::<Vec<_>>() })
    }
}

type WsSink = SplitSink<WebSocket, Message>;

enum Command {
    Start {
        engine: String,
        session_id: Option<String>,
    },
    Audio(Bytes),
    Endpoint,
    Text(String),
    BargeIn {
        abort: bool,
    },
    Speed(f64),
    Stop,
    Say(String),
    AudioOut(Bytes),
    Event(Value),
    EngineDone(i64),
    EngineError(String),
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum VoiceState {
    Listening,
    Transcribing,
    Thinking,
    Working,
    Speaking,
    AwaitingPermission,
    AwaitingAnswer,
}

impl VoiceState {
    fn as_str(self) -> &'static str {
        match self {
            VoiceState::Listening => "listening",
            VoiceState::Transcribing => "transcribing",
            VoiceState::Thinking => "thinking",
            VoiceState::Working => "working",
            VoiceState::Speaking => "speaking",
            VoiceState::AwaitingPermission => "awaiting_permission",
            VoiceState::AwaitingAnswer => "awaiting_answer",
        }
    }
}

struct Session {
    settings: VoiceSettings,
    manager: Arc<OpencodeManager>,
    hub: Arc<VoiceHub>,
    cmd_tx: mpsc::Sender<Command>,
    engine: String,
    engine_session: Option<String>,
    state: VoiceState,
    pcm: Vec<u8>,
    chunker: SentenceChunker,
    piper: Option<PiperWorker>,
    speed: f64,
    generation: i64,
    cancel: Option<CancellationToken>,
    pending_permission: Option<Value>,
    pending_question: Option<Value>,
    question_index: usize,
    question_answers: Vec<Vec<String>>,
    muted: bool,
    delta_parts: HashSet<String>,
    updated_parts: HashSet<String>,
    assistant_messages: HashSet<String>,
    user_messages: HashSet<String>,
    turn_finished: bool,
    prewarm_discard_until: Option<Instant>,
    last_audio_at: Instant,
    stt_ms: u64,
    sentences_sent: u64,
    audio_since_sentence: bool,
    last_sentence_at: Instant,
    first_token_at: Option<Instant>,
    first_audio_at: Option<Instant>,
    turn_started_at: Option<Instant>,
    stop_requested: bool,
}

impl Session {
    fn new(
        settings: VoiceSettings,
        manager: Arc<OpencodeManager>,
        hub: Arc<VoiceHub>,
        cmd_tx: mpsc::Sender<Command>,
    ) -> Self {
        let speed = settings.speed;
        Self {
            settings,
            manager,
            hub,
            cmd_tx,
            engine: "opencode".to_string(),
            engine_session: None,
            state: VoiceState::Listening,
            pcm: Vec::new(),
            chunker: SentenceChunker::new(speech::DEFAULT_MAX_SENTENCE_CHARS),
            piper: None,
            speed,
            generation: 0,
            cancel: None,
            pending_permission: None,
            pending_question: None,
            question_index: 0,
            question_answers: Vec::new(),
            muted: false,
            delta_parts: HashSet::new(),
            updated_parts: HashSet::new(),
            assistant_messages: HashSet::new(),
            user_messages: HashSet::new(),
            turn_finished: false,
            prewarm_discard_until: None,
            last_audio_at: Instant::now(),
            stt_ms: 0,
            sentences_sent: 0,
            audio_since_sentence: true,
            last_sentence_at: Instant::now(),
            first_token_at: None,
            first_audio_at: None,
            turn_started_at: None,
            stop_requested: false,
        }
    }

    fn length_scale(&self) -> f64 {
        1.0 / self.speed.clamp(0.5, 2.0)
    }

    async fn send(sink: &mut WsSink, value: Value) {
        let text = value.to_string();
        let _ = sink.send(Message::Text(text.into())).await;
    }

    async fn send_state(&self, sink: &mut WsSink) {
        Self::send(sink, json!({ "type": "state", "value": self.state.as_str() })).await;
    }

    async fn start(&mut self, engine: String, session_id: Option<String>, sink: &mut WsSink) {
        self.engine = if engine == "local" { "local" } else { "opencode" }.to_string();

        if self.engine == "opencode" {
            let session = match session_id.filter(|id| !id.trim().is_empty()) {
                Some(id) => Some(id),
                None => match self.manager.create_session(Some("Wraith voice".to_string())).await {
                    Ok(value) => value.get("id").and_then(|v| v.as_str()).map(str::to_string),
                    Err(e) => {
                        Self::send(sink, json!({ "type": "error", "error": e })).await;
                        None
                    }
                },
            };
            self.engine_session = session.clone();
            Self::send(sink, json!({ "type": "session", "session_id": session })).await;
        }

        if let Err(e) = self.start_piper().await {
            Self::send(sink, json!({ "type": "error", "error": e })).await;
        }

        self.state = VoiceState::Listening;
        self.pcm.clear();
        self.send_state(sink).await;
        Self::send(
            sink,
            json!({
                "type": "config",
                "silence_ms": self.settings.silence_ms,
                "min_speech_ms": self.settings.min_speech_ms,
                "speed": self.speed,
                "sample_rate": self.piper.as_ref().map(|p| p.sample_rate).unwrap_or(speech::DEFAULT_SAMPLE_RATE),
                "narration": self.settings.narration,
                "debug": self.settings.debug,
            }),
        )
        .await;
    }

    async fn start_piper(&mut self) -> Result<(), String> {
        if self.piper.is_some() {
            return Ok(());
        }
        let (audio_tx, mut audio_rx) = mpsc::channel::<Bytes>(64);
        let worker = PiperWorker::spawn(&self.settings.tts_model, self.length_scale(), audio_tx).await?;
        let cmd_tx = self.cmd_tx.clone();
        tokio::spawn(async move {
            while let Some(chunk) = audio_rx.recv().await {
                if cmd_tx.send(Command::AudioOut(chunk)).await.is_err() {
                    break;
                }
            }
        });
        self.piper = Some(worker);

        if self.settings.prewarm {
            self.prewarm_discard_until = Some(Instant::now() + Duration::from_millis(900));
            if let Some(piper) = self.piper.as_mut() {
                let _ = piper.say(" ").await;
            }
        }

        Ok(())
    }

    async fn shutdown(&mut self) {
        if let Some(cancel) = self.cancel.take() {
            cancel.cancel();
        }
        if let Some(mut piper) = self.piper.take() {
            let _ = piper.restart(self.length_scale()).await;
            // Dropping the worker kills the child (kill_on_drop).
            drop(piper);
        }
    }

    async fn speak(&mut self, text: &str, sink: &mut WsSink) {
        if self.muted {
            return;
        }
        if let Some(cleaned) = speech::clean_for_speech(text) {
            Self::send(sink, json!({ "type": "sentence", "text": cleaned })).await;
            if self.first_audio_at.is_none() {
                self.first_audio_at = Some(Instant::now());
            }
            self.sentences_sent += 1;
            self.audio_since_sentence = false;
            self.last_sentence_at = Instant::now();
            if let Some(piper) = self.piper.as_mut() {
                if let Err(e) = piper.say(&cleaned).await {
                    eprintln!("[voice] piper error: {e}");
                }
            }
        }
    }

    async fn speak_sentences(&mut self, sentences: Vec<String>, sink: &mut WsSink) {
        for sentence in sentences {
            self.speak(&sentence, sink).await;
        }
    }

    async fn begin_opencode_turn(&mut self, text: &str, sink: &mut WsSink) {
        let Some(session_id) = self.engine_session.clone() else {
            Self::send(sink, json!({ "type": "error", "error": "no opencode session" })).await;
            self.state = VoiceState::Listening;
            self.send_state(sink).await;
            return;
        };

        self.generation += 1;
        let generation = self.generation;
        let cancel = CancellationToken::new();
        if let Some(previous) = self.cancel.take() {
            previous.cancel();
        }
        self.cancel = Some(cancel.clone());
        self.muted = false;
        self.turn_finished = false;
        self.delta_parts.clear();
        self.updated_parts.clear();
        self.assistant_messages.clear();
        self.user_messages.clear();
        self.audio_since_sentence = true;
        self.sentences_sent = 0;
        self.first_token_at = None;
        self.first_audio_at = None;
        self.turn_started_at = Some(Instant::now());
        self.state = VoiceState::Thinking;
        self.send_state(sink).await;

        let manager = Arc::clone(&self.manager);
        let cmd_tx = self.cmd_tx.clone();
        let prompt = text.to_string();

        tokio::spawn(async move {
            let mut events = manager.subscribe();

            if let Err(e) = manager
                .prompt_async_full(&session_id, &prompt, None, None, Some(VOICE_PERSONA))
                .await
            {
                let _ = cmd_tx.send(Command::EngineError(e)).await;
                let _ = cmd_tx.send(Command::EngineDone(generation)).await;
                return;
            }

            loop {
                tokio::select! {
                    _ = cancel.cancelled() => return,
                    event = events.recv() => match event {
                        Ok(value) => {
                            let event_type = value.get("type").and_then(|t| t.as_str()).unwrap_or_default();
                            let event_session = value
                                .get("properties")
                                .and_then(|p| p.get("sessionID").or_else(|| p.get("part").and_then(|part| part.get("sessionID"))))
                                .and_then(|s| s.as_str());

                            if event_type == "session.idle" && event_session == Some(session_id.as_str()) {
                                let _ = cmd_tx.send(Command::EngineDone(generation)).await;
                                return;
                            }
                            if event_type == "session.error" && event_session == Some(session_id.as_str()) {
                                let _ = cmd_tx.send(Command::EngineDone(generation)).await;
                                return;
                            }
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                        Err(tokio::sync::broadcast::error::RecvError::Closed) => return,
                    }
                }
            }
        });
    }

    async fn begin_local_turn(&mut self, text: &str, sink: &mut WsSink) {
        self.muted = false;
        self.turn_finished = false;
        self.delta_parts.clear();
        self.updated_parts.clear();
        self.assistant_messages.clear();
        self.user_messages.clear();
        self.audio_since_sentence = true;
        self.sentences_sent = 0;
        self.first_token_at = None;
        self.first_audio_at = None;
        self.turn_started_at = Some(Instant::now());
        self.state = VoiceState::Thinking;
        self.send_state(sink).await;

        let response = crate::local_chat(text).await;
        match response {
            Ok(text) => {
                if self.first_token_at.is_none() {
                    self.first_token_at = Some(Instant::now());
                }
                self.state = VoiceState::Speaking;
                self.send_state(sink).await;
                let mut chunker = SentenceChunker::new(speech::DEFAULT_MAX_SENTENCE_CHARS);
                let mut sentences = chunker.push(&text);
                if let Some(rest) = chunker.flush() {
                    sentences.push(rest);
                }
                self.speak_sentences(sentences, sink).await;
            }
            Err(e) => {
                Self::send(sink, json!({ "type": "error", "error": e })).await;
            }
        }

        self.turn_finished = true;
    }

    async fn handle_final_text(&mut self, text: String, sink: &mut WsSink) {
        let trimmed = text.trim().to_string();
        if trimmed.is_empty() {
            self.state = VoiceState::Listening;
            self.send_state(sink).await;
            return;
        }

        if self.state == VoiceState::AwaitingPermission {
            if let Some((response, remember)) = match_permission_answer(&trimmed) {
                if let Some(permission) = self.pending_permission.take() {
                    let session_id = permission
                        .get("sessionID")
                        .and_then(|v| v.as_str())
                        .unwrap_or_default()
                        .to_string();
                    let permission_id = permission
                        .get("id")
                        .and_then(|v| v.as_str())
                        .unwrap_or_default()
                        .to_string();
                    let _ = self
                        .manager
                        .reply_permission(&session_id, &permission_id, response, remember)
                        .await;
                    Self::send(
                        sink,
                        json!({ "type": "permission_reply", "id": permission_id, "response": response }),
                    )
                    .await;
                    self.state = VoiceState::Working;
                    self.send_state(sink).await;
                    return;
                }
            } else {
                self.speak("I need a yes, no, or always.", sink).await;
                return;
            }
        }

        if self.state == VoiceState::AwaitingAnswer {
            if let Some(question) = self.pending_question.clone() {
                let questions: Vec<Value> = question
                    .get("questions")
                    .and_then(|q| q.as_array())
                    .cloned()
                    .unwrap_or_default();
                let index = self.question_index.min(questions.len().saturating_sub(1));
                let answer = questions
                    .get(index)
                    .map(|q| match_question_answer(q, &trimmed))
                    .unwrap_or_else(|| vec![trimmed.clone()]);
                self.question_answers.push(answer);

                if self.question_index + 1 < questions.len() {
                    self.question_index += 1;
                    self.speak_next_question(sink).await;
                    return;
                }

                let request_id = question
                    .get("id")
                    .and_then(|v| v.as_str())
                    .unwrap_or_default()
                    .to_string();
                let answers = std::mem::take(&mut self.question_answers);
                let _ = self.manager.reply_question(&request_id, answers).await;
                self.pending_question = None;
                Self::send(sink, json!({ "type": "question_reply", "id": request_id })).await;
                self.state = VoiceState::Working;
                self.send_state(sink).await;
                return;
            }
        }

        if is_stop_command(&trimmed) {
            self.cancel_turn().await;
            self.speak("Stopped.", sink).await;
            self.state = VoiceState::Listening;
            self.send_state(sink).await;
            return;
        }

        if let Some((_, new_speed)) = speech::parse_speed_command(&trimmed, self.speed) {
            self.speed = new_speed;
            let scale = self.length_scale();
            if let Some(piper) = self.piper.as_mut() {
                let _ = piper.restart(scale).await;
            }
            Self::send(sink, json!({ "type": "speed", "value": self.speed })).await;
            self.speak(
                &format!("Speaking at {:.0} percent speed.", self.speed * 100.0),
                sink,
            )
            .await;
            self.state = VoiceState::Listening;
            self.send_state(sink).await;
            return;
        }

        if self.engine == "local" {
            self.begin_local_turn(&trimmed, sink).await;
        } else {
            self.begin_opencode_turn(&trimmed, sink).await;
        }
    }

    async fn cancel_turn(&mut self) {
        if let Some(cancel) = self.cancel.take() {
            cancel.cancel();
        }
        self.generation += 1;
    }

    async fn speak_next_question(&mut self, sink: &mut WsSink) {
        let Some(request) = self.pending_question.clone() else {
            return;
        };
        let questions = request
            .get("questions")
            .and_then(|q| q.as_array())
            .cloned()
            .unwrap_or_default();
        let Some(question) = questions.get(self.question_index) else {
            return;
        };

        let text = question
            .get("question")
            .and_then(|t| t.as_str())
            .unwrap_or("a question");
        self.speak(text, sink).await;

        if let Some(options) = question.get("options").and_then(|o| o.as_array()) {
            let labels: Vec<&str> = options
                .iter()
                .filter_map(|option| option.get("label").and_then(|l| l.as_str()))
                .collect();
            if !labels.is_empty() {
                self.speak(&format!("Options: {}.", labels.join(", ")), sink)
                    .await;
            }
        }
    }

    async fn handle_spoken_text(&mut self, text: &str, sink: &mut WsSink) {
        if text.is_empty() {
            return;
        }
        if self.first_token_at.is_none() {
            self.first_token_at = Some(Instant::now());
            if self.state == VoiceState::Thinking {
                self.state = VoiceState::Working;
                self.send_state(sink).await;
            }
        }
        let sentences = self.chunker.push(text);
        self.speak_sentences(sentences, sink).await;
    }

    async fn handle_tool_narration(&mut self, part: &Value, sink: &mut WsSink) {
        let tool = part.get("tool").and_then(|t| t.as_str()).unwrap_or_default();
        let state = part.get("state").cloned().unwrap_or(Value::Null);
        let status = state.get("status").and_then(|s| s.as_str()).unwrap_or_default();

        Self::send(
            sink,
            json!({
                "type": "tool",
                "tool": tool,
                "status": status,
                "title": state.get("title").and_then(|t| t.as_str()).unwrap_or_default(),
            }),
        )
        .await;

        if status != "running" || self.settings.narration == "off" || self.muted {
            return;
        }
        if self.settings.narration != "tools" {
            return;
        }
        if let Some(phrase) = tool_phrase(tool) {
            self.speak(phrase, sink).await;
        }
    }

    async fn handle_event(&mut self, event: Value, sink: &mut WsSink) {
        if std::env::var("WRAITH_VOICE_TRACE").is_ok() {
            eprintln!("[voice-trace] event {} session={:?} engine={}", event.get("type").and_then(|t| t.as_str()).unwrap_or("?"), self.engine_session, self.engine);
        }
        if self.engine != "opencode" || self.engine_session.is_none() {
            return;
        }

        let event_type = event.get("type").and_then(|t| t.as_str()).unwrap_or_default();
        let properties = event.get("properties").cloned().unwrap_or(Value::Null);

        let event_session = properties
            .get("sessionID")
            .or_else(|| properties.get("part").and_then(|part| part.get("sessionID")))
            .and_then(|s| s.as_str());
        if let Some(event_session) = event_session {
            if Some(event_session) != self.engine_session.as_deref() {
                return;
            }
        }

        match event_type {
            "message.updated" => {
                let info = properties.get("info").cloned().unwrap_or(Value::Null);
                let message_id = info.get("id").and_then(|i| i.as_str()).unwrap_or_default();
                match info.get("role").and_then(|r| r.as_str()) {
                    Some("assistant") if !message_id.is_empty() => {
                        self.assistant_messages.insert(message_id.to_string());
                    }
                    Some("user") if !message_id.is_empty() => {
                        self.user_messages.insert(message_id.to_string());
                    }
                    _ => {}
                }
            }
            "message.part.updated" => {
                let part = properties.get("part").cloned().unwrap_or(Value::Null);
                let part_type = part.get("type").and_then(|t| t.as_str()).unwrap_or_default();
                match part_type {
                    "text" => {
                        let part_id = part.get("id").and_then(|i| i.as_str()).unwrap_or_default();
                        let message_id = part.get("messageID").and_then(|m| m.as_str()).unwrap_or_default();
                        if !self.assistant_messages.contains(message_id) {
                            return;
                        }
                        if self.delta_parts.contains(part_id) {
                            return;
                        }
                        if !self.updated_parts.insert(part_id.to_string()) {
                            return;
                        }
                        if let Some(text) = part.get("text").and_then(|t| t.as_str()) {
                            self.handle_spoken_text(text, sink).await;
                        }
                    }
                    "reasoning" => {
                        let part_id = part.get("id").and_then(|i| i.as_str()).unwrap_or_default();
                        let message_id = part.get("messageID").and_then(|m| m.as_str()).unwrap_or_default();
                        if !self.assistant_messages.contains(message_id) {
                            return;
                        }
                        if self.delta_parts.contains(part_id) || !self.updated_parts.insert(part_id.to_string()) {
                            return;
                        }
                        if self.settings.narration == "thoughts" && !self.muted {
                            if let Some(text) = part.get("text").and_then(|t| t.as_str()) {
                                let sentences = self.chunker.push(text);
                                self.speak_sentences(sentences, sink).await;
                            }
                        }
                    }
                    "tool" => self.handle_tool_narration(&part, sink).await,
                    _ => {}
                }
            }
            "message.part.delta" => {
                let field = properties.get("field").and_then(|f| f.as_str()).unwrap_or_default();
                let part_id = properties
                    .get("partID")
                    .and_then(|p| p.as_str())
                    .unwrap_or_default()
                    .to_string();
                let delta = properties.get("delta").and_then(|d| d.as_str()).unwrap_or_default();
                let message_id = properties
                    .get("messageID")
                    .and_then(|m| m.as_str())
                    .unwrap_or_default();
                if self.user_messages.contains(message_id) {
                    return;
                }

                match field {
                    "text" => {
                        self.delta_parts.insert(part_id);
                        self.handle_spoken_text(delta, sink).await;
                    }
                    "reasoning" if self.settings.narration == "thoughts" && !self.muted => {
                        self.delta_parts.insert(part_id);
                        let sentences = self.chunker.push(delta);
                        self.speak_sentences(sentences, sink).await;
                    }
                    _ => {}
                }
            }
            "permission.updated" => {
                self.pending_permission = Some(properties.clone());
                let title = properties
                    .get("title")
                    .and_then(|t| t.as_str())
                    .unwrap_or("a permission request");
                self.speak(&format!("I need permission to {title}. Yes, no, or always?"), sink)
                    .await;
                Self::send(sink, json!({ "type": "permission", "permission": properties })).await;
                self.state = VoiceState::AwaitingPermission;
                self.send_state(sink).await;
            }
            "question.asked" | "question.v2.asked" => {
                self.pending_question = Some(properties.clone());
                self.question_index = 0;
                self.question_answers.clear();
                self.speak_next_question(sink).await;
                Self::send(sink, json!({ "type": "question", "question": properties })).await;
                self.state = VoiceState::AwaitingAnswer;
                self.send_state(sink).await;
            }
            "session.status" => {
                let status = properties
                    .get("status")
                    .and_then(|s| s.get("type"))
                    .and_then(|t| t.as_str())
                    .unwrap_or_default();
                match status {
                    "busy" => {
                        if self.state == VoiceState::Thinking {
                            self.state = VoiceState::Working;
                            self.send_state(sink).await;
                        }
                    }
                    "retry" => {
                        self.speak("One moment, reconnecting.", sink).await;
                    }
                    _ => {}
                }
            }
            "session.error" => {
                Self::send(sink, json!({ "type": "error", "error": "opencode session error" })).await;
                self.turn_finished = true;
            }
            _ => {}
        }
    }

    async fn finish_turn(&mut self, sink: &mut WsSink) {
        if let Some(rest) = self.chunker.flush() {
            self.speak(&rest, sink).await;
        }
        self.turn_finished = true;

        if matches!(
            self.state,
            VoiceState::Thinking | VoiceState::Working
        ) {
            self.state = VoiceState::Speaking;
        }

        let started = self.turn_started_at.take();
        let turn = json!({
            "at": chrono_lite_timestamp(),
            "stt_ms": self.stt_ms,
            "first_token_ms": self.first_token_at.map(|t| t.duration_since(started.unwrap_or(t)).as_millis() as u64),
            "first_audio_ms": self.first_audio_at.map(|t| t.duration_since(started.unwrap_or(t)).as_millis() as u64),
            "total_ms": started.map(|t| t.elapsed().as_millis() as u64),
        });

        if self.settings.debug {
            eprintln!("[voice] turn latency: {turn}");
        }
        self.hub.record_turn(turn).await;
    }

    async fn tick(&mut self, sink: &mut WsSink) {
        if let Some(until) = self.prewarm_discard_until {
            if Instant::now() >= until
                || self.last_audio_at.elapsed() > Duration::from_millis(AUDIO_IDLE_MS)
            {
                self.prewarm_discard_until = None;
            }
        }

        let idle = self.last_audio_at.elapsed();
        let drained = self.audio_since_sentence
            || self.sentences_sent == 0
            || self.last_sentence_at.elapsed() > Duration::from_secs(5);
        if self.turn_finished
            && drained
            && matches!(
                self.state,
                VoiceState::Speaking | VoiceState::Working | VoiceState::Thinking
            )
            && idle > Duration::from_millis(AUDIO_IDLE_MS)
        {
            Self::send(sink, json!({ "type": "audio_end" })).await;
            self.state = VoiceState::Listening;
            self.send_state(sink).await;
        }
    }

    async fn handle(&mut self, command: Command, sink: &mut WsSink) {
        match command {
            Command::Start { engine, session_id } => self.start(engine, session_id, sink).await,
            Command::Audio(bytes) => {
                if self.pcm.len() < MAX_PCM_BYTES {
                    self.pcm.extend_from_slice(&bytes);
                }
            }
            Command::Endpoint => {
                if self.state != VoiceState::Listening {
                    return;
                }
                let pcm = std::mem::take(&mut self.pcm);
                if pcm.len() < (PCM_SAMPLE_RATE as usize / 2) {
                    return;
                }

                self.state = VoiceState::Transcribing;
                self.send_state(sink).await;

                let started = Instant::now();
                let wav = speech::write_wav(&pcm, PCM_SAMPLE_RATE);
                let transcript = tokio::task::spawn_blocking(move || {
                    crate::stt::transcribe_wav_bytes(&wav)
                })
                .await
                .unwrap_or_else(|e| Err(format!("STT_TASK_FAILED:{e}")));
                self.stt_ms = started.elapsed().as_millis() as u64;

                match transcript {
                    Ok(text) => {
                        Self::send(sink, json!({ "type": "final", "text": text })).await;
                        self.handle_final_text(text, sink).await;
                    }
                    Err(code) => {
                        eprintln!("[voice] transcription failed: {code}");
                        let message = if code.starts_with("EMPTY_TRANSCRIPTION") {
                            "I didn't catch that."
                        } else {
                            "Speech recognition failed."
                        };
                        Self::send(sink, json!({ "type": "final", "text": "" })).await;
                        self.speak(message, sink).await;
                        self.turn_finished = true;
                        self.state = VoiceState::Speaking;
                        self.send_state(sink).await;
                    }
                }
            }
            Command::Text(text) => {
                Self::send(sink, json!({ "type": "final", "text": text })).await;
                self.handle_final_text(text, sink).await;
            }
            Command::BargeIn { abort } => {
                let scale = self.length_scale();
                if let Some(piper) = self.piper.as_mut() {
                    let _ = piper.restart(scale).await;
                }
                self.chunker = SentenceChunker::new(speech::DEFAULT_MAX_SENTENCE_CHARS);
                self.turn_finished = false;
                self.last_audio_at = Instant::now();
                Self::send(sink, json!({ "type": "audio_end", "reason": "barge_in" })).await;

                if abort || self.settings.interrupt_aborts {
                    self.cancel_turn().await;
                    if let Some(session_id) = self.engine_session.clone() {
                        let _ = self.manager.abort(&session_id).await;
                    }
                } else {
                    self.muted = true;
                }

                self.pcm.clear();
                self.state = VoiceState::Listening;
                self.send_state(sink).await;
            }
            Command::Speed(value) => {
                self.speed = value.clamp(0.5, 2.0);
                let scale = self.length_scale();
                if let Some(piper) = self.piper.as_mut() {
                    let _ = piper.restart(scale).await;
                }
                Self::send(sink, json!({ "type": "speed", "value": self.speed })).await;
            }
            Command::Stop => {
                self.stop_requested = true;
            }
            Command::Say(text) => {
                self.speak(&text, sink).await;
            }
            Command::AudioOut(chunk) => {
                self.last_audio_at = Instant::now();
                self.audio_since_sentence = true;
                if self.prewarm_discard_until.is_some() {
                    return;
                }
                if self.state != VoiceState::Speaking {
                    self.state = VoiceState::Speaking;
                    self.send_state(sink).await;
                }
                let _ = sink.send(Message::Binary(chunk)).await;
            }
            Command::Event(event) => self.handle_event(event, sink).await,
            Command::EngineDone(generation) => {
                if generation < self.generation {
                    return;
                }
                self.finish_turn(sink).await;
            }
            Command::EngineError(error) => {
                Self::send(sink, json!({ "type": "error", "error": error })).await;
                self.turn_finished = true;
            }
        }
    }
}

fn chrono_lite_timestamp() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or_default()
}

/// Runs a voice session until the socket closes.
pub async fn run_voice_socket(
    socket: WebSocket,
    settings: VoiceSettings,
    manager: Arc<OpencodeManager>,
    hub: Arc<VoiceHub>,
) {
    let (mut sink, mut stream) = socket.split();
    let (cmd_tx, mut cmd_rx) = mpsc::channel::<Command>(256);
    let closed = CancellationToken::new();
    let (hub_id, hub_token) = hub.register().await;

    let (say_tx, mut say_rx) = mpsc::channel::<String>(16);
    hub.set_say(say_tx).await;
    {
        let cmd_tx = cmd_tx.clone();
        let closed = closed.clone();
        tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = closed.cancelled() => break,
                    text = say_rx.recv() => match text {
                        Some(text) => {
                            if cmd_tx.send(Command::Say(text)).await.is_err() {
                                break;
                            }
                        }
                        None => break,
                    }
                }
            }
        });
    }

    {
        let cmd_tx = cmd_tx.clone();
        let closed = closed.clone();
        tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = closed.cancelled() => break,
                    incoming = stream.next() => match incoming {
                        Some(Ok(Message::Text(text))) => {
                            if let Ok(value) = serde_json::from_str::<Value>(&text) {
                                let command = match value.get("type").and_then(|t| t.as_str()) {
                                    Some("start") => Command::Start {
                                        engine: value.get("engine").and_then(|e| e.as_str()).unwrap_or("opencode").to_string(),
                                        session_id: value.get("session_id").and_then(|s| s.as_str()).map(str::to_string),
                                    },
                                    Some("endpoint") => Command::Endpoint,
                                    Some("text") => Command::Text(value.get("text").and_then(|t| t.as_str()).unwrap_or_default().to_string()),
                                    Some("barge_in") => Command::BargeIn { abort: value.get("abort").and_then(|a| a.as_bool()).unwrap_or(false) },
                                    Some("speed") => Command::Speed(value.get("value").and_then(|s| s.as_f64()).unwrap_or(1.0)),
                                    Some("stop") => Command::Stop,
                                    _ => continue,
                                };
                                if cmd_tx.send(command).await.is_err() {
                                    break;
                                }
                            }
                        }
                        Some(Ok(Message::Binary(data))) => {
                            if cmd_tx.send(Command::Audio(data)).await.is_err() {
                                break;
                            }
                        }
                        Some(Ok(Message::Close(_))) | None | Some(Err(_)) => {
                            let _ = cmd_tx.send(Command::Stop).await;
                            break;
                        }
                        _ => {}
                    }
                }
            }
        });
    }

    {
        let mut events = manager.subscribe();
        let cmd_tx = cmd_tx.clone();
        let closed = closed.clone();
        tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = closed.cancelled() => break,
                    event = events.recv() => match event {
                        Ok(value) => {
                            if cmd_tx.send(Command::Event(value)).await.is_err() {
                                break;
                            }
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                        Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                    }
                }
            }
        });
    }

    let mut session = Session::new(settings, manager, Arc::clone(&hub), cmd_tx.clone());
    // Initial state so the UI can render before `start`.
    Session::send(&mut sink, json!({ "type": "state", "value": "listening" })).await;

    loop {
        tokio::select! {
            _ = hub_token.cancelled() => break,
            command = tokio::time::timeout(Duration::from_millis(100), cmd_rx.recv()) => {
                match command {
                    Ok(Some(command)) => session.handle(command, &mut sink).await,
                    Ok(None) => break,
                    Err(_) => session.tick(&mut sink).await,
                }
            }
        }
        if session.stop_requested {
            break;
        }
    }

    closed.cancel();
    session.shutdown().await;
    hub.clear_say().await;
    hub.unregister(hub_id).await;
}

pub fn match_permission_answer(text: &str) -> Option<(&'static str, bool)> {
    let normalized = text.to_lowercase();
    let tokens: Vec<&str> = normalized
        .split(|c: char| !c.is_alphanumeric())
        .filter(|token| !token.is_empty())
        .collect();

    let has = |words: &[&str]| tokens.iter().any(|token| words.contains(token));

    if has(&["always", "remember", "permanent"]) {
        return Some(("always", true));
    }
    if has(&["yes", "yeah", "yep", "allow", "ok", "okay", "sure", "approve", "go"]) {
        return Some(("once", false));
    }
    if has(&["no", "nope", "reject", "deny", "cancel", "don't", "dont", "stop"]) {
        return Some(("reject", false));
    }

    None
}

pub fn match_question_answer(question: &Value, text: &str) -> Vec<String> {
    let normalized = text.to_lowercase();
    let options = question.get("options").and_then(|o| o.as_array());

    if let Some(options) = options {
        let matches: Vec<String> = options
            .iter()
            .filter_map(|option| option.get("label").and_then(|l| l.as_str()))
            .filter(|label| {
                let label_lower = label.to_lowercase();
                normalized.contains(&label_lower) || label_lower.contains(normalized.trim())
            })
            .map(str::to_string)
            .collect();

        if !matches.is_empty() {
            return matches;
        }
    }

    vec![text.trim().to_string()]
}

fn is_stop_command(text: &str) -> bool {
    let normalized = text.to_lowercase();
    let trimmed = normalized.trim().trim_end_matches(['.', '!']);
    matches!(trimmed, "stop" | "cancel" | "stop it" | "never mind" | "nevermind")
}

fn tool_phrase(tool: &str) -> Option<&'static str> {
    match tool {
        "bash" => Some("Running a command."),
        "read" => Some("Reading a file."),
        "edit" | "write" | "patch" | "apply_patch" => Some("Editing files."),
        "grep" | "glob" | "list" => Some("Searching the codebase."),
        "task" => Some("Starting a helper agent."),
        "webfetch" | "websearch" => Some("Looking that up."),
        _ => None,
    }
}

/// The zero-install voice persona injected on each opencode prompt.
pub const VOICE_PERSONA: &str = "You are speaking with the user over a realtime voice call. \
Keep spoken replies short and natural, like a colleague on the phone. \
Do not read code, file paths, URLs, or long lists aloud; summarize them and keep details on screen. \
Briefly say what you are about to do before long-running tools. \
If a tool needs permission, the user will be asked by voice.";
