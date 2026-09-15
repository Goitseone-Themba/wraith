//! Speech synthesis helpers: sentence chunking, persistent piper workers,
//! WAV writing and voice-speed commands.

use std::sync::OnceLock;

use bytes::Bytes;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::process::{Child, ChildStdin, Command};
use tokio::sync::mpsc;

pub const DEFAULT_SAMPLE_RATE: u32 = 22050;
pub const DEFAULT_MAX_SENTENCE_CHARS: usize = 220;

/// Splits streamed model output into speakable sentences.
pub struct SentenceChunker {
    max_chars: usize,
    buffer: String,
    in_fence: bool,
    fence_held: String,
}

impl SentenceChunker {
    pub fn new(max_chars: usize) -> Self {
        Self {
            max_chars: max_chars.max(40),
            buffer: String::new(),
            in_fence: false,
            fence_held: String::new(),
        }
    }

    pub fn push(&mut self, delta: &str) -> Vec<String> {
        let mut out = Vec::new();

        for character in delta.chars() {
            if self.in_fence {
                self.buffer.push(character);
                if self.buffer.ends_with("```") {
                    self.buffer.clear();
                    self.in_fence = false;
                    self.buffer = std::mem::take(&mut self.fence_held);
                } else if self.buffer.len() > 8 && !self.buffer.ends_with('`') {
                    self.buffer.clear();
                }
                continue;
            }

            self.buffer.push(character);

            if self.buffer.ends_with("```") {
                self.buffer.truncate(self.buffer.len() - 3);
                out.extend(self.drain_sentences());
                self.fence_held = std::mem::take(&mut self.buffer);
                self.in_fence = true;
                continue;
            }

            if matches!(character, '.' | '!' | '?' | '\n')
                || character.is_whitespace()
                || self.buffer.chars().count() >= self.max_chars
            {
                out.extend(self.drain_sentences());
            }
        }

        out
    }

    fn drain_sentences(&mut self) -> Vec<String> {
        let mut sentences = Vec::new();

        loop {
            if let Some(index) = find_boundary(&self.buffer) {
                let sentence: String = self.buffer.drain(..=index).collect();
                if let Some(cleaned) = clean_for_speech(&sentence) {
                    sentences.push(cleaned);
                }
                continue;
            }

            if self.buffer.chars().count() >= self.max_chars {
                let split = self
                    .buffer
                    .char_indices()
                    .nth(self.max_chars)
                    .map(|(index, _)| index)
                    .unwrap_or(self.buffer.len());
                let rest = self.buffer.split_off(split);
                let head = std::mem::replace(&mut self.buffer, rest);
                if let Some(cleaned) = clean_for_speech(&head) {
                    sentences.push(cleaned);
                }
                continue;
            }

            break;
        }

        sentences
    }

    pub fn flush(&mut self) -> Option<String> {
        if self.in_fence {
            self.in_fence = false;
            self.buffer.clear();
            self.buffer = std::mem::take(&mut self.fence_held);
        }
        if self.buffer.trim().is_empty() {
            self.buffer.clear();
            return None;
        }
        let pending = std::mem::take(&mut self.buffer);
        clean_for_speech(&pending)
    }
}

fn find_boundary(text: &str) -> Option<usize> {
    let chars: Vec<(usize, char)> = text.char_indices().collect();

    for (position, (byte_index, character)) in chars.iter().enumerate() {
        match character {
            '\n' => return Some(*byte_index),
            '.' | '!' | '?' => {
                match chars.get(position + 1) {
                    Some((_, next)) if next.is_whitespace() => {
                        if *character == '.' && is_abbreviation(&chars, position) {
                            continue;
                        }
                        return Some(*byte_index);
                    }
                    None => {
                        // Strong enders may complete the stream; a trailing '.'
                        // is held in case it is an abbreviation or decimal.
                        if *character != '.' {
                            return Some(*byte_index);
                        }
                    }
                    _ => {}
                }
            }
            _ => {}
        }
    }

    None
}

fn is_abbreviation(chars: &[(usize, char)], position: usize) -> bool {
    // Single-letter initial ("A.") or decimal-ish patterns are left intact.
    match chars.get(position.wrapping_sub(1)) {
        Some((_, previous)) if previous.is_ascii_digit() => {
            // 3. 5 -> likely a numbered list; treat as boundary anyway unless
            // the next non-space character is also a digit (e.g. "3. 14").
            matches!(chars.get(position + 2), Some((_, next)) if next.is_ascii_digit())
        }
        _ => false,
    }
}

