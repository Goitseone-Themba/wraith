#[cfg(test)]
mod tests;

mod agent;
mod opencode;
mod speech;
mod stt;
mod tls;
mod tools;
mod voice;

use serde::Deserialize;
use std::convert::Infallible;
use std::env;
use std::fs;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use axum::{
    extract::ws::WebSocketUpgrade,
    extract::ConnectInfo,
    response::sse::{Event as SseEvent, Sse},
    response::{Html, IntoResponse},
    routing::{get, post},
    Form, Json, Router,
};
use axum_server::tls_rustls::RustlsConfig;
use base64::prelude::*;
use futures_util::{stream, Stream};
use serde_json::{json, Value};
use std::io::Write;
use std::process::{Command, Stdio};
use tokio::sync::broadcast;
use tower_http::cors::CorsLayer;

const MAX_RETRIES: u32 = 1;
const RETRY_DELAY_MS: u64 = 100;

#[derive(Debug, Deserialize)]
struct Config {
    server: Option<ServerConfig>,
    llm: Option<LlmConfig>,
    tts: Option<TtsConfig>,
    stt: Option<SttConfig>,
    vad: Option<VadConfig>,
    security: Option<SecurityConfig>,
    opencode: Option<OpencodeConfig>,
    voice: Option<VoiceConfigFile>,
}

#[derive(Debug, Deserialize)]
struct OpencodeConfig {
    executable: Option<String>,
    directory: Option<String>,
    agent: Option<String>,
    model: Option<String>,
    port: Option<u16>,
    auto_start: Option<bool>,
    extra_args: Option<Vec<String>>,
}

#[derive(Debug, Deserialize)]
struct VoiceConfigFile {
    silence_ms: Option<u64>,
    min_speech_ms: Option<u64>,
    speed: Option<f64>,
    narration: Option<String>,
    interrupt_aborts: Option<bool>,
    prewarm: Option<bool>,
    debug: Option<bool>,
}

#[derive(Debug, Deserialize)]
struct ServerConfig {
    host: Option<String>,
    port: Option<u16>,
}

#[derive(Debug, Deserialize)]
struct LlmConfig {
    model: Option<String>,
    endpoint: Option<String>,
}

#[derive(Debug, Deserialize)]
struct TtsConfig {
    model: Option<String>,
}

#[derive(Debug, Deserialize)]
struct SttConfig {
    executable: Option<String>,
}

#[derive(Debug, Deserialize)]
struct VadConfig {
    silence_threshold_ms: Option<u64>,
    volume_threshold_speaking: Option<f64>,
    volume_threshold_interrupt: Option<f64>,
    min_recording_duration_ms: Option<u64>,
}

#[derive(Debug, Deserialize)]
struct SecurityConfig {
    cert_path: Option<String>,
    key_path: Option<String>,
    auto_tls: Option<bool>,
}

impl Config {
    fn server_host(&self) -> String {
        env::var("WRAITH_HOST")
            .ok()
            .or_else(|| self.server.as_ref().and_then(|s| s.host.clone()))
            .unwrap_or_else(|| "0.0.0.0".to_string())
    }

    fn server_port(&self) -> u16 {
        env::var("WRAITH_PORT")
            .ok()
            .and_then(|p| p.parse().ok())
            .or_else(|| self.server.as_ref().and_then(|s| s.port))
            .unwrap_or(2026)
    }

    fn llm_model(&self) -> String {
        env::var("WRAITH_LLM_MODEL")
            .ok()
            .or_else(|| self.llm.as_ref().and_then(|l| l.model.clone()))
            .unwrap_or_else(|| "liquid/lfm2.5-1.2b".to_string())
    }

    fn llm_endpoint(&self) -> String {
        env::var("WRAITH_LLM_ENDPOINT")
            .ok()
            .or_else(|| self.llm.as_ref().and_then(|l| l.endpoint.clone()))
            .unwrap_or_else(|| "http://localhost:1234/v1/chat/completions".to_string())
    }

    fn tts_model(&self) -> String {
        env::var("WRAITH_TTS_MODEL")
            .ok()
            .or_else(|| self.tts.as_ref().and_then(|t| t.model.clone()))
            .unwrap_or_else(|| "/home/goitseone/piper-voices/en_US-libritts_r-high.onnx".to_string())
    }

    fn stt_executable(&self) -> String {
        env::var("WRAITH_STT_EXECUTABLE")
            .ok()
            .or_else(|| self.stt.as_ref().and_then(|s| s.executable.clone()))
            .unwrap_or_else(|| "voxtype".to_string())
    }

    fn vad_silence_threshold_ms(&self) -> u64 {
        env::var("WRAITH_VAD_SILENCE_MS")
            .ok()
            .and_then(|p| p.parse().ok())
            .or_else(|| self.vad.as_ref().and_then(|v| v.silence_threshold_ms))
            .unwrap_or(3000)
    }

    fn vad_volume_threshold_speaking(&self) -> f64 {
        env::var("WRAITH_VAD_VOLUME_SPEAKING")
            .ok()
            .and_then(|p| p.parse().ok())
            .or_else(|| self.vad.as_ref().and_then(|v| v.volume_threshold_speaking))
            .unwrap_or(5.0)
    }

    fn vad_volume_threshold_interrupt(&self) -> f64 {
        env::var("WRAITH_VAD_VOLUME_INTERRUPT")
            .ok()
            .and_then(|p| p.parse().ok())
            .or_else(|| self.vad.as_ref().and_then(|v| v.volume_threshold_interrupt))
            .unwrap_or(8.0)
    }

    fn vad_min_recording_duration_ms(&self) -> u64 {
        env::var("WRAITH_VAD_MIN_RECORDING_MS")
            .ok()
            .and_then(|p| p.parse().ok())
            .or_else(|| self.vad.as_ref().and_then(|v| v.min_recording_duration_ms))
            .unwrap_or(500)
    }

