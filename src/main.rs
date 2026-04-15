#[cfg(test)]
mod tests;

use serde::Deserialize;
use std::env;
use std::fs;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::OnceLock;
use std::time::Duration;

use axum::{
    response::Html,
    routing::{get, post},
    Form, Router,
};
use axum_server::tls_rustls::RustlsConfig;
use base64::prelude::*;
use serde_json::json;
use std::process::{Command, Stdio};
use std::io::Write;
use tempfile::NamedTempFile;
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
    println!();
    println!("  Override with env vars: WRAITH_*");
    println!();
}

fn create_app() -> Router {
    let cors = CorsLayer::permissive();
    Router::new()
        .route("/synthesize", post(synthesize))
        .route("/transcribe", post(transcribe))
        .route("/chat", post(chat))
        .route("/", get(home))
        .layer(cors)
}

async fn serve_https(addr: SocketAddr, tls_config: RustlsConfig, app: Router) {
    println!("HTTPS server launched. Listening on https://{addr}");
    let service = app.into_make_service();
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

    let cert_path = config.cert_path();
    let key_path = config.key_path();

    if fs::metadata(&cert_path).is_ok() && fs::metadata(&key_path).is_ok() {
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
    } else {
        println!();
        println!("TLS certificates not found!");
        println!();
        println!("Generate certificates with:");
        println!("  openssl req -x509 -newkey rsa:4096 \\");
        println!("    -keyout {} -out {} \\", key_path, cert_path);
        println!("    -days 365 -nodes");
        println!();
        println!("Then run: cargo run --release");
        println!();
        std::process::exit(1);
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

fn which(program: &str) -> Option<PathBuf> {
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
    let config = cached_config();

    let audio_data = match BASE64_STANDARD.decode(&payload.audio_base64) {
        Ok(data) => data,
        Err(e) => {
            eprintln!("[ERROR] Failed to decode base64 audio: {}", e);
            return Html(String::from("ERROR:DECODE_FAILED"));
        }
    };

    let mut temp_file = match NamedTempFile::new() {
        Ok(file) => file,
        Err(e) => {
            eprintln!("[ERROR] Failed to create temp file: {}", e);
            return Html(String::from("ERROR:TEMP_FILE_FAILED"));
        }
    };

    if let Err(e) = temp_file.write_all(&audio_data) {
        eprintln!("[ERROR] Failed to write audio to temp file: {}", e);
        return Html(String::from("ERROR:TEMP_WRITE_FAILED"));
    }

    let temp_path = temp_file.path().to_str().unwrap().to_string();

    let wav_file = match NamedTempFile::with_suffix(".wav") {
        Ok(f) => f,
        Err(e) => {
            eprintln!("[ERROR] Failed to create wav temp file: {}", e);
            return Html(String::from("ERROR:TEMP_WAV_FAILED"));
        }
    };
    let wav_path = wav_file.path().to_str().unwrap().to_string();

    let ffmpeg_result = run_with_retry("ffmpeg", || {
        let output = Command::new("ffmpeg")
            .arg("-y")
            .arg("-i")
            .arg(&temp_path)
            .arg("-ar")
            .arg("16000")
            .arg("-ac")
            .arg("1")
            .arg(&wav_path)
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .output()
            .map_err(|e| format!("Failed to execute ffmpeg: {}", e))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(format!("ffmpeg conversion failed: {}", stderr));
        }
        Ok(())
    });

    if let Err(e) = ffmpeg_result {
        eprintln!("[ERROR] ffmpeg failed after retries: {}", e);
        return Html(String::from("ERROR:FFMPEG_FAILED"));
    }

    let stt_executable = config.stt_executable();

    let output = run_with_retry(&stt_executable, || {
        Command::new(&stt_executable)
            .arg("--quiet")
            .arg("transcribe")
            .arg(&wav_path)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .map_err(|e| format!("Failed to execute {}: {}", stt_executable, e))
    });

    match output {
        Ok(out) => {
            if out.status.success() {
                let text = String::from_utf8_lossy(&out.stdout).to_string();
                let clean_text = text
                    .split("\n\n")
                    .last()
                    .unwrap_or(&text)
                    .trim()
                    .to_string();

                if clean_text.is_empty() {
                    Html(String::from("ERROR:EMPTY_TRANSCRIPTION"))
                } else {
                    Html(clean_text)
                }
            } else {
                let stderr = String::from_utf8_lossy(&out.stderr);
                eprintln!(
                    "[ERROR] {} exited with status {:?}: {}",
                    stt_executable, out.status, stderr
                );
                Html(String::from("ERROR:STT_FAILED"))
            }
        }
        Err(e) => {
            eprintln!("[ERROR] STT failed after retries: {}", e);
            Html(String::from("ERROR:STT_FAILED"))
        }
    }
}

async fn chat(Form(payload): Form<ChatRequest>) -> Html<String> {
    let config = load_config();

    if payload.text.trim().is_empty() {
        return Html(String::new());
    }

    let client = reqwest::Client::new();
    let request_body = json!({
        "model": config.llm_model(),
        "messages": [
            {
                "role": "system",
                "content": "You are a concise, highly efficient, and direct AI
                    assistant, inspired by sleek futuristic interfaces like
                    Grok and SpaceX. Respond with crisp, accurate information
                    without run-on sentences or unnecessary filler. note: your
                    response is going to be read outloud by a text to speech model,
                    so no emojis or markdown respond in a way that a text to speech model can read."
            },
            {
                "role": "user",
                "content": payload.text
            }
        ],
        "temperature": 0.7,
        "max_tokens": -1,
        "stream": false
    });

    let endpoint = config.llm_endpoint();
    let res = client
        .post(&endpoint)
        .json(&request_body)
        .send()
        .await;

    match res {
        Ok(response) => {
            if response.status().is_success() {
                if let Ok(lm_res) = response.json::<LmStudioResponse>().await {
                    if let Some(choice) = lm_res.choices.first() {
                        return Html(choice.message.content.clone());
                    }
                }
            }
            Html(String::from("Error: Failed to parse AI response."))
        }
        Err(e) => {
            eprintln!("Reqwest error calling LLM: {}", e);
            Html(String::from("Error connecting to AI Server."))
        }
    }
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
