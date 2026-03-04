use axum::{
    routing::post,
    Router, Form, response::Html,
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::process::{Command, Stdio};
use tower_http::cors::CorsLayer;
use base64::prelude::*;
use std::io::Write;
use tempfile::NamedTempFile;

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

#[tokio::main]
async fn main() {
    let cors = CorsLayer::permissive();

    let app = Router::new()
        .route("/synthesize", post(synthesize))
        .route("/transcribe", post(transcribe))
        .route("/chat", post(chat))
        .layer(cors);

    let listener = tokio::net::TcpListener::bind("127.0.0.1:2026").await.unwrap();
    println!("Server launched. Listening on http://127.0.0.1:2026");
    axum::serve(listener, app).await.unwrap();
}

async fn synthesize(Form(payload): Form<SynthesizeRequest>) -> Html<String> {
    if payload.text.trim().is_empty() {
        return Html(String::new());
    }

    let mut child = Command::new("piper-tts")
        .arg("--model")
        .arg("/home/goitseone/piper-voices/en_US-libritts_r-high.onnx")
        .arg("--output_file")
        .arg("-")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .expect("Failed to spawn piper-tts");

    if let Some(mut stdin) = child.stdin.take() {
        // piper-tts processes text line-by-line. We replace newlines with spaces 
        // to ensure the entire block of text is submitted as a single sequence.
        let single_line_text = payload.text.replace("\n", " ");
        stdin.write_all(single_line_text.as_bytes()).expect("Failed to write to stdin");
    }

    let output = child.wait_with_output().expect("Failed to read stdout");
    
    if !output.status.success() {
        eprintln!("piper-tts failed with status: {}", output.status);
        if let Ok(err_str) = String::from_utf8(output.stderr) {
            eprintln!("piper-tts stderr: {}", err_str);
        }
        return Html(String::from("<span style='color:red;'>Failed to generate audio transmission.</span>"));
    }

    let b64 = BASE64_STANDARD.encode(&output.stdout);
    
    let html = format!(
        r#"<audio controls autoplay style="width:100%; height: 40px;" src="data:audio/wav;base64,{}"></audio>"#,
        b64
    );
    Html(html)
}

async fn transcribe(Form(payload): Form<TranscribeRequest>) -> Html<String> {
    // 1. Decode base64 audio
    let audio_data = match BASE64_STANDARD.decode(&payload.audio_base64) {
        Ok(data) => data,
        Err(e) => {
            eprintln!("Failed to decode base64 audio: {}", e);
            return Html(String::from("Error decoding audio"));
        }
    };

    // 2. Write to a temporary file
    let mut temp_file = match NamedTempFile::new() {
        Ok(file) => file,
        Err(e) => {
            eprintln!("Failed to create temp file: {}", e);
            return Html(String::from("Error creating temp file"));
        }
    };

    if let Err(e) = temp_file.write_all(&audio_data) {
        eprintln!("Failed to write audio to temp file: {}", e);
        return Html(String::from("Error writing audio file"));
    }

    let temp_path = temp_file.path().to_str().unwrap().to_string();

    let wav_file = match NamedTempFile::with_suffix(".wav") {
        Ok(f) => f,
        Err(e) => {
            eprintln!("Failed to create wav temp file: {}", e);
            return Html(String::from("Error creating wav temp file"));
        }
    };
    let wav_path = wav_file.path().to_str().unwrap().to_string();

    let ffmpeg_status = Command::new("ffmpeg")
        .arg("-y") // overwrite output file just in case
        .arg("-i")
        .arg(&temp_path) // input file (webm/ogg)
        .arg("-ar")
        .arg("16000") // 16kHz sampling rate
        .arg("-ac")
        .arg("1") // Mono channel
        .arg(&wav_path) // output wav file
        .stdout(Stdio::null()) // suppress noisy ffmpeg stdout
        .stderr(Stdio::piped())
        .status()
        .expect("Failed to execute ffmpeg");

    if !ffmpeg_status.success() {
        eprintln!("ffmpeg failed to convert audio format");
        return Html(String::from("Transcription failed: invalid audio format."));
    }

    // 3. Run voxtype transcribe
    let output = Command::new("voxtype")
        .arg("--quiet")
        .arg("transcribe")
        .arg(&wav_path)
        .stdout(Stdio::piped())
        .output()
        .expect("Failed to execute voxtype");

    // 4. Return the transcribed text
    if output.status.success() {
        let text = String::from_utf8_lossy(&output.stdout).to_string();
        
        // Strip voxtype stdout logs which end with an empty double newline "\n\n"
        let clean_text = text.split("\n\n").last().unwrap_or(&text).trim().to_string();
        
        Html(clean_text)
    } else {
        eprintln!("voxtype failed with status: {}", output.status);
        if let Ok(err_str) = String::from_utf8(output.stderr) {
            eprintln!("voxtype stderr: {}", err_str);
        }
        Html(String::from("Transcription failed."))
    }
}

async fn chat(Form(payload): Form<ChatRequest>) -> Html<String> {
    if payload.text.trim().is_empty() {
        return Html(String::new());
    }

    let client = reqwest::Client::new();
    let request_body = json!({
        "model": "qwen/qwen3-vl-4b",
        "messages": [
            {
                "role": "system",
                "content": "You are a concise, highly efficient, and direct AI assistant, inspired by sleek futuristic interfaces like Grok and SpaceX. Respond with crisp, accurate information without run-on sentences or unnecessary filler."
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

    let res = client
        .post("http://localhost:1234/v1/chat/completions")
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
            eprintln!("Reqwest error calling LMStudio: {}", e);
            Html(String::from("Error connecting to AI Server."))
        }
    }
}