    fn cert_path(&self) -> String {
        env::var("WRAITH_CERT_PATH")
            .ok()
            .or_else(|| self.security.as_ref().and_then(|s| s.cert_path.clone()))
            .unwrap_or_else(|| "cert.pem".to_string())
    }

    fn key_path(&self) -> String {
        env::var("WRAITH_KEY_PATH")
            .ok()
            .or_else(|| self.security.as_ref().and_then(|s| s.key_path.clone()))
            .unwrap_or_else(|| "key.pem".to_string())
    }

    fn auto_tls(&self) -> bool {
        env::var("WRAITH_AUTO_TLS")
            .ok()
            .map(|v| matches!(v.as_str(), "1" | "true" | "yes" | "on"))
            .or_else(|| self.security.as_ref().and_then(|s| s.auto_tls))
            .unwrap_or(true)
    }

    fn opencode_executable(&self) -> String {
        env::var("WRAITH_OPENCODE_EXECUTABLE")
            .ok()
            .or_else(|| self.opencode.as_ref().and_then(|o| o.executable.clone()))
            .unwrap_or_else(|| "opencode".to_string())
    }

    fn opencode_directory(&self) -> String {
        env::var("WRAITH_OPENCODE_DIRECTORY")
            .ok()
            .or_else(|| self.opencode.as_ref().and_then(|o| o.directory.clone()))
            .unwrap_or_default()
    }

    fn opencode_agent(&self) -> String {
        env::var("WRAITH_OPENCODE_AGENT")
            .ok()
            .or_else(|| self.opencode.as_ref().and_then(|o| o.agent.clone()))
            .unwrap_or_default()
    }

    fn opencode_model(&self) -> String {
        env::var("WRAITH_OPENCODE_MODEL")
            .ok()
            .or_else(|| self.opencode.as_ref().and_then(|o| o.model.clone()))
            .unwrap_or_default()
    }

    fn opencode_port(&self) -> u16 {
        env::var("WRAITH_OPENCODE_PORT")
            .ok()
            .and_then(|p| p.parse().ok())
            .or_else(|| self.opencode.as_ref().and_then(|o| o.port))
            .unwrap_or(0)
    }

    fn opencode_auto_start(&self) -> bool {
        env::var("WRAITH_OPENCODE_AUTO_START")
            .ok()
            .map(|v| matches!(v.as_str(), "1" | "true" | "yes" | "on"))
            .or_else(|| self.opencode.as_ref().and_then(|o| o.auto_start))
            .unwrap_or(false)
    }

    fn opencode_extra_args(&self) -> Vec<String> {
        env::var("WRAITH_OPENCODE_EXTRA_ARGS")
            .ok()
            .map(|v| v.split_whitespace().map(str::to_string).collect())
            .or_else(|| self.opencode.as_ref().and_then(|o| o.extra_args.clone()))
            .unwrap_or_default()
    }

    fn opencode_settings(&self) -> opencode::OpencodeSettings {
        opencode::OpencodeSettings {
            executable: self.opencode_executable(),
            directory: self.opencode_directory(),
            agent: self.opencode_agent(),
            model: self.opencode_model(),
            port: self.opencode_port(),
            extra_args: self.opencode_extra_args(),
            bridge_url: format!("http://127.0.0.1:{}", self.server_port()),
            bridge_token: bridge_token(),
        }
    }

    fn voice_silence_ms(&self) -> u64 {
        env::var("WRAITH_VOICE_SILENCE_MS")
            .ok()
            .and_then(|v| v.parse().ok())
            .or_else(|| self.voice.as_ref().and_then(|v| v.silence_ms))
            .unwrap_or(650)
    }

    fn voice_min_speech_ms(&self) -> u64 {
        env::var("WRAITH_VOICE_MIN_SPEECH_MS")
            .ok()
            .and_then(|v| v.parse().ok())
            .or_else(|| self.voice.as_ref().and_then(|v| v.min_speech_ms))
            .unwrap_or(350)
    }

    fn voice_speed(&self) -> f64 {
        env::var("WRAITH_VOICE_SPEED")
            .ok()
            .and_then(|v| v.parse().ok())
            .or_else(|| self.voice.as_ref().and_then(|v| v.speed))
            .unwrap_or(1.0)
            .clamp(0.5, 2.0)
    }

    fn voice_narration(&self) -> String {
        env::var("WRAITH_VOICE_NARRATION")
            .ok()
            .or_else(|| self.voice.as_ref().and_then(|v| v.narration.clone()))
            .unwrap_or_else(|| "tools".to_string())
    }

    fn voice_interrupt_aborts(&self) -> bool {
        env::var("WRAITH_VOICE_INTERRUPT_ABORTS")
            .ok()
            .map(|v| matches!(v.as_str(), "1" | "true" | "yes" | "on"))
            .or_else(|| self.voice.as_ref().and_then(|v| v.interrupt_aborts))
            .unwrap_or(false)
    }

    fn voice_prewarm(&self) -> bool {
        env::var("WRAITH_VOICE_PREWARM")
            .ok()
            .map(|v| matches!(v.as_str(), "1" | "true" | "yes" | "on"))
            .or_else(|| self.voice.as_ref().and_then(|v| v.prewarm))
            .unwrap_or(true)
    }

    fn voice_debug(&self) -> bool {
        env::var("WRAITH_VOICE_DEBUG")
            .ok()
            .map(|v| matches!(v.as_str(), "1" | "true" | "yes" | "on"))
            .or_else(|| self.voice.as_ref().and_then(|v| v.debug))
            .unwrap_or(false)
    }

