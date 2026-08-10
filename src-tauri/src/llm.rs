//! Tier 1 — Local LLM classification via Ollama (Gemma 4B).
//!
//! Calls `http://localhost:11434/api/generate` synchronously.
//! If Ollama is unreachable the call returns `None`, and the
//! caller should fall back to the Tier 0 default.

use serde::{Deserialize, Serialize};
use std::io::Read;
use std::net::TcpStream;
use std::time::Duration;

const OLLAMA_HOST: &str = "127.0.0.1:11434";
const MODEL: &str = "gemma:e4b";
const TIMEOUT: Duration = Duration::from_secs(4);
const READ_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Debug, Clone, PartialEq)]
pub enum LlmVerdict {
    OnTask,
    OffTask,
}

/// Ask the local Gemma model whether the current window is on-task for the
/// given study goal and detailed description. Returns `None` if Ollama is unavailable or the model
/// can't decide.
pub fn classify_ambiguous(
    goal: &str,
    description: &str,
    app_name: &str,
    window_title: &str,
) -> Option<LlmVerdict> {
    let prompt = build_prompt(goal, description, app_name, window_title);
    let body = serde_json::json!({
        "model": MODEL,
        "prompt": prompt,
        "stream": false,
        "options": {
            "temperature": 0.0,
            "num_predict": 12
        }
    });
    let body_bytes = serde_json::to_vec(&body).ok()?;

    // Build a raw HTTP POST — avoids pulling in reqwest/ureq as a dependency
    let mut stream = TcpStream::connect_timeout(
        &OLLAMA_HOST.parse().ok()?,
        TIMEOUT,
    ).ok()?;
    stream.set_read_timeout(Some(READ_TIMEOUT)).ok()?;
    stream.set_write_timeout(Some(TIMEOUT)).ok()?;

    let request = format!(
        "POST /api/generate HTTP/1.1\r\n\
         Host: {OLLAMA_HOST}\r\n\
         Content-Type: application/json\r\n\
         Content-Length: {}\r\n\
         Connection: close\r\n\
         \r\n",
        body_bytes.len()
    );

    use std::io::Write;
    stream.write_all(request.as_bytes()).ok()?;
    stream.write_all(&body_bytes).ok()?;
    stream.flush().ok()?;

    // Read response
    let mut buf = Vec::with_capacity(4096);
    stream.read_to_end(&mut buf).ok()?;
    let raw = String::from_utf8_lossy(&buf);

    // Find the JSON body after the HTTP headers
    let json_start = raw.find("\r\n\r\n").map(|i| i + 4)
        .or_else(|| raw.find("\n\n").map(|i| i + 2))?;
    let json_str = &raw[json_start..];

    // Handle chunked transfer encoding — take the first chunk
    let json_clean = if json_str.starts_with(|c: char| c.is_ascii_hexdigit()) {
        json_str.lines().nth(1).unwrap_or(json_str)
    } else {
        json_str
    };

    let resp: OllamaResponse = serde_json::from_str(json_clean.trim()).ok()?;
    let answer = resp.response.trim().to_lowercase();

    if answer.starts_with("on_task") || answer.starts_with("on task") || answer.starts_with("yes") {
        Some(LlmVerdict::OnTask)
    } else if answer.starts_with("off_task") || answer.starts_with("off task") || answer.starts_with("no") {
        Some(LlmVerdict::OffTask)
    } else {
        log::warn!("LLM returned ambiguous answer: {}", resp.response);
        None
    }
}

/// Quick check if Ollama is reachable. Cached per-session to avoid spam.
pub fn is_ollama_available() -> bool {
    TcpStream::connect_timeout(
        &match OLLAMA_HOST.parse() {
            Ok(addr) => addr,
            Err(_) => return false,
        },
        Duration::from_millis(500),
    ).is_ok()
}

