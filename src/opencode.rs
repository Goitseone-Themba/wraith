//! opencode integration: managed `opencode serve` child process, HTTP adapter,
//! and the SSE event stream used to drive the browser UI.

use std::path::PathBuf;
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use serde::Serialize;
use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::{broadcast, Mutex};

pub const EVENT_CHANNEL_CAPACITY: usize = 1024;
const SERVER_START_TIMEOUT: Duration = Duration::from_secs(20);
const HEALTH_TIMEOUT: Duration = Duration::from_secs(3);
const MESSAGE_TIMEOUT: Duration = Duration::from_secs(600);

#[derive(Debug, Clone)]
pub struct OpencodeSettings {
    pub executable: String,
    pub directory: String,
    pub agent: String,
    pub model: String,
    pub port: u16,
    pub extra_args: Vec<String>,
    pub bridge_url: String,
    pub bridge_token: String,
}

struct Running {
    child: Option<Child>,
    base_url: String,
    password: String,
    question_api: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct OpencodeStatus {
    pub enabled: bool,
    pub installed: bool,
    pub executable: String,
    pub version: Option<String>,
    pub running: bool,
    pub healthy: bool,
    pub base_url: Option<String>,
    pub directory: String,
    pub question_api: bool,
    pub error: Option<String>,
}

pub struct OpencodeManager {
    pub settings: OpencodeSettings,
    http: reqwest::Client,
    process: Mutex<Option<Running>>,
    events: broadcast::Sender<Value>,
    version: Mutex<Option<String>>,
    pump_started: AtomicBool,
}

impl OpencodeManager {
    pub fn new(settings: OpencodeSettings) -> Arc<Self> {
        let (events, _) = broadcast::channel(EVENT_CHANNEL_CAPACITY);
        Arc::new(Self {
            settings,
            http: reqwest::Client::new(),
            process: Mutex::new(None),
            events,
            version: Mutex::new(None),
            pump_started: AtomicBool::new(false),
        })
    }

    pub fn subscribe(&self) -> broadcast::Receiver<Value> {
        self.events.subscribe()
    }

    /// Start (or reuse) the managed opencode server.
    pub async fn ensure_running(&self) -> Result<String, String> {
        let mut guard = self.process.lock().await;

        if let Some(running) = guard.as_mut() {
            let child_state = match running.child.as_mut() {
                Some(child) => match child.try_wait() {
                    Ok(None) => Ok(()),
                    Ok(Some(status)) => {
                        eprintln!("[opencode] managed server exited: {status}");
                        Err(())
                    }
                    Err(e) => {
                        eprintln!("[opencode] failed to check managed server: {e}");
                        Err(())
                    }
                },
                None => Ok(()),
            };

            if child_state.is_ok() && self.health(&running.base_url, &running.password).await {
                return Ok(running.base_url.clone());
            }

            if child_state.is_ok() {
                eprintln!("[opencode] managed server unhealthy, restarting");
            }
            *guard = None;
        }

        let (child, base_url, password) = self.spawn_process().await?;
        let question_api = self.detect_question_api(&base_url, &password).await;
        *guard = Some(Running {
            child: Some(child),
            base_url: base_url.clone(),
            password,
            question_api,
        });

        Ok(base_url)
    }

    /// Start the background event pump (idempotent). Requires the `Arc`.
    pub fn start_event_pump(self: &Arc<Self>) {
        self.ensure_pump();
    }

    fn ensure_pump(self: &Arc<Self>) {
        if self.pump_started.swap(true, Ordering::SeqCst) {
            return;
        }
        let manager = Arc::clone(self);
        tokio::spawn(async move {
            manager.event_pump().await;
        });
    }

    async fn spawn_process(&self) -> Result<(Child, String, String), String> {
        let executable = find_executable(&self.settings.executable)
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| self.settings.executable.clone());
        let directory = {
            let configured = self.settings.directory.trim();
            if configured.is_empty() {
                std::env::current_dir()
                    .map(|p| p.display().to_string())
                    .unwrap_or_else(|_| ".".to_string())
            } else {
                configured.to_string()
            }
        };

        let password = uuid::Uuid::new_v4().to_string();
        let port = if self.settings.port == 0 {
            random_port()
        } else {
            self.settings.port
        };

        println!(
            "[opencode] starting `{} serve` in {} (port {})",
            executable, directory, port
        );

