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

/// Send a prompt to the local Ollama /api/generate endpoint and return the response
/// text. Returns None if Ollama isn't reachable, the request fails, or the response
/// can't be parsed. Handles both chunked and non-chunked HTTP responses.
fn ollama_generate(prompt: &str, num_predict: i32) -> Option<String> {
    let body = serde_json::json!({
        "model": MODEL,
        "prompt": prompt,
        "stream": false,
        "options": { "temperature": 0.0, "num_predict": num_predict }
    });
    let body_bytes = serde_json::to_vec(&body).ok()?;

    let mut stream = TcpStream::connect_timeout(&OLLAMA_HOST.parse().ok()?, TIMEOUT).ok()?;
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

    let mut buf = Vec::with_capacity(4096);
    stream.read_to_end(&mut buf).ok()?;
    let raw = String::from_utf8_lossy(&buf);

    let json_start = raw.find("\r\n\r\n").map(|i| i + 4)
        .or_else(|| raw.find("\n\n").map(|i| i + 2))?;
    let json_str = &raw[json_start..];

    let json_clean = if json_str.starts_with(|c: char| c.is_ascii_hexdigit()) {
        json_str.lines().nth(1).unwrap_or(json_str)
    } else {
        json_str
    };

    let resp: OllamaResponse = serde_json::from_str(json_clean.trim()).ok()?;
    Some(resp.response)
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
    let response = ollama_generate(&prompt, 12)?;
    let answer = response.trim().to_lowercase();

    if answer.starts_with("on_task") || answer.starts_with("on task") || answer.starts_with("yes") {
        Some(LlmVerdict::OnTask)
    } else if answer.starts_with("off_task") || answer.starts_with("off task") || answer.starts_with("no") {
        Some(LlmVerdict::OffTask)
    } else {
        log::warn!("LLM returned ambiguous answer: {}", response);
        None
    }
}

/// Verdict + reason from the AI when the user submits a "This is actually work" justification.
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "snake_case", tag = "kind", content = "reason")]
pub enum JustifyVerdict {
    /// The AI thinks the reason plausibly connects the current window to the committed task.
    Plausible,
    /// The AI thinks the reason is unrelated, made-up, or an obvious dodge. Carries a
    /// short specific reason to show the user (like the goal-validation flow).
    Implausible(String),
}