/// Verify if a goal text and description are a real study/work task (min 25 chars) using Gemma LLM.
pub fn validate_goal(goal: &str, description: &str) -> bool {
    let clean_goal = goal.trim();
    let clean_desc = description.trim();

    if clean_goal.len() < 2 || clean_desc.len() < 25 {
        return false;
    }
    
    // Quick heuristic check for keyboard spam (e.g. "asdfasdfasdfasdfasdfasdfasdf")
    let chars: Vec<char> = clean_desc.chars().collect();
    let all_same = chars.iter().all(|&c| c == chars[0]);
    if all_same {
        return false;
    }

    if !is_ollama_available() {
        return clean_desc.len() >= 25;
    }

    let prompt = format!(
        "Is the following task and description a coherent, meaningful work or study task?\n\
         Task: \"{clean_goal}\"\n\
         Description: \"{clean_desc}\"\n\n\
         Reply ONLY 'valid' if it describes a real task, or 'invalid' if it is random keyboard spam or nonsense."
    );

    let body = serde_json::json!({
        "model": MODEL,
        "prompt": prompt,
        "stream": false,
        "options": {
            "temperature": 0.0,
            "num_predict": 10
        }
    });

    let body_bytes = match serde_json::to_vec(&body) {
        Ok(b) => b,
        Err(_) => return true,
    };

    let mut stream = match TcpStream::connect_timeout(&match OLLAMA_HOST.parse() { Ok(a) => a, Err(_) => return true }, TIMEOUT) {
        Ok(s) => s,
        Err(_) => return true,
    };
    let _ = stream.set_read_timeout(Some(READ_TIMEOUT));
    let _ = stream.set_write_timeout(Some(TIMEOUT));

    let request = format!(
        "POST /api/generate HTTP/1.1\r\n\
         Host: {OLLAMA_HOST}\r\n\
         Content-Type: application/json\r\n\
         Content-Length: {}\r\n\
         Connection: close\r\n\
         \r\n",
        body_bytes.len()
    );

    use std::io::Write;
    if stream.write_all(request.as_bytes()).is_err() || stream.write_all(&body_bytes).is_err() || stream.flush().is_err() {
        return true;
    }

    let mut buf = Vec::with_capacity(2048);
    if stream.read_to_end(&mut buf).is_err() {
        return true;
    }
    let raw = String::from_utf8_lossy(&buf);
    let json_start = match raw.find("\r\n\r\n").map(|i| i + 4).or_else(|| raw.find("\n\n").map(|i| i + 2)) {
        Some(i) => i,
        None => return true,
    };
    let json_str = &raw[json_start..];
    let json_clean = if json_str.starts_with(|c: char| c.is_ascii_hexdigit()) {
        json_str.lines().nth(1).unwrap_or(json_str)
    } else {
        json_str
    };

    let resp: OllamaResponse = match serde_json::from_str(json_clean.trim()) {
        Ok(r) => r,
        Err(_) => return true,
    };

    let ans = resp.response.trim().to_lowercase();
    !ans.contains("invalid")
}

fn build_prompt(goal: &str, description: &str, app_name: &str, window_title: &str) -> String {
    format!(
        "You are a strict focus-tracking assistant.\n\
         Student's Task: \"{goal}\"\n\
         Task Description: \"{description}\"\n\n\
         Current Active Window:\n\
         App: {app_name}\n\
         Title: {window_title}\n\n\
         Based on the student's task description, is the current window relevant, productive, and on-task for this session?\n\
         Guidelines:\n\
         - If watching educational videos or lectures (e.g. YouTube calculus/coding tutorials) relevant to their task description, reply on_task.\n\
         - If watching unrelated entertainment, gaming videos, music videos, or social media, reply off_task.\n\
         - If using search tools, reference sites, or development apps related to their description, reply on_task.\n\n\
         Reply with ONLY 'on_task' or 'off_task'."
    )
}

#[derive(Deserialize, Serialize)]
struct OllamaResponse {
    response: String,
}
