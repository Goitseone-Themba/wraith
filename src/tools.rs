//! Toolchain detection and (loopback-guarded) one-click installation.

use std::process::Stdio;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use serde::Serialize;
use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;
use tokio::sync::broadcast;

const INSTALL_CHANNEL_CAPACITY: usize = 512;

#[derive(Debug, Clone, Serialize)]
pub struct InstallMethod {
    pub id: String,
    pub label: String,
    pub command: String,
    pub one_click: bool,
    pub available: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct ToolStatus {
    pub id: String,
    pub name: String,
    pub description: String,
    pub installed: bool,
    pub path: Option<String>,
    pub version: Option<String>,
    pub required: bool,
    pub install_methods: Vec<InstallMethod>,
    pub docs_url: String,
}

pub struct ToolState {
    logs: broadcast::Sender<String>,
    installing: AtomicBool,
}

impl ToolState {
    pub fn new() -> Arc<Self> {
        let (logs, _) = broadcast::channel(INSTALL_CHANNEL_CAPACITY);
        Arc::new(Self {
            logs,
            installing: AtomicBool::new(false),
        })
    }

    pub fn subscribe(&self) -> broadcast::Receiver<String> {
        self.logs.subscribe()
    }

    pub fn is_installing(&self) -> bool {
        self.installing.load(Ordering::SeqCst)
    }

    pub fn install_status(&self) -> Value {
        json!({ "installing": self.is_installing() })
    }

    pub async fn status(&self, stt_executable: &str) -> Value {
        let platform = std::env::consts::OS;
        let arch = std::env::consts::ARCH;

        let opencode = tool_status(
            "opencode",
            "opencode",
            "AI coding agent with tools, sessions and MCP support",
            true,
            "opencode",
            opencode_install_methods(platform),
            "https://opencode.ai/docs/",
        )
        .await;

        let piper = tool_status(
            "piper",
            "piper-tts",
            "Local neural text-to-speech",
            true,
            "piper-tts",
            vec![InstallMethod {
                id: "manual".to_string(),
                label: "Manual install".to_string(),
                command: "https://github.com/rhasspy/piper#installation".to_string(),
                one_click: false,
                available: true,
            }],
            "https://github.com/rhasspy/piper",
        )
        .await;

        let stt = tool_status(
            "stt",
            stt_executable,
            "Speech-to-text executable used by the voice loop",
            false,
            stt_executable,
            vec![InstallMethod {
                id: "manual".to_string(),
                label: "Manual install".to_string(),
                command: "https://github.com/taylor-vann/voxtype".to_string(),
                one_click: false,
                available: true,
            }],
            "https://github.com/taylor-vann/voxtype",
        )
        .await;

        let ffmpeg = tool_status(
            "ffmpeg",
            "ffmpeg",
            "Audio conversion for transcription",
            false,
            "ffmpeg",
            vec![InstallMethod {
                id: "manual".to_string(),
                label: "Manual install".to_string(),
                command: if platform == "linux" {
                    "sudo pacman -S ffmpeg   # or: sudo apt install ffmpeg".to_string()
                } else {
                    "brew install ffmpeg".to_string()
                },
                one_click: false,
                available: true,
            }],
            "https://ffmpeg.org/download.html",
        )
        .await;

        let tmux = tool_status(
            "tmux",
            "tmux",
            "Optional: persistent worker sessions for orchestration",
            false,
            "tmux",
            vec![],
            "https://github.com/tmux/tmux",
        )
        .await;

        json!({
            "platform": platform,
            "arch": arch,
            "tools": [opencode, piper, stt, ffmpeg, tmux],
        })
    }