    fn voice_settings(&self) -> voice::VoiceSettings {
        voice::VoiceSettings {
            silence_ms: self.voice_silence_ms(),
            min_speech_ms: self.voice_min_speech_ms(),
            speed: self.voice_speed(),
            narration: self.voice_narration(),
            interrupt_aborts: self.voice_interrupt_aborts(),
            prewarm: self.voice_prewarm(),
            debug: self.voice_debug(),
            tts_model: self.tts_model(),
        }
    }
}

fn get_config_path() -> Option<PathBuf> {
    if let Ok(path) = env::var("WRAITH_CONFIG") {
        let path = PathBuf::from(path);
        if path.exists() {
            return Some(path);
        }

        panic!(
            "WRAITH_CONFIG is set to '{}', but that file does not exist",
            path.display()
        );
    }

    if let Some(config_dir) = dirs::config_dir() {
        let config_path = config_dir.join("wraith").join("wraith.toml");
        if config_path.exists() {
            return Some(config_path);
        }
    }

    let local_config = PathBuf::from(".wraith.toml");
    if local_config.exists() {
        return Some(local_config);
    }

    None
}

fn load_config() -> Config {
    let config_path = get_config_path();

    if let Some(config_path) = config_path.as_ref() {
        match fs::read_to_string(config_path) {
            Ok(contents) => match toml::from_str(&contents) {
                Ok(config) => {
                    println!("Loaded config from: {}", config_path.display());
                    return config;
                }
                Err(e) => {
                    eprintln!(
                        "Warning: Failed to parse config file '{}': {}",
                        config_path.display(),
                        e
                    );
                }
            },
            Err(e) => {
                eprintln!(
                    "Warning: Failed to read config file '{}': {}",
                    config_path.display(),
                    e
                );
            }
        }
    }

    if config_path.is_some() {
        println!("Using default configuration (config file found but could not be read or parsed)");
    } else {
        println!("Using default configuration (no config file found)");
    }
    Config {
        server: None,
        llm: None,
        tts: None,
        stt: None,
        vad: None,
        security: None,
        opencode: None,
        voice: None,
    }
}

fn print_config_info(config: &Config) {
    println!();
    println!("╔════════════════════════════════════════════════════════╗");
    println!("║                    WRAITH CONFIG                        ║");
    println!("╚════════════════════════════════════════════════════════╝");
    println!();
    println!("  Server");
    println!("    Host:     {}", config.server_host());
    println!("    Port:     {}", config.server_port());
    println!();
    println!("  LLM");
    println!("    Model:    {}", config.llm_model());
    println!("    Endpoint: {}", config.llm_endpoint());
    println!();
    println!("  TTS");
    println!("    Model:    {}", config.tts_model());
    println!();
    println!("  STT");
    println!("    Executable: {}", config.stt_executable());
    println!();
    println!("  VAD (Voice Activity Detection)");
    println!("    Silence Threshold:     {}ms", config.vad_silence_threshold_ms());
    println!("    Volume (Speaking):     {}", config.vad_volume_threshold_speaking());
    println!("    Volume (Interrupt):    {}", config.vad_volume_threshold_interrupt());
    println!("    Min Recording:         {}ms", config.vad_min_recording_duration_ms());
    println!();
    println!("  Security");
    println!("    Cert: {}", config.cert_path());
    println!("    Key:  {}", config.key_path());
    println!("    Auto TLS: {}", config.auto_tls());
    println!();
    println!("  opencode");
    println!("    Executable: {}", config.opencode_executable());
    let directory = config.opencode_directory();
    if directory.is_empty() {
        println!("    Directory:  (current working directory)");
    } else {
        println!("    Directory:  {}", directory);
    }
    print_option("    Agent:      ", &config.opencode_agent());
    print_option("    Model:      ", &config.opencode_model());
    println!("    Auto-start: {}", config.opencode_auto_start());
    println!();
    println!("  Voice");
    println!("    Listen timeout: {}ms", config.voice_silence_ms());
    println!("    Speed:          {}", config.voice_speed());
    println!("    Narration:      {}", config.voice_narration());
    println!();
    println!("  Override with env vars: WRAITH_*");
    println!();
}

fn print_option(label: &str, value: &str) {
    if value.is_empty() {
        println!("{label}(default)");
    } else {
        println!("{label}{value}");
    }
}

fn create_app() -> Router {
    let cors = CorsLayer::permissive();
    Router::new()
        .route("/synthesize", post(synthesize))
        .route("/transcribe", post(transcribe))
        .route("/chat", post(chat))
        .route("/vad-config", get(vad_config))
        .route("/history", get(get_history_handler))
        .route("/clear-history", post(clear_history_handler))
        .route("/opencode/status", get(opencode_status))
        .route("/opencode/start", post(opencode_start))
        .route("/opencode/stop", post(opencode_stop))
        .route("/opencode/events", get(opencode_events))
        .route(
            "/opencode/sessions",
            get(opencode_sessions).post(opencode_create_session),
        )
        .route(
            "/opencode/sessions/{session_id}",
            axum::routing::delete(opencode_delete_session),
        )
        .route(
            "/opencode/sessions/{session_id}/messages",
            get(opencode_session_messages),
        )
        .route("/opencode/prompt", post(opencode_prompt))
        .route("/opencode/message", post(opencode_message))
        .route("/opencode/abort", post(opencode_abort))
        .route("/opencode/permission", post(opencode_permission))
        .route("/opencode/questions", get(opencode_questions))
        .route("/opencode/question", post(opencode_answer_question))
        .route(
            "/opencode/question/reject",
            post(opencode_reject_question),
        )
        .route("/api/tools/status", get(tools_status))
        .route("/api/tools/install", post(tools_install))
        .route("/api/tools/install/stream", get(tools_install_stream))
        .route("/voice/config", get(voice_config))
        .route("/voice/metrics", get(voice_metrics))
        .route("/ws/voice", get(voice_socket))
        .route("/opencode/workers", get(opencode_workers).post(opencode_worker_spawn))
        .route("/agent/worker", post(agent_worker))
        .route("/opencode/agent/status", get(agent_status))
        .route("/opencode/agent/install", post(agent_install))
        .route("/opencode/agent/remove", post(agent_remove))
        .route("/agent/say", post(agent_say))
        .route("/", get(home))
        .layer(cors)
}