        let mut command = Command::new(&executable);
        command
            .arg("serve")
            .arg("--hostname")
            .arg("127.0.0.1")
            .arg("--port")
            .arg(port.to_string())
            .current_dir(&directory)
            .env("OPENCODE_SERVER_PASSWORD", &password)
            .env("WRAITH_BRIDGE_URL", &self.settings.bridge_url)
            .env("WRAITH_BRIDGE_TOKEN", &self.settings.bridge_token)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);

        #[cfg(target_os = "linux")]
        unsafe {
            // Kill the managed server if Wraith dies abruptly.
            command.pre_exec(|| {
                libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGTERM);
                Ok(())
            });
        }

        for arg in &self.settings.extra_args {
            command.arg(arg);
        }

        let mut child = command
            .spawn()
            .map_err(|e| format!("failed to start opencode: {e}"))?;

        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| "failed to capture opencode stdout".to_string())?;
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| "failed to capture opencode stderr".to_string())?;

        tokio::spawn(async move {
            let reader = BufReader::new(stderr);
            let mut lines = reader.lines();
            while let Ok(Some(line)) = lines.next_line().await {
                eprintln!("[opencode] {line}");
            }
        });

        let parsed = tokio::time::timeout(SERVER_START_TIMEOUT, async move {
            let reader = BufReader::new(stdout);
            let mut lines = reader.lines();
            while let Some(line) = lines.next_line().await.map_err(|e| e.to_string())? {
                if let Some(url) = parse_server_url(&line) {
                    tokio::spawn(async move {
                        while let Ok(Some(line)) = lines.next_line().await {
                            println!("[opencode] {line}");
                        }
                    });
                    return Ok::<String, String>(url);
                }
            }
            Err("opencode exited before reporting a server URL".to_string())
        })
        .await;

        let base_url = match parsed {
            Ok(Ok(url)) => url,
            Ok(Err(e)) => {
                let _ = child.kill().await;
                return Err(e);
            }
            Err(_) => {
                let _ = child.kill().await;
                if port != 0 {
                    let fallback = format!("http://127.0.0.1:{port}");
                    eprintln!("[opencode] no URL parsed, falling back to {fallback}");
                    fallback
                } else {
                    return Err("timed out waiting for opencode server URL".to_string());
                }
            }
        };

        let mut healthy = false;
        for _ in 0..20 {
            if self.health(&base_url, &password).await {
                healthy = true;
                break;
            }
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
        if !healthy {
            let _ = child.kill().await;
            return Err(format!(
                "opencode server at {base_url} failed health checks"
            ));
        }

        println!("[opencode] server ready at {base_url}");
        Ok((child, base_url, password))
    }

    async fn health(&self, base_url: &str, password: &str) -> bool {
        match self
            .http
            .get(format!("{base_url}/global/health"))
            .basic_auth("opencode", Some(password))
            .timeout(HEALTH_TIMEOUT)
            .send()
            .await
        {
            Ok(res) => res.status().is_success(),
            Err(_) => false,
        }
    }

    async fn detect_question_api(&self, base_url: &str, password: &str) -> bool {
        match self
            .http
            .get(format!("{base_url}/question"))
            .basic_auth("opencode", Some(password))
            .timeout(HEALTH_TIMEOUT)
            .send()
            .await
        {
            Ok(res) => res.status().is_success(),
            Err(_) => false,
        }
    }

    async fn current_connection(&self) -> Option<(String, String)> {
        let guard = self.process.lock().await;
        guard
            .as_ref()
            .map(|r| (r.base_url.clone(), r.password.clone()))
    }

    async fn event_pump(self: Arc<Self>) {
        loop {
            if let Some((base_url, password)) = self.current_connection().await {
                if let Err(e) = self.stream_events(&base_url, &password).await {
                    eprintln!("[opencode] event stream ended: {e}");
                }
            }
            tokio::time::sleep(Duration::from_millis(1000)).await;
        }
    }

    async fn stream_events(&self, base_url: &str, password: &str) -> Result<(), String> {
        use futures_util::StreamExt;

        let response = self
            .http
            .get(format!("{base_url}/event"))
            .basic_auth("opencode", Some(password))
            .send()
            .await
            .map_err(|e| format!("event request failed: {e}"))?;

        if !response.status().is_success() {
            return Err(format!("event stream returned {}", response.status()));
        }

        let mut stream = response.bytes_stream();
        let mut parser = SseParser::new();

        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|e| format!("event stream error: {e}"))?;
            let text = String::from_utf8_lossy(&chunk);
            for payload in parser.push(&text) {
                if let Ok(mut value) = serde_json::from_str::<Value>(&payload) {
                    if let Some(inner) = value.get("payload").cloned() {
                        value = inner;
                    }
                    let _ = self.events.send(value);
                }
            }
        }

        Err("stream closed".to_string())
    }

    pub async fn stop(&self) {
        let mut guard = self.process.lock().await;
        if let Some(mut running) = guard.take() {
            if let Some(child) = running.child.as_mut() {
                let _ = child.kill().await;
                let _ = child.wait().await;
            }
            println!("[opencode] managed server stopped");
        }
    }

    #[cfg(test)]
    pub(crate) async fn set_connection_for_test(&self, base_url: String, password: String) {
        let mut guard = self.process.lock().await;
        *guard = Some(Running {
            child: None,
            base_url,
            password,
            question_api: true,
        });
    }

    pub async fn status(&self) -> OpencodeStatus {
        let executable = self.settings.executable.clone();
        let path = find_executable(&executable);

        let version = match path {
            Some(_) => Some(self.version().await),
            None => None,
        };

        let (running, healthy, base_url, question_api) = {
            let mut guard = self.process.lock().await;
            match guard.as_mut() {
                Some(r) => {
                    let alive = match r.child.as_mut() {
                        Some(child) => matches!(child.try_wait(), Ok(None)),
                        None => true,
                    };
                    let health = if alive {
                        self.health(&r.base_url, &r.password).await
                    } else {
                        false
                    };
                    (alive, health, Some(r.base_url.clone()), r.question_api)
                }
                None => (false, false, None, false),
            }
        };

        OpencodeStatus {
            enabled: true,
            installed: path.is_some(),
            executable,
            version,
            running,
            healthy,
            base_url,
            directory: effective_directory(&self.settings.directory),
            question_api,
            error: None,
        }
    }

    pub async fn version(&self) -> String {
        {
            let guard = self.version.lock().await;
            if let Some(version) = guard.as_ref() {
                return version.clone();
            }
        }

        let output = tokio::time::timeout(
            Duration::from_secs(5),
            Command::new(&self.settings.executable)
                .arg("--version")
                .stdin(Stdio::null())
                .output(),
        )
        .await;

        let version = match output {
            Ok(Ok(out)) if out.status.success() => {
                String::from_utf8_lossy(&out.stdout).trim().to_string()
            }
            _ => "unknown".to_string(),
        };

        *self.version.lock().await = Some(version.clone());
        version
    }

    // ------------------------------------------------------------------
    // HTTP adapter
    // ------------------------------------------------------------------

    pub async fn api_get(&self, path: &str) -> Result<Value, String> {
        let (base_url, password) = self.connection_info().await?;
        let response = self
            .http
            .get(format!("{base_url}{path}"))
            .basic_auth("opencode", Some(&password))
            .timeout(HEALTH_TIMEOUT)
            .send()
            .await
            .map_err(|e| format!("GET {path} failed: {e}"))?;

        json_response(response, path).await
    }

    pub async fn api_post(&self, path: &str, body: Value) -> Result<Value, String> {
        self.api_post_with_timeout(path, body, HEALTH_TIMEOUT).await
    }

    pub async fn api_post_with_timeout(
        &self,
        path: &str,
        body: Value,
        timeout: Duration,
    ) -> Result<Value, String> {
        let (base_url, password) = self.connection_info().await?;
        let response = self
            .http
            .post(format!("{base_url}{path}"))
            .basic_auth("opencode", Some(&password))
            .json(&body)
            .timeout(timeout)
            .send()
            .await
            .map_err(|e| format!("POST {path} failed: {e}"))?;

        json_response(response, path).await
    }

    async fn connection_info(&self) -> Result<(String, String), String> {
        self.ensure_running().await?;
        let guard = self.process.lock().await;
        let running = guard
            .as_ref()
            .ok_or_else(|| "opencode server not running".to_string())?;
        Ok((running.base_url.clone(), running.password.clone()))
    }

    pub async fn list_sessions(&self) -> Result<Value, String> {
        self.api_get("/session").await
    }

    pub async fn create_session(&self, title: Option<String>) -> Result<Value, String> {
        let body = match title {
            Some(title) if !title.trim().is_empty() => json!({ "title": title }),
            _ => json!({}),
        };
        self.api_post("/session", body).await
    }

    pub async fn delete_session(&self, session_id: &str) -> Result<Value, String> {
        let (base_url, password) = self.connection_info().await?;
        let path = format!("/session/{session_id}");
        let response = self
            .http
            .delete(format!("{base_url}{path}"))
            .basic_auth("opencode", Some(&password))
            .timeout(HEALTH_TIMEOUT)
            .send()
            .await
            .map_err(|e| format!("DELETE {path} failed: {e}"))?;

        json_response(response, &path).await
    }

    pub async fn list_messages(&self, session_id: &str) -> Result<Value, String> {
        self.api_get(&format!("/session/{session_id}/message"))
            .await
    }

    pub async fn prompt_async(
        &self,
        session_id: &str,
        text: &str,
        agent: Option<&str>,
        model: Option<&str>,
    ) -> Result<(), String> {
        self.prompt_async_full(session_id, text, agent, model, None)
            .await
    }

    pub async fn prompt_async_full(
        &self,
        session_id: &str,
        text: &str,
        agent: Option<&str>,
        model: Option<&str>,
        system: Option<&str>,
    ) -> Result<(), String> {
        let mut body = self.prompt_body(text, agent, model);
        if let Some(system) = system.filter(|value| !value.trim().is_empty()) {
            body["system"] = json!(system);
        }
        self.api_post(&format!("/session/{session_id}/prompt_async"), body)
            .await
            .map(|_| ())
    }

    pub async fn prompt_blocking(
        &self,
        session_id: &str,
        text: &str,
        agent: Option<&str>,
        model: Option<&str>,
    ) -> Result<String, String> {
        let body = self.prompt_body(text, agent, model);
        let response = self
            .api_post_with_timeout(
                &format!("/session/{session_id}/message"),
                body,
                MESSAGE_TIMEOUT,
            )
            .await?;
        Ok(extract_text_parts(&response))
    }

    fn prompt_body(&self, text: &str, agent: Option<&str>, model: Option<&str>) -> Value {
        let agent = agent
            .filter(|a| !a.trim().is_empty())
            .or_else(|| Some(self.settings.agent.as_str()).filter(|a| !a.trim().is_empty()));

        let model = model
            .filter(|m| !m.trim().is_empty())
            .or_else(|| Some(self.settings.model.as_str()).filter(|m| !m.trim().is_empty()));

        build_prompt_body(text, agent, model)
    }

    pub async fn abort(&self, session_id: &str) -> Result<Value, String> {
        self.api_post(&format!("/session/{session_id}/abort"), json!({}))
            .await
    }

    pub async fn reply_permission(
        &self,
        session_id: &str,
        permission_id: &str,
        response: &str,
        remember: bool,
    ) -> Result<Value, String> {
        let path = format!("/session/{session_id}/permissions/{permission_id}");
        let body = json!({ "response": response, "remember": remember });
        match self.api_post(&path, body.clone()).await {
            Ok(value) => Ok(value),
            Err(first_error) => {
                // Newer servers expose /permission/:id/reply with a `reply` field.
                let fallback = format!("/permission/{permission_id}/reply");
                match self.api_post(&fallback, json!({ "reply": response })).await {
                    Ok(value) => Ok(value),
                    Err(_) => Err(first_error),
                }
            }
        }
    }

    pub async fn list_questions(&self) -> Result<Value, String> {
        self.api_get("/question").await
    }

    pub async fn reply_question(
        &self,
        request_id: &str,
        answers: Vec<Vec<String>>,
    ) -> Result<Value, String> {
        self.api_post(
            &format!("/question/{request_id}/reply"),
            json!({ "answers": answers }),
        )
        .await
    }

    pub async fn reject_question(&self, request_id: &str) -> Result<Value, String> {
        self.api_post(&format!("/question/{request_id}/reject"), json!({}))
            .await
    }
}

