//! Speech-to-text pipeline shared by the HTTP endpoint and the voice session.

use std::io::Write;
use std::process::{Command, Stdio};

use tempfile::NamedTempFile;

/// Transcribes encoded audio (any container ffmpeg understands) to text.
pub fn transcribe_audio_bytes(audio: &[u8]) -> Result<String, String> {
    let mut temp_file = NamedTempFile::new().map_err(|e| format!("TEMP_FILE_FAILED:{e}"))?;
    temp_file
        .write_all(audio)
        .map_err(|e| format!("TEMP_WRITE_FAILED:{e}"))?;
    let temp_path = temp_file
        .path()
        .to_str()
        .ok_or_else(|| "TEMP_PATH_FAILED".to_string())?
        .to_string();

    let wav_file =
        NamedTempFile::with_suffix(".wav").map_err(|e| format!("TEMP_WAV_FAILED:{e}"))?;
    let wav_path = wav_file
        .path()
        .to_str()
        .ok_or_else(|| "TEMP_WAV_FAILED".to_string())?
        .to_string();

    crate::run_with_retry("ffmpeg", || {
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
            .map_err(|e| format!("FFMPEG_SPAWN_FAILED:{e}"))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(format!("FFMPEG_RUN_FAILED:{stderr}"));
        }
        Ok(())
    })
    .map_err(|e| {
        eprintln!("[ERROR] ffmpeg failed: {e}");
        "FFMPEG_FAILED".to_string()
    })?;

    transcribe_wav_file(&wav_path)
}

/// Transcribes a 16 kHz mono 16-bit WAV buffer directly (voice session path).
pub fn transcribe_wav_bytes(wav: &[u8]) -> Result<String, String> {
    let mut temp_file =
        NamedTempFile::with_suffix(".wav").map_err(|e| format!("TEMP_WAV_FAILED:{e}"))?;
    temp_file
        .write_all(wav)
        .map_err(|e| format!("TEMP_WRITE_FAILED:{e}"))?;
    let path = temp_file
        .path()
        .to_str()
        .ok_or_else(|| "TEMP_PATH_FAILED".to_string())?
        .to_string();

    transcribe_wav_file(&path)
}

fn transcribe_wav_file(wav_path: &str) -> Result<String, String> {
    let stt_executable = crate::cached_config().stt_executable();

    let output = crate::run_with_retry(&stt_executable, || {
        Command::new(&stt_executable)
            .arg("--quiet")
            .arg("transcribe")
            .arg(wav_path)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .map_err(|e| format!("STT_SPAWN_FAILED:{e}"))
    })
    .map_err(|e| {
        eprintln!("[ERROR] {e}");
        "STT_FAILED".to_string()
    })?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        eprintln!("[ERROR] STT exited with {:?}: {stderr}", output.status);
        return Err("STT_FAILED".to_string());
    }

    let text = String::from_utf8_lossy(&output.stdout).to_string();
    let clean_text = text
        .split("\n\n")
        .last()
        .unwrap_or(&text)
        .trim()
        .to_string();

    if clean_text.is_empty() {
        return Err("EMPTY_TRANSCRIPTION".to_string());
    }

    Ok(clean_text)
}