static VOICE_HUB: OnceLock<Arc<voice::VoiceHub>> = OnceLock::new();
static BRIDGE_TOKEN: OnceLock<String> = OnceLock::new();

/// Token shared with the managed opencode process so its tools can call back.
pub(crate) fn bridge_token() -> String {
    BRIDGE_TOKEN
        .get_or_init(|| uuid::Uuid::new_v4().to_string())
        .clone()
}

fn voice_hub() -> &'static Arc<voice::VoiceHub> {
    VOICE_HUB.get_or_init(voice::VoiceHub::new)
}

const WORKER_PREFIX: &str = "Worker: ";

async fn opencode_workers() -> Result<Json<Value>, ApiError> {
    let manager = opencode_manager();
    let sessions = manager.list_sessions().await.map_err(bad_gateway)?;
    let statuses = manager
        .api_get("/session/status")
        .await
        .unwrap_or_else(|_| json!({}));

    let workers: Vec<Value> = sessions
        .as_array()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .filter(|session| {
            session
                .get("title")
                .and_then(|t| t.as_str())
                .map(|title| title.starts_with(WORKER_PREFIX))
                .unwrap_or(false)
        })
        .map(|mut session| {
            let id = session
                .get("id")
                .and_then(|i| i.as_str())
                .unwrap_or_default()
                .to_string();
            session["status"] = statuses
                .get(id.as_str())
                .cloned()
                .unwrap_or(json!({ "type": "idle" }));
            session
        })
        .collect();

    Ok(Json(json!({ "workers": workers })))
}

#[derive(Deserialize)]
struct WorkerRequest {
    prompt: String,
    title: Option<String>,
}

fn worker_session_title(payload: &WorkerRequest) -> String {
    let title = payload
        .title
        .clone()
        .filter(|t| !t.trim().is_empty())
        .unwrap_or_else(|| opencode::worker_title(&payload.prompt));
    format!("{WORKER_PREFIX}{title}")
}

async fn spawn_worker(payload: WorkerRequest) -> Result<Value, String> {
    let manager = opencode_manager();
    let session = manager
        .create_session(Some(worker_session_title(&payload)))
        .await?;
    let id = session
        .get("id")
        .and_then(|i| i.as_str())
        .ok_or_else(|| "worker session missing id".to_string())?
        .to_string();

    manager
        .prompt_async(&id, &payload.prompt, None, None)
        .await?;

    Ok(json!({ "ok": true, "worker": session }))
}

async fn opencode_worker_spawn(
    Json(payload): Json<WorkerRequest>,
) -> Result<Json<Value>, ApiError> {
    spawn_worker(payload).await.map(Json).map_err(bad_gateway)
}

async fn agent_worker(
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    headers: axum::http::HeaderMap,
    Json(payload): Json<WorkerRequest>,
) -> Result<Json<Value>, ApiError> {
    loopback(&addr)?;

    let token = headers
        .get("x-wraith-token")
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default();
    if token != bridge_token() {
        return Err(api_error(
            axum::http::StatusCode::UNAUTHORIZED,
            "invalid bridge token",
        ));
    }

    spawn_worker(payload).await.map(Json).map_err(bad_gateway)
}

async fn agent_status() -> Json<Value> {
    let directory = opencode::effective_directory(&cached_config().opencode_directory());
    Json(serde_json::to_value(agent::status(&directory)).unwrap_or_else(|_| json!({})))
}

fn loopback(addr: &SocketAddr) -> Result<(), ApiError> {
    if addr.ip().is_loopback() {
        Ok(())
    } else {
        Err(api_error(
            axum::http::StatusCode::FORBIDDEN,
            "this action is only allowed from this machine",
        ))
    }
}

async fn agent_install(
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
) -> Result<Json<Value>, ApiError> {
    loopback(&addr)?;
    let directory = opencode::effective_directory(&cached_config().opencode_directory());
    agent::install(&directory)
        .map(|status| Json(serde_json::to_value(status).unwrap_or_else(|_| json!({}))))
        .map_err(|e| api_error(axum::http::StatusCode::BAD_REQUEST, e))
}

async fn agent_remove(
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
) -> Result<Json<Value>, ApiError> {
    loopback(&addr)?;
    let directory = opencode::effective_directory(&cached_config().opencode_directory());
    agent::remove(&directory)
        .map(|status| Json(serde_json::to_value(status).unwrap_or_else(|_| json!({}))))
        .map_err(|e| api_error(axum::http::StatusCode::BAD_REQUEST, e))
}

#[derive(Deserialize)]
struct AgentSayRequest {
    text: String,
}

async fn agent_say(
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    headers: axum::http::HeaderMap,
    Json(payload): Json<AgentSayRequest>,
) -> Result<Json<Value>, ApiError> {
    loopback(&addr)?;

    let token = headers
        .get("x-wraith-token")
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default();
    if token != bridge_token() {
        return Err(api_error(
            axum::http::StatusCode::UNAUTHORIZED,
            "invalid bridge token",
        ));
    }

    match voice_hub().say(&payload.text).await {
        Ok(()) => Ok(Json(json!({ "ok": true }))),
        Err(e) => Err(api_error(axum::http::StatusCode::SERVICE_UNAVAILABLE, e)),
    }
}