pub(crate) fn effective_directory(directory: &str) -> String {
    if directory.trim().is_empty() {
        std::env::current_dir()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|_| ".".to_string())
    } else {
        directory.to_string()
    }
}

async fn json_response(response: reqwest::Response, path: &str) -> Result<Value, String> {
    let status = response.status();
    let text = response
        .text()
        .await
        .map_err(|e| format!("{path}: failed to read body: {e}"))?;

    if !status.is_success() {
        return Err(format!(
            "{path} returned {status}: {}",
            truncate(&text, 300)
        ));
    }

    if text.trim().is_empty() {
        return Ok(Value::Null);
    }

    serde_json::from_str(&text).map_err(|e| format!("{path}: invalid JSON: {e}"))
}

fn truncate(text: &str, max: usize) -> String {
    let trimmed = text.trim();
    if trimmed.chars().count() <= max {
        trimmed.to_string()
    } else {
        let cut: String = trimmed.chars().take(max).collect();
        format!("{cut}…")
    }
}

pub fn build_prompt_body(text: &str, agent: Option<&str>, model: Option<&str>) -> Value {
    let mut body = json!({
        "parts": [{ "type": "text", "text": text }]
    });

    if let Some(agent) = agent.filter(|a| !a.trim().is_empty()) {
        body["agent"] = json!(agent);
    }

    if let Some(model) = model.filter(|m| !m.trim().is_empty()) {
        if let Some((provider, id)) = model.split_once('/') {
            body["model"] = json!({ "providerID": provider, "modelID": id });
        }
    }

    body
}