/// Ask the AI whether a user's justification for "this is actually work" plausibly
/// connects the current window to their committed task, AND — when it doesn't — get a
/// short one-line reason. Returns None if Ollama is unavailable (caller treats that as
/// "give the benefit of the doubt").
pub fn validate_work_justification(
    goal: &str,
    description: &str,
    app_name: &str,
    window_title: &str,
    reason: &str,
) -> Option<JustifyVerdict> {
    let prompt = format!(
        "You are a strict, skeptical focus-tracking assistant. A student is trying to convince you \
         that a window flagged as off-task is actually work for their session. Your default assumption \
         is that they are lying — flip to PLAUSIBLE only when the reason clearly, specifically, and \
         verifiably connects THIS window to THIS task.\n\n\
         Student's committed task: \"{goal}\"\n\
         Task description: \"{description}\"\n\n\
         Currently-active window:\n\
         App: {app_name}\n\
         Title: {window_title}\n\n\
         Student's stated reason it's actually work:\n\
         \"{reason}\"\n\n\
         RULES — a reason is IMPLAUSIBLE if ANY of these apply:\n\
         - It's fewer than about 5 words or reads as filler (\"cuz\", \"just because\", \"I need to\", \
           \"trust me\", \"it's related\", \"yeah\").\n\
         - It's about the student's feelings, mood, needing a break, or wanting to relax — those are \
           reasons to STOP working, not reasons this is work.\n\
         - It's generic and could apply to literally any window (\"I use this all the time\", \"for \
           research\", \"it helps me focus\") without naming a specific concrete tie-in.\n\
         - It contradicts what the window title actually says (claiming YouTube is a lecture when \
           the title is a music video, claiming Discord is a study group when the channel is #memes).\n\
         - It doesn't mention any specific artifact tied to the committed task — a specific tool, a \
           teacher's name, a course code, a specific reference page, a specific tutorial, etc.\n\n\
         A reason is PLAUSIBLE only if it names something CONCRETE and SPECIFIC (a named tool, a \
         teacher's email, a specific reference page, a specific tutorial video) that clearly helps \
         the committed task. \"Break\" / \"relax\" / \"just because\" are never plausible reasons for \
         a work session.\n\n\
         Reply in EXACTLY this format on ONE line:\n\
         PLAUSIBLE\n\
         or\n\
         IMPLAUSIBLE: <one short sentence explaining why, addressing the student directly as \"you\">\n\n\
         If the reason is about wanting a break, resting, or relaxing, remind the student that the \
         \"Just a break\" button is for that — this dialog is only for claims that this window is \
         actual work.\n\n\
         Examples:\n\
         IMPLAUSIBLE: \"cuz\" isn't a reason — you need to actually name what this window helps with.\n\
         IMPLAUSIBLE: If you just want a break, click \"Just a break\" — no need to pretend Discord is math.\n\
         IMPLAUSIBLE: You said you're doing math but this is a music video, not a lecture.\n\
         PLAUSIBLE"
    );
    // 60 tokens is plenty for the verdict + a one-sentence reason.
    let response = ollama_generate(&prompt, 60)?;
    let trimmed = response.trim();
    let lower = trimmed.to_lowercase();

    if lower.starts_with("plausible") || lower.starts_with("yes") || lower.starts_with("valid") {
        return Some(JustifyVerdict::Plausible);
    }
    if lower.starts_with("implausible") || lower.starts_with("no") || lower.starts_with("invalid") {
        // Peel off the leading verdict word and the separator; keep what's after as the reason.
        // Handles: "IMPLAUSIBLE: reason", "IMPLAUSIBLE - reason", "implausible reason", plain "no reason".
        let after = trimmed
            .splitn(2, |c: char| c == ':' || c == '-' || c == '—')
            .nth(1)
            .map(|s| s.trim())
            .unwrap_or("");
        let reason_text = if after.is_empty() {
            // Model didn't give a reason — use a generic line so the UI still has something to render.
            "That doesn't specifically explain how this window helps your committed task.".to_string()
        } else {
            after.to_string()
        };
        return Some(JustifyVerdict::Implausible(reason_text));
    }
    log::warn!("LLM justification verdict was ambiguous: {}", response);
    None
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

/// Where the Ollama installer drops `ollama.exe` on Windows. Checked in order — first hit wins.
#[cfg(windows)]
fn find_ollama_binary() -> Option<std::path::PathBuf> {
    let candidates = [
        std::env::var("LOCALAPPDATA").ok().map(|p| format!("{}\\Programs\\Ollama\\ollama.exe", p)),
        std::env::var("ProgramFiles").ok().map(|p| format!("{}\\Ollama\\ollama.exe", p)),
        std::env::var("ProgramFiles(x86)").ok().map(|p| format!("{}\\Ollama\\ollama.exe", p)),
    ];
    for c in candidates.into_iter().flatten() {
        let p = std::path::PathBuf::from(&c);
        if p.exists() {
            return Some(p);
        }
    }
    None
}

#[cfg(not(windows))]
fn find_ollama_binary() -> Option<std::path::PathBuf> {
    // POSIX: look on PATH via `which`-style probe.
    for prefix in ["/usr/local/bin", "/usr/bin", "/opt/homebrew/bin"] {
        let p = std::path::PathBuf::from(prefix).join("ollama");
        if p.exists() {
            return Some(p);
        }
    }
    None
}

/// Three-state status the frontend uses to decide whether to auto-launch Ollama or
/// show an "install this for better classifications" banner.
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum OllamaStatus {
    Running,
    InstalledNotRunning,
    NotInstalled,
}

pub fn get_ollama_status() -> OllamaStatus {
    if is_ollama_available() {
        OllamaStatus::Running
    } else if find_ollama_binary().is_some() {
        OllamaStatus::InstalledNotRunning
    } else {
        OllamaStatus::NotInstalled
    }
}

/// Spawn `ollama serve` in the background. Returns Ok(true) if the process was launched
/// AND the API port became reachable within a short window, Ok(false) if we spawned but
/// it never came up, or Err if we couldn't find/spawn the binary at all.
pub fn try_launch_ollama() -> Result<bool, String> {
    let bin = find_ollama_binary().ok_or_else(|| "Ollama binary not found".to_string())?;

    #[cfg(windows)]
    let spawn_result = {
        use std::os::windows::process::CommandExt;
        // CREATE_NO_WINDOW = 0x08000000 — keeps a console flash from popping up.
        std::process::Command::new(&bin)
            .arg("serve")
            .creation_flags(0x08000000)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
    };
    #[cfg(not(windows))]
    let spawn_result = std::process::Command::new(&bin)
        .arg("serve")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn();

    if let Err(e) = spawn_result {
        return Err(format!("Failed to launch Ollama: {}", e));
    }

    // Poll for readiness for up to ~5 seconds.
    for _ in 0..10 {
        std::thread::sleep(Duration::from_millis(500));
        if is_ollama_available() {
            return Ok(true);
        }
    }
    Ok(false)
}

/// Verify if a goal text and description are a real study/work task (min 25 chars) using Gemma LLM.
/// Returns Ok("") if valid, or Ok("reason") if rejected.
pub fn validate_goal(goal: &str, description: &str) -> String {
    let clean_goal = goal.trim();
    let clean_desc = description.trim();

    if clean_goal.len() < 2 || clean_desc.len() < 25 {
        return "Description must be at least 25 characters.".into();
    }

    // Content moderation — reject inappropriate/illegal content
    let blocked = [
        "meth", "cocaine", "heroin", "fentanyl", "crack", "ecstasy", "mdma",
        "lsd", "ketamine", "pcp", "opioid", "amphetamine", "xanax", "molly",
        "weed", "marijuana", "shrooms", "mushrooms", "dmt", "opium",
        "fuck", "shit", "bitch", "dick", "porn", "hentai", "nsfw",
        "kill", "murder", "suicide", "bomb", "terrorism",
    ];
    let combined_lower = format!("{} {}", clean_goal, clean_desc).to_lowercase();
    let combined_words: Vec<&str> = combined_lower.split(|c: char| !c.is_alphanumeric()).collect();
    for word in &combined_words {
        if blocked.contains(word) {
            return "That doesn't look like a real study or work task.".into();
        }
    }

    // Quick heuristic check for keyboard spam
    let chars: Vec<char> = clean_desc.chars().collect();
    let all_same = chars.iter().all(|&c| c == chars[0]);
    if all_same {
        return "That looks like keyboard spam — describe your actual task.".into();
    }

    // Detect repeating short patterns (e.g. "sdcsdcsdcsdc", "ababababab")
    if is_repeating_pattern(&clean_desc.to_lowercase()) {
        return "That looks like a repeating pattern — describe your actual task.".into();
    }

    // Check for low unique-character ratio (e.g. "aaabbbcccaaabbb")
    let unique: std::collections::HashSet<char> = clean_desc.chars().filter(|c| c.is_alphabetic()).collect();
    let alpha_count = clean_desc.chars().filter(|c| c.is_alphabetic()).count();
    if alpha_count > 10 && unique.len() <= 4 {
        return "Description uses too few distinct letters — write a real sentence.".into();
    }

    // Check that it contains at least a few real words (spaces)
    let word_count = clean_desc.split_whitespace().count();
    if word_count < 3 {
        return "Write at least a short sentence describing your task.".into();
    }

    if !is_ollama_available() {
        return String::new();
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
        Err(_) => return String::new(),
    };

    let mut stream = match TcpStream::connect_timeout(&match OLLAMA_HOST.parse() { Ok(a) => a, Err(_) => return String::new() }, TIMEOUT) {
        Ok(s) => s,
        Err(_) => return String::new(),
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
        return String::new();
    }

    let mut buf = Vec::with_capacity(2048);
    if stream.read_to_end(&mut buf).is_err() {
        return String::new();
    }
    let raw = String::from_utf8_lossy(&buf);
    let json_start = match raw.find("\r\n\r\n").map(|i| i + 4).or_else(|| raw.find("\n\n").map(|i| i + 2)) {
        Some(i) => i,
        None => return String::new(),
    };
    let json_str = &raw[json_start..];
    let json_clean = if json_str.starts_with(|c: char| c.is_ascii_hexdigit()) {
        json_str.lines().nth(1).unwrap_or(json_str)
    } else {
        json_str
    };

    let resp: OllamaResponse = match serde_json::from_str(json_clean.trim()) {
        Ok(r) => r,
        Err(_) => return String::new(),
    };

    let ans = resp.response.trim().to_lowercase();
    if ans.contains("invalid") {
        "AI rejected this — it doesn't look like a real task description.".into()
    } else {
        String::new()
    }
}

/// Detect repeating short substrings (e.g. "sdc" repeated in "sdcsdcsdcsdc").
fn is_repeating_pattern(s: &str) -> bool {
    let bytes = s.as_bytes();
    let len = bytes.len();
    if len < 6 { return false; }
    for pat_len in 2..=6.min(len / 2) {
        let pat = &bytes[..pat_len];
        let repeats = bytes.chunks(pat_len).filter(|chunk| *chunk == pat).count();
        if repeats >= 3 && repeats * pat_len >= len - pat_len {
            return true;
        }
    }
    false
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