async fn voice_socket(ws: WebSocketUpgrade) -> impl IntoResponse {
    let settings = cached_config().voice_settings();
    let manager = Arc::clone(opencode_manager());
    ws.on_upgrade(move |socket| voice::run_voice_socket(socket, settings, manager, Arc::clone(voice_hub())))
}

async fn voice_config() -> Json<Value> {
    let settings = cached_config().voice_settings();
    let sample_rate = speech::sample_rate_from_model(&settings.tts_model)
        .unwrap_or(speech::DEFAULT_SAMPLE_RATE);
    let piper_available = opencode::find_executable("piper-tts").is_some();

    Json(json!({
        "silence_ms": settings.silence_ms,
        "min_speech_ms": settings.min_speech_ms,
        "speed": settings.speed,
        "narration": settings.narration,
        "interrupt_aborts": settings.interrupt_aborts,
        "prewarm": settings.prewarm,
        "debug": settings.debug,
        "sample_rate": sample_rate,
        "piper_available": piper_available,
    }))
}

async fn voice_metrics() -> Json<Value> {
    Json(voice_hub().metrics().await)
}

static OPENCODE_MANAGER: OnceLock<Arc<opencode::OpencodeManager>> = OnceLock::new();
static TOOL_STATE: OnceLock<Arc<tools::ToolState>> = OnceLock::new();

fn opencode_manager() -> &'static Arc<opencode::OpencodeManager> {
    OPENCODE_MANAGER.get_or_init(|| {
        opencode::OpencodeManager::new(cached_config().opencode_settings())
    })
}

fn tool_state() -> &'static Arc<tools::ToolState> {
    TOOL_STATE.get_or_init(tools::ToolState::new)
}

type ApiError = (axum::http::StatusCode, Json<Value>);

fn api_error(status: axum::http::StatusCode, message: impl Into<String>) -> ApiError {
    (status, Json(json!({ "error": message.into() })))
}

fn bad_gateway(message: impl Into<String>) -> ApiError {
    api_error(axum::http::StatusCode::BAD_GATEWAY, message)
}

fn json_stream(
    receiver: broadcast::Receiver<Value>,
) -> impl Stream<Item = Result<SseEvent, Infallible>> {
    stream::unfold(receiver, |mut receiver| async move {
        loop {
            match receiver.recv().await {
                Ok(value) => {
                    let data = serde_json::to_string(&value).unwrap_or_default();
                    return Some((Ok(SseEvent::default().data(data)), receiver));
                }
                Err(broadcast::error::RecvError::Lagged(_)) => continue,
                Err(broadcast::error::RecvError::Closed) => return None,
            }
        }
    })
}

fn line_stream(
    receiver: broadcast::Receiver<String>,
) -> impl Stream<Item = Result<SseEvent, Infallible>> {
    stream::unfold(receiver, |mut receiver| async move {
        loop {
            match receiver.recv().await {
                Ok(line) => {
                    return Some((Ok(SseEvent::default().data(line)), receiver));
                }
                Err(broadcast::error::RecvError::Lagged(_)) => continue,
                Err(broadcast::error::RecvError::Closed) => return None,
            }
        }
    })
}

async fn opencode_status() -> Json<opencode::OpencodeStatus> {
    Json(opencode_manager().status().await)
}

async fn opencode_start() -> Result<Json<Value>, ApiError> {
    match opencode_manager().ensure_running().await {
        Ok(base_url) => Ok(Json(json!({ "ok": true, "base_url": base_url }))),
        Err(e) => Err(bad_gateway(e)),
    }
}

async fn opencode_stop() -> Json<Value> {
    opencode_manager().stop().await;
    Json(json!({ "ok": true }))
}

async fn opencode_events() -> Sse<impl Stream<Item = Result<SseEvent, Infallible>>> {
    Sse::new(json_stream(opencode_manager().subscribe()))
        .keep_alive(axum::response::sse::KeepAlive::default())
}

async fn opencode_sessions() -> Result<Json<Value>, ApiError> {
    opencode_manager().list_sessions().await.map(Json).map_err(bad_gateway)
}

async fn opencode_create_session(
    Json(payload): Json<CreateSessionRequest>,
) -> Result<Json<Value>, ApiError> {
    opencode_manager()
        .create_session(payload.title)
        .await
        .map(Json)
        .map_err(bad_gateway)
}

async fn opencode_delete_session(
    axum::extract::Path(session_id): axum::extract::Path<String>,
) -> Result<Json<Value>, ApiError> {
    opencode_manager()
        .delete_session(&session_id)
        .await
        .map(Json)
        .map_err(bad_gateway)
}

async fn opencode_session_messages(
    axum::extract::Path(session_id): axum::extract::Path<String>,
) -> Result<Json<Value>, ApiError> {
    opencode_manager()
        .list_messages(&session_id)
        .await
        .map(Json)
        .map_err(bad_gateway)
}

async fn opencode_prompt(
    Json(payload): Json<OpencodePromptRequest>,
) -> Result<Json<Value>, ApiError> {
    opencode_manager()
        .prompt_async(
            &payload.session_id,
            &payload.text,
            payload.agent.as_deref(),
            payload.model.as_deref(),
        )
        .await
        .map(|_| Json(json!({ "ok": true })))
        .map_err(bad_gateway)
}

async fn opencode_message(
    Json(payload): Json<OpencodePromptRequest>,
) -> Result<Json<Value>, ApiError> {
    opencode_manager()
        .prompt_blocking(
            &payload.session_id,
            &payload.text,
            payload.agent.as_deref(),
            payload.model.as_deref(),
        )
        .await
        .map(|text| Json(json!({ "text": text })))
        .map_err(bad_gateway)
}

async fn opencode_abort(
    Json(payload): Json<SessionRequest>,
) -> Result<Json<Value>, ApiError> {
    opencode_manager()
        .abort(&payload.session_id)
        .await
        .map(Json)
        .map_err(bad_gateway)
}