pub fn extract_text_parts(response: &Value) -> String {
    response
        .get("parts")
        .and_then(|parts| parts.as_array())
        .map(|parts| {
            parts
                .iter()
                .filter(|part| part.get("type").and_then(|t| t.as_str()) == Some("text"))
                .filter_map(|part| part.get("text").and_then(|t| t.as_str()))
                .collect::<Vec<_>>()
                .join("\n")
        })
        .unwrap_or_default()
}

/// Builds a worker session title from a task prompt.
pub fn worker_title(prompt: &str) -> String {
    let cleaned = prompt.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut title: String = cleaned.chars().take(60).collect();
    if cleaned.chars().count() > 60 {
        title.push('…');
    }
    if title.is_empty() {
        "task".to_string()
    } else {
        title
    }
}

/// Pick a port for the managed server when none is configured.
pub fn random_port() -> u16 {
    let bytes = uuid::Uuid::new_v4().into_bytes();
    4300 + (u16::from_le_bytes([bytes[0], bytes[1]]) % 500)
}

/// Parse the `http://127.0.0.1:PORT` URL out of opencode's startup output.
pub fn parse_server_url(text: &str) -> Option<String> {
    static URL_REGEX: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    let regex = URL_REGEX.get_or_init(|| {
        regex::Regex::new(r"https?://(?:127\.0\.0\.1|localhost):\d+").expect("valid url regex")
    });
    regex.find(text).map(|m| m.as_str().to_string())
}