    /// Spawn an installer command, streaming its output to subscribers.
    /// Returns an error string when the request is not allowed.
    pub fn start_install(self: &Arc<Self>, tool: &str, method: &str) -> Result<String, String> {
        if tool != "opencode" {
            return Err(format!("one-click install is not supported for '{tool}'"));
        }
        if self.installing.swap(true, Ordering::SeqCst) {
            return Err("an install is already running".to_string());
        }

        let command =
            install_command(method).ok_or_else(|| format!("unknown install method '{method}'"))?;

        let state = Arc::clone(self);
        let label = command.clone();
        tokio::spawn(async move {
            let outcome = run_installer(&state, &command).await;
            let _ = state.logs.send(format!("__EXIT__{outcome}"));
            state.installing.store(false, Ordering::SeqCst);
        });

        Ok(label)
    }
}

async fn run_installer(state: &Arc<ToolState>, command: &str) -> i32 {
    let _ = state.logs.send(format!("$ {command}"));

    let mut child = match Command::new("sh")
        .arg("-c")
        .arg(command)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
    {
        Ok(child) => child,
        Err(e) => {
            let _ = state.logs.send(format!("failed to start installer: {e}"));
            return -1;
        }
    };

    if let Some(stdout) = child.stdout.take() {
        let logs = state.logs.clone();
        tokio::spawn(async move {
            let mut lines = BufReader::new(stdout).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                let _ = logs.send(line);
            }
        });
    }

    if let Some(stderr) = child.stderr.take() {
        let logs = state.logs.clone();
        tokio::spawn(async move {
            let mut lines = BufReader::new(stderr).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                let _ = logs.send(line);
            }
        });
    }

    match child.wait().await {
        Ok(status) => status.code().unwrap_or(-1),
        Err(e) => {
            let _ = state.logs.send(format!("installer wait failed: {e}"));
            -1
        }
    }
}

pub fn install_command(method: &str) -> Option<String> {
    match method {
        "official" => Some("curl -fsSL https://opencode.ai/install | bash".to_string()),
        "npm" => Some("npm install -g opencode-ai".to_string()),
        "brew" => Some("brew install sst/tap/opencode".to_string()),
        "mise" => Some("mise use -g opencode".to_string()),
        _ => None,
    }
}

pub fn opencode_install_methods(platform: &str) -> Vec<InstallMethod> {
    let has = |program: &str| super::opencode::find_executable(program).is_some();

    let mut methods = vec![
        InstallMethod {
            id: "official".to_string(),
            label: "Official install script".to_string(),
            command: install_command("official").unwrap(),
            one_click: true,
            available: has("curl") && platform != "windows",
        },
        InstallMethod {
            id: "npm".to_string(),
            label: "npm (global)".to_string(),
            command: install_command("npm").unwrap(),
            one_click: true,
            available: has("npm"),
        },
        InstallMethod {
            id: "brew".to_string(),
            label: "Homebrew".to_string(),
            command: install_command("brew").unwrap(),
            one_click: true,
            available: has("brew"),
        },
        InstallMethod {
            id: "mise".to_string(),
            label: "mise".to_string(),
            command: install_command("mise").unwrap(),
            one_click: true,
            available: has("mise"),
        },
    ];

    if platform == "linux" && has("pacman") {
        methods.push(InstallMethod {
            id: "pacman".to_string(),
            label: "pacman (requires sudo)".to_string(),
            command: "sudo pacman -S opencode".to_string(),
            one_click: false,
            available: true,
        });
    }

    methods
}

async fn tool_status(
    id: &str,
    name: &str,
    description: &str,
    required: bool,
    executable: &str,
    install_methods: Vec<InstallMethod>,
    docs_url: &str,
) -> ToolStatus {
    let path = super::opencode::find_executable(executable);

    let version = match path.as_ref() {
        Some(path) => executable_version(executable, path).await,
        None => None,
    };

    ToolStatus {
        id: id.to_string(),
        name: name.to_string(),
        description: description.to_string(),
        installed: path.is_some(),
        path: path.map(|p| p.display().to_string()),
        version,
        required,
        install_methods,
        docs_url: docs_url.to_string(),
    }
}

async fn executable_version(executable: &str, path: &std::path::Path) -> Option<String> {
    let attempts: &[&[&str]] = &[&["--version"], &["-V"], &["version"]];

    for args in attempts {
        let output = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            Command::new(path).args(*args).output(),
        )
        .await;

        if let Ok(Ok(out)) = output {
            if out.status.success() {
                let text = String::from_utf8_lossy(&out.stdout);
                let first = text.lines().next().unwrap_or("").trim();
                if !first.is_empty() {
                    return Some(first.to_string());
                }
            }
        }
    }

    let _ = executable;
    None
}