async fn opencode_permission(
    Json(payload): Json<PermissionReplyRequest>,
) -> Result<Json<Value>, ApiError> {
    opencode_manager()
        .reply_permission(
            &payload.session_id,
            &payload.permission_id,
            &payload.response,
            payload.remember,
        )
        .await
        .map(Json)
        .map_err(bad_gateway)
}

async fn opencode_questions() -> Result<Json<Value>, ApiError> {
    opencode_manager().list_questions().await.map(Json).map_err(bad_gateway)
}

async fn opencode_answer_question(
    Json(payload): Json<QuestionReplyRequest>,
) -> Result<Json<Value>, ApiError> {
    opencode_manager()
        .reply_question(&payload.request_id, payload.answers)
        .await
        .map(Json)
        .map_err(bad_gateway)
}

async fn opencode_reject_question(
    Json(payload): Json<QuestionRejectRequest>,
) -> Result<Json<Value>, ApiError> {
    opencode_manager()
        .reject_question(&payload.request_id)
        .await
        .map(Json)
        .map_err(bad_gateway)
}

async fn tools_status() -> Json<Value> {
    let stt = cached_config().stt_executable();
    let mut status = tool_state().status(&stt).await;
    if let Some(object) = status.as_object_mut() {
        object.insert("install".to_string(), tool_state().install_status());
    }
    Json(status)
}

async fn tools_install(
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    Json(payload): Json<InstallRequest>,
) -> Result<Json<Value>, ApiError> {
    if !addr.ip().is_loopback() {
        return Err(api_error(
            axum::http::StatusCode::FORBIDDEN,
            "installing tools is only allowed from this machine",
        ));
    }

    match tool_state().start_install(&payload.tool, &payload.method) {
        Ok(command) => Ok(Json(json!({ "ok": true, "command": command }))),
        Err(e) => Err(api_error(axum::http::StatusCode::BAD_REQUEST, e)),
    }
}

async fn tools_install_stream() -> Sse<impl Stream<Item = Result<SseEvent, Infallible>>> {
    Sse::new(line_stream(tool_state().subscribe()))
        .keep_alive(axum::response::sse::KeepAlive::default())
}

#[derive(Deserialize)]
struct CreateSessionRequest {
    title: Option<String>,
}

#[derive(Deserialize)]
struct OpencodePromptRequest {
    session_id: String,
    text: String,
    agent: Option<String>,
    model: Option<String>,
}

#[derive(Deserialize)]
struct SessionRequest {
    session_id: String,
}

#[derive(Deserialize)]
struct PermissionReplyRequest {
    session_id: String,
    permission_id: String,
    response: String,
    #[serde(default)]
    remember: bool,
}

#[derive(Deserialize)]
struct QuestionReplyRequest {
    request_id: String,
    answers: Vec<Vec<String>>,
}

#[derive(Deserialize)]
struct QuestionRejectRequest {
    request_id: String,
}

#[derive(Deserialize)]
struct InstallRequest {
    tool: String,
    method: String,
}

async fn get_history_handler() -> axum::Json<Vec<ChatMessage>> {
    if let Ok(history) = get_chat_history().lock() {
        axum::Json(history.clone())
    } else {
        axum::Json(vec![])
    }
}

async fn serve_https(addr: SocketAddr, tls_config: RustlsConfig, app: Router) {
    println!("HTTPS server launched. Listening on https://{addr}");
    let service = app.into_make_service_with_connect_info::<SocketAddr>();
    axum_server::bind_rustls(addr, tls_config)
        .serve(service)
        .await
        .unwrap();
}

#[tokio::main]
async fn main() {
    let config = load_config();
    print_config_info(&config);
    validate_external_tools(&config);

    let saved_history = load_history_from_disk();
    if let Ok(mut history) = get_chat_history().lock() {
        *history = saved_history;
    }

    watch_for_shutdown();
    opencode_manager().start_event_pump();

    if config.opencode_auto_start() && opencode::find_executable(&config.opencode_executable()).is_some() {
        let manager = Arc::clone(opencode_manager());
        tokio::spawn(async move {
            match manager.ensure_running().await {
                Ok(base_url) => println!("[opencode] auto-started at {base_url}"),
                Err(e) => eprintln!("[opencode] auto-start failed: {e}"),
            }
        });
    }

    let app = create_app();

    let host = config.server_host();
    let port = config.server_port();
    let https_addr: SocketAddr = match format!("{host}:{port}").parse() {
        Ok(addr) => addr,
        Err(e) => {
            eprintln!(
                "Invalid server bind address '{}:{}': {}",
                host, port, e
            );
            eprintln!(
                "Please configure a valid IP address for the host and a valid port number."
            );
            std::process::exit(1);
        }
    };

    let (cert_path, key_path) =
        match tls::ensure_tls(&config.cert_path(), &config.key_path(), config.auto_tls()) {
            Ok(paths) => paths,
            Err(message) => {
                eprintln!("{message}");
                std::process::exit(1);
            }
        };

    {
        println!("Found TLS certificates, starting HTTPS server...");

        let tls_config = match RustlsConfig::from_pem_file(&cert_path, &key_path).await {
            Ok(config) => config,
            Err(e) => {
                eprintln!("Failed to load TLS certificates: {e}");
                eprintln!("Place {} and {} in the project directory.", cert_path, key_path);
                std::process::exit(1);
            }
        };

        println!();
        println!("╔════════════════════════════════════════════════════════╗");
        println!("║              WRAITH is running!                        ║");
        println!("╠════════════════════════════════════════════════════════╣");
        println!("║                                                        ║");
        let port = config.server_port();
        println!("║   HTTPS: https://127.0.0.1:{}                           ║", port);
        println!("║   HTTPS: https://YOUR_IP:{} (for mobile)             ║", port);
        println!("║                                                        ║");
        println!("║   Note: Accept the self-signed certificate warning.   ║");
        println!("║                                                        ║");
        println!("╚════════════════════════════════════════════════════════╝");
        println!();

        serve_https(https_addr, tls_config, app).await;
    }
}