/// Incremental parser for `text/event-stream` payloads.
pub struct SseParser {
    buffer: String,
    data: String,
}

impl SseParser {
    pub fn new() -> Self {
        Self {
            buffer: String::new(),
            data: String::new(),
        }
    }

    pub fn push(&mut self, chunk: &str) -> Vec<String> {
        self.buffer.push_str(chunk);
        let mut events = Vec::new();

        while let Some(index) = self.buffer.find('\n') {
            let line = self.buffer[..index].trim_end_matches('\r').to_string();
            self.buffer.drain(..=index);

            if line.is_empty() {
                if !self.data.is_empty() {
                    events.push(self.data.trim_end_matches('\n').to_string());
                    self.data.clear();
                }
            } else if let Some(rest) = line.strip_prefix("data:") {
                self.data.push_str(rest.strip_prefix(' ').unwrap_or(rest));
                self.data.push('\n');
            }
        }

        events
    }
}

impl Default for SseParser {
    fn default() -> Self {
        Self::new()
    }
}

pub fn find_executable(program: &str) -> Option<PathBuf> {
    if program.contains('/') {
        let path = PathBuf::from(program);
        return path.exists().then_some(path);
    }

    // PATH first.
    if let Some(found) = crate::which(program) {
        return Some(found);
    }

    // Common install locations that a GUI-launched server may not inherit.
    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Some(home) = dirs::home_dir() {
        candidates.push(home.join(".opencode").join("bin").join(program));
        candidates.push(home.join(".local").join("bin").join(program));
        candidates.push(home.join(".bun").join("bin").join(program));
        candidates.push(home.join(".local/share/mise/shims").join(program));
        candidates.push(home.join(".cargo").join("bin").join(program));
    }
    candidates.push(PathBuf::from("/usr/local/bin").join(program));
    candidates.push(PathBuf::from("/usr/bin").join(program));

    candidates.into_iter().find(|path| path.exists())
}