/// Strips markdown noise so piper reads sensible prose.
pub fn clean_for_speech(text: &str) -> Option<String> {
    static CODE_BLOCK: OnceLock<regex::Regex> = OnceLock::new();
    static INLINE_CODE: OnceLock<regex::Regex> = OnceLock::new();
    static URL: OnceLock<regex::Regex> = OnceLock::new();
    static MARKDOWN: OnceLock<regex::Regex> = OnceLock::new();

    let mut value = text.to_string();

    let code_block =
        CODE_BLOCK.get_or_init(|| regex::Regex::new(r"(?s)```.*?```").expect("code regex"));
    value = code_block.replace_all(&value, " ").to_string();

    let inline_code =
        INLINE_CODE.get_or_init(|| regex::Regex::new(r"`[^`]*`").expect("inline code regex"));
    value = inline_code.replace_all(&value, " ").to_string();

    let url = URL.get_or_init(|| regex::Regex::new(r"https?://\S+").expect("url regex"));
    value = url.replace_all(&value, " ").to_string();

    let markdown = MARKDOWN
        .get_or_init(|| regex::Regex::new(r"[*_#>~]+").expect("markdown regex"));
    value = markdown.replace_all(&value, " ").to_string();

    value = value.split_whitespace().collect::<Vec<_>>().join(" ");

    if value.trim().is_empty() {
        None
    } else {
        Some(value.trim().to_string())
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum SpeedChange {
    Faster,
    Slower,
    Normal,
    Set(f64),
}

/// Interprets short spoken commands like "speak faster" into a new speed.
pub fn parse_speed_command(text: &str, current: f64) -> Option<(SpeedChange, f64)> {
    let normalized = text.to_lowercase();
    let normalized = normalized.trim().trim_end_matches(['.', '!', ',']);

    if normalized.chars().count() > 40 {
        return None;
    }

    let clamp = |value: f64| ((value * 100.0).round() / 100.0).clamp(0.5, 2.0);

    let faster = ["speak faster", "talk faster", "faster please", "speed up", "speak up"];
    let slower = ["speak slower", "talk slower", "slower please", "slow down"];
    let normal = ["speak normally", "normal speed", "speak normal", "reset speed"];

    if faster.iter().any(|phrase| normalized.contains(phrase)) {
        return Some((SpeedChange::Faster, clamp(current * 1.25)));
    }
    if slower.iter().any(|phrase| normalized.contains(phrase)) {
        return Some((SpeedChange::Slower, clamp(current * 0.8)));
    }
    if normal.iter().any(|phrase| normalized.contains(phrase)) {
        return Some((SpeedChange::Normal, 1.0));
    }

    static PERCENT: OnceLock<regex::Regex> = OnceLock::new();
    let percent = PERCENT.get_or_init(|| {
        regex::Regex::new(r"(?:speed|rate)[^\d]{0,12}(\d{2,3})\s*%").expect("percent regex")
    });
    if let Some(captures) = percent.captures(normalized) {
        if let Ok(value) = captures[1].parse::<f64>() {
            return Some((SpeedChange::Set(value / 100.0), clamp(value / 100.0)));
        }
    }

    None
}

/// Minimal PCM WAV writer (16-bit little-endian, mono).
pub fn write_wav(pcm: &[u8], sample_rate: u32) -> Vec<u8> {
    let channels: u16 = 1;
    let bits_per_sample: u16 = 16;
    let byte_rate = sample_rate * u32::from(channels) * u32::from(bits_per_sample) / 8;
    let block_align = channels * bits_per_sample / 8;
    let data_len = pcm.len() as u32;

    let mut out = Vec::with_capacity(44 + pcm.len());
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data_len).to_le_bytes());
    out.extend_from_slice(b"WAVE");
    out.extend_from_slice(b"fmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&channels.to_le_bytes());
    out.extend_from_slice(&sample_rate.to_le_bytes());
    out.extend_from_slice(&byte_rate.to_le_bytes());
    out.extend_from_slice(&block_align.to_le_bytes());
    out.extend_from_slice(&bits_per_sample.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data_len.to_le_bytes());
    out.extend_from_slice(pcm);
    out
}

/// Reads the sample rate from a piper voice config JSON (`<model>.json`).
pub fn sample_rate_from_model(model_path: &str) -> Option<u32> {
    let config_path = format!("{model_path}.json");
    let contents = std::fs::read_to_string(config_path).ok()?;
    let value: serde_json::Value = serde_json::from_str(&contents).ok()?;

    let candidates = [
        value.get("audio").and_then(|v| v.get("sample_rate")),
        value.get("sample_rate"),
        value.get("dataset").and_then(|v| v.get("sample_rate")),
    ];

    for candidate in candidates.into_iter().flatten() {
        if let Some(rate) = candidate.as_u64() {
            return Some(rate as u32);
        }
    }

    None
}

/// Long-lived piper process fed one sentence per line.
pub struct PiperWorker {
    child: Child,
    stdin: ChildStdin,
    model: String,
    tx: mpsc::Sender<Bytes>,
    pub sample_rate: u32,
}

impl PiperWorker {
    pub async fn spawn(
        model: &str,
        length_scale: f64,
        tx: mpsc::Sender<Bytes>,
    ) -> Result<Self, String> {
        let executable = crate::opencode::find_executable("piper-tts")
            .map(|path| path.display().to_string())
            .unwrap_or_else(|| "piper-tts".to_string());

        let mut child = Command::new(&executable)
            .arg("--model")
            .arg(model)
            .arg("--output_raw")
            .arg("--length_scale")
            .arg(format!("{length_scale:.3}"))
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .map_err(|e| format!("failed to start piper: {e}"))?;

        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| "failed to capture piper stdin".to_string())?;
        let mut stdout = child
            .stdout
            .take()
            .ok_or_else(|| "failed to capture piper stdout".to_string())?;

        let stream_tx = tx.clone();
        tokio::spawn(async move {
            let mut buffer = vec![0u8; 16384];
            loop {
                match stdout.read(&mut buffer).await {
                    Ok(0) => break,
                    Ok(n) => {
                        if stream_tx.send(Bytes::copy_from_slice(&buffer[..n])).await.is_err() {
                            break;
                        }
                    }
                    Err(_) => break,
                }
            }
        });

        let sample_rate = sample_rate_from_model(model).unwrap_or(DEFAULT_SAMPLE_RATE);

        Ok(Self {
            child,
            stdin,
            model: model.to_string(),
            tx,
            sample_rate,
        })
    }

    pub async fn say(&mut self, text: &str) -> Result<(), String> {
        if text.trim().is_empty() {
            return Ok(());
        }
        self.stdin
            .write_all(text.as_bytes())
            .await
            .map_err(|e| format!("piper write failed: {e}"))?;
        self.stdin
            .write_all(b"\n")
            .await
            .map_err(|e| format!("piper write failed: {e}"))?;
        self.stdin
            .flush()
            .await
            .map_err(|e| format!("piper flush failed: {e}"))
    }

    pub async fn restart(&mut self, length_scale: f64) -> Result<(), String> {
        let _ = self.child.kill().await;
        let _ = self.child.wait().await;

        let replacement = Self::spawn(&self.model, length_scale, self.tx.clone()).await?;
        *self = replacement;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chunker_splits_on_punctuation() {
        let mut chunker = SentenceChunker::new(200);
        let first = chunker.push("Hello there. How are");
        assert_eq!(first, vec!["Hello there.".to_string()]);
        let second = chunker.push(" you doing?");
        assert_eq!(second, vec!["How are you doing?".to_string()]);
    }

    #[test]
    fn chunker_strips_code_and_markdown() {
        let mut chunker = SentenceChunker::new(400);
        let sentences = chunker.push("Here is code:\n```rust\nlet x = 1;\n```\nAll done!");
        assert_eq!(sentences, vec!["Here is code:".to_string(), "All done!".to_string()]);
    }

    #[test]
    fn chunker_hard_splits_long_text() {
        let mut chunker = SentenceChunker::new(50);
        let sentences = chunker.push(&"word ".repeat(30));
        assert!(!sentences.is_empty());
        assert!(sentences.iter().all(|s| s.chars().count() <= 60));
    }

    #[test]
    fn chunker_flush_returns_remainder() {
        let mut chunker = SentenceChunker::new(200);
        assert!(chunker.push("partial sentence").is_empty());
        assert_eq!(chunker.flush(), Some("partial sentence".to_string()));
        assert_eq!(chunker.flush(), None);
    }

    #[test]
    fn speed_commands_parse() {
        assert_eq!(
            parse_speed_command("speak faster", 1.0),
            Some((SpeedChange::Faster, 1.25))
        );
        assert_eq!(
            parse_speed_command("Slow down please!", 1.5),
            Some((SpeedChange::Slower, 1.2))
        );
        assert_eq!(
            parse_speed_command("normal speed", 1.3),
            Some((SpeedChange::Normal, 1.0))
        );
        assert_eq!(
            parse_speed_command("set speed 150%", 1.0),
            Some((SpeedChange::Set(1.5), 1.5))
        );
        assert_eq!(parse_speed_command("tell me about faster horses", 1.0), None);
    }

    #[test]
    fn wav_header_is_valid() {
        let wav = write_wav(&[0u8; 320], 16000);
        assert_eq!(&wav[0..4], b"RIFF");
        assert_eq!(&wav[8..12], b"WAVE");
        assert_eq!(wav.len(), 44 + 320);
        assert_eq!(u32::from_le_bytes([wav[24], wav[25], wav[26], wav[27]]), 16000);
        assert_eq!(u32::from_le_bytes([wav[40], wav[41], wav[42], wav[43]]), 320);
    }

    #[test]
    fn sample_rate_reads_config() {
        let rate = sample_rate_from_model("/home/goitseone/piper-voices/en_US-lessac-medium.onnx");
        assert_eq!(rate, Some(22050));
        assert_eq!(sample_rate_from_model("/nope/missing.onnx"), None);
    }
}