static CONFIG: OnceLock<Config> = OnceLock::new();

fn cached_config() -> &'static Config {
    CONFIG.get_or_init(load_config)
}

fn validate_external_tools(config: &Config) {
    println!();
    println!("Validating external tools...");

    let piper_path = which("piper-tts");
    if piper_path.is_none() {
        println!("  [!] WARNING: piper-tts not found in PATH");
        println!("      TTS will fail. Install piper: https://github.com/rhasspy/piper");
    } else {
        println!("  [✓] piper-tts: {}", piper_path.unwrap().display());
    }

    let stt_executable = config.stt_executable();
    let stt_path = which(&stt_executable);
    if stt_path.is_none() {
        println!("  [!] WARNING: {} not found in PATH", stt_executable);
        println!("      STT will fail. Install voxtype: https://github.com/taylor-vann/voxtype");
    } else {
        println!("  [✓] {}: {}", stt_executable, stt_path.unwrap().display());
    }

    let ffmpeg_path = which("ffmpeg");
    if ffmpeg_path.is_none() {
        println!("  [!] WARNING: ffmpeg not found in PATH");
        println!("      Audio conversion will fail. Install ffmpeg.");
    } else {
        println!("  [✓] ffmpeg: {}", ffmpeg_path.unwrap().display());
    }

    println!();
}

fn watch_for_shutdown() {
    tokio::spawn(async {
        #[cfg(unix)]
        {
            let mut terminate = tokio::signal::unix::signal(
                tokio::signal::unix::SignalKind::terminate(),
            )
            .ok();

            let terminated = async {
                match terminate.as_mut() {
                    Some(signal) => {
                        signal.recv().await;
                    }
                    None => std::future::pending::<()>().await,
                }
            };

            tokio::select! {
                _ = tokio::signal::ctrl_c() => {}
                _ = terminated => {}
            }
        }

        #[cfg(not(unix))]
        {
            let _ = tokio::signal::ctrl_c().await;
        }

        println!();
        println!("[wraith] shutting down");
        opencode_manager().stop().await;
        std::process::exit(0);
    });
}

pub(crate) fn which(program: &str) -> Option<PathBuf> {
    std::env::var_os("PATH").and_then(|paths| {
        for path in std::env::split_paths(&paths) {
            let full_path = path.join(program);
            if full_path.exists() {
                return Some(full_path);
            }
        }
        None
    })
}

fn run_with_retry<F, R>(name: &str, mut f: F) -> Result<R, String>
where
    F: FnMut() -> Result<R, String>,
{
    let mut last_error = String::new();

    for attempt in 0..=MAX_RETRIES {
        match f() {
            Ok(result) => return Ok(result),
            Err(e) => {
                last_error = e;
                if attempt < MAX_RETRIES {
                    let delay = RETRY_DELAY_MS * (2_u64.pow(attempt));
                    eprintln!("[{}] Attempt {} failed, retrying in {}ms...", name, attempt + 1, delay);
                    std::thread::sleep(Duration::from_millis(delay));
                }
            }
        }
    }

    Err(last_error)
}

async fn synthesize(Form(payload): Form<SynthesizeRequest>) -> Html<String> {
    let config = cached_config();

    if payload.text.trim().is_empty() {
        return Html(String::new());
    }

    let tts_model = config.tts_model();
    let clean_text = {
        let mut text = payload.text.clone();

        let re_code = regex::Regex::new(r"```[\s\S]*?```").unwrap();
        text = re_code.replace_all(&text, "").to_string();

        let re_inline_code = regex::Regex::new(r"`[^`]*`").unwrap();
        text = re_inline_code.replace_all(&text, "").to_string();

        let re_bold_italic = regex::Regex::new(r"(\*\*|__|\*|_)").unwrap();
        text = re_bold_italic.replace_all(&text, "").to_string();

        let re_header = regex::Regex::new(r"(?m)^#+\s*").unwrap();
        text = re_header.replace_all(&text, "").to_string();

        text.replace("\n", " ")
    };

    let result = run_with_retry("piper-tts", || {
        let mut child = Command::new("piper-tts")
            .arg("--model")
            .arg(&tts_model)
            .arg("--output_file")
            .arg("-")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .map_err(|e| format!("Failed to spawn piper-tts: {}", e))?;

        if let Some(mut stdin) = child.stdin.take() {
            stdin
                .write_all(clean_text.as_bytes())
                .map_err(|e| format!("Failed to write to piper-tts stdin: {}", e))?;
        }

        let output = child
            .wait_with_output()
            .map_err(|e| format!("Failed to read piper-tts output: {}", e))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(format!(
                "piper-tts exited with status {}: {}",
                output.status, stderr
            ));
        }

        Ok(output.stdout)
    });

    match result {
        Ok(output) => {
            let b64 = BASE64_STANDARD.encode(&output);
            let html = format!(
                r#"<audio controls autoplay style="width:100%; height: 40px;" src="data:audio/wav;base64,{}"></audio>"#,
                b64
            );
            Html(html)
        }
        Err(e) => {
            eprintln!("[ERROR] TTS failed after {} retries: {}", MAX_RETRIES + 1, e);
            Html(String::from("ERROR:TTS_FAILED"))
        }
    }
}

async fn transcribe(Form(payload): Form<TranscribeRequest>) -> Html<String> {
    let audio_data = match BASE64_STANDARD.decode(&payload.audio_base64) {
        Ok(data) => data,
        Err(e) => {
            eprintln!("[ERROR] Failed to decode base64 audio: {}", e);
            return Html(String::from("ERROR:DECODE_FAILED"));
        }
    };

    match stt::transcribe_audio_bytes(&audio_data) {
        Ok(text) => Html(text),
        Err(code) => {
            let marker = code.split(':').next().unwrap_or("STT_FAILED");
            eprintln!("[ERROR] transcription failed: {code}");
            Html(format!("ERROR:{marker}"))
        }
    }
}

pub(crate) async fn local_chat(text: &str) -> Result<String, String> {
    if text.trim().is_empty() {
        return Ok(String::new());
    }

    let config = cached_config();
    let client = reqwest::Client::new();

    let history = get_chat_history()
        .lock()
        .map(|h| h.clone())
        .unwrap_or_default();

    let mut messages = vec![
        json!({
            "role": "system",
            "content": "You are a concise, highly efficient, and direct AI
                assistant, inspired by sleek futuristic interfaces like
                Grok and SpaceX. Respond with crisp, accurate information
                without run-on sentences or unnecessary filler. note: your
                response is going to be read outloud by a text to speech model,
                so no emojis or markdown respond in a way that a text to speech model can read."
        })
    ];

    for msg in history.iter().take(10) {
        messages.push(json!({
            "role": msg.role,
            "content": msg.content
        }));
    }

    messages.push(json!({
        "role": "user",
        "content": text
    }));

    let request_body = json!({
        "model": config.llm_model(),
        "messages": messages,
        "temperature": 0.7,
        "max_tokens": -1,
        "stream": false
    });

    let endpoint = config.llm_endpoint();
    let response = client
        .post(&endpoint)
        .json(&request_body)
        .send()
        .await
        .map_err(|e| {
            eprintln!("Reqwest error calling LLM: {e}");
            "Error connecting to AI Server.".to_string()
        })?;

    if !response.status().is_success() {
        return Err("Error: Failed to parse AI response.".to_string());
    }

    let parsed = response
        .json::<LmStudioResponse>()
        .await
        .map_err(|_| "Error: Failed to parse AI response.".to_string())?;

    let content = parsed
        .choices
        .first()
        .map(|choice| choice.message.content.clone())
        .ok_or_else(|| "Error: Failed to parse AI response.".to_string())?;

    if let Ok(mut history) = get_chat_history().lock() {
        history.push(ChatMessage {
            role: "user".to_string(),
            content: text.to_string(),
        });
        history.push(ChatMessage {
            role: "assistant".to_string(),
            content: content.clone(),
        });
        if history.len() > 20 {
            history.drain(0..4);
        }
        save_history_to_disk(&history);
    }

    Ok(content)
}

async fn chat(Form(payload): Form<ChatRequest>) -> Html<String> {
    if payload.text.trim().is_empty() {
        return Html(String::new());
    }

    match local_chat(&payload.text).await {
        Ok(content) => Html(content),
        Err(message) => Html(message),
    }
}

async fn vad_config() -> axum::Json<serde_json::Value> {
    let config = cached_config();
    axum::Json(serde_json::json!({
        "silence_threshold_ms": config.vad_silence_threshold_ms(),
        "volume_threshold_speaking": config.vad_volume_threshold_speaking(),
        "volume_threshold_interrupt": config.vad_volume_threshold_interrupt(),
        "min_recording_duration_ms": config.vad_min_recording_duration_ms()
    }))
}

async fn home() -> Html<String> {
    Html(String::from(include_str!("index.html")))
}

#[derive(Deserialize)]
struct SynthesizeRequest {
    text: String,
}

#[derive(Deserialize)]
struct TranscribeRequest {
    audio_base64: String,
}

#[derive(Deserialize)]
struct ChatRequest {
    text: String,
}

#[derive(serde::Serialize, serde::Deserialize, Clone, Debug)]
struct ChatMessage {
    role: String,
    content: String,
}

static CHAT_HISTORY: OnceLock<std::sync::Mutex<Vec<ChatMessage>>> = OnceLock::new();

fn get_chat_history() -> &'static std::sync::Mutex<Vec<ChatMessage>> {
    CHAT_HISTORY.get_or_init(|| std::sync::Mutex::new(Vec::new()))
}

fn get_history_path() -> Option<PathBuf> {
    dirs::config_dir().map(|d| d.join("wraith").join("history.json"))
}

fn load_history_from_disk() -> Vec<ChatMessage> {
    if let Some(path) = get_history_path() {
        if let Some(parent) = path.parent() {
            let _ = fs::create_dir_all(parent);
        }
        if let Ok(data) = fs::read_to_string(&path) {
            if let Ok(history) = serde_json::from_str::<Vec<ChatMessage>>(&data) {
                println!("[Wraith] Loaded {} messages from history", history.len());
                return history;
            }
        }
    }
    Vec::new()
}

fn save_history_to_disk(history: &[ChatMessage]) {
    if let Some(path) = get_history_path() {
        if let Some(parent) = path.parent() {
            let _ = fs::create_dir_all(parent);
        }
        if let Ok(data) = serde_json::to_string(history) {
            let _ = fs::write(&path, data);
        }
    }
}

async fn clear_history_handler() -> Html<String> {
    if let Ok(mut history) = get_chat_history().lock() {
        history.clear();
        save_history_to_disk(&[]);
    }
    Html(String::from("History cleared"))
}

#[derive(Deserialize, Debug)]
struct LmStudioResponse {
    choices: Vec<LmStudioChoice>,
}

#[derive(Deserialize, Debug)]
struct LmStudioChoice {
    message: LmStudioMessage,
}

#[derive(Deserialize, Debug)]
struct LmStudioMessage {
    content: String,
}
