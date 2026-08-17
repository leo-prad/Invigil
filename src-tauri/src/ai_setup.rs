//! First-run AI onboarding: detect / install Ollama, pull gemma3:4b.
//!
//! All work runs on a background thread; progress is streamed to the
//! frontend via the `ai-setup-progress` event.

use serde::Serialize;
use std::io::{Read, Write};
use std::net::TcpStream;
use std::path::PathBuf;
use std::process::Command;
use std::time::Duration;
use tauri::{AppHandle, Emitter};

const OLLAMA_HOST: &str = "127.0.0.1:11434";
pub const MODEL: &str = "gemma3:4b";
const OLLAMA_INSTALLER_URL: &str = "https://ollama.com/download/OllamaSetup.exe";

#[derive(Debug, Clone, Serialize)]
pub struct AiStatus {
    pub setup_seen: bool,
    pub ai_enabled: bool,
    pub ollama_reachable: bool,
    pub model_present: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SetupEvent {
    Step { step: String, index: u8, total: u8 },
    Progress { percent: f64, transferred: u64, total: u64, note: String },
    Log { message: String },
    Done,
    Failed { reason: String },
}

fn emit(app: &AppHandle, ev: SetupEvent) {
    let _ = app.emit("ai-setup-progress", &ev);
}

// ─── Detection ────────────────────────────────────────────────────────

pub fn ollama_reachable() -> bool {
    let addr = match OLLAMA_HOST.parse() {
        Ok(a) => a,
        Err(_) => return false,
    };
    TcpStream::connect_timeout(&addr, Duration::from_millis(600)).is_ok()
}

pub fn model_present(model: &str) -> bool {
    let Some(body) = http_get("/api/tags", Duration::from_secs(2)) else {
        return false;
    };
    // Cheap contains — Ollama returns { models: [{name, ...}, ...] }
    body.contains(&format!("\"{model}\""))
}

// ─── Raw HTTP helpers (no HTTP crate on purpose) ──────────────────────

fn http_get(path: &str, timeout: Duration) -> Option<String> {
    let addr = OLLAMA_HOST.parse().ok()?;
    let mut stream = TcpStream::connect_timeout(&addr, timeout).ok()?;
    stream.set_read_timeout(Some(timeout)).ok()?;
    stream.set_write_timeout(Some(timeout)).ok()?;
    let req = format!(
        "GET {path} HTTP/1.1\r\nHost: {OLLAMA_HOST}\r\nConnection: close\r\n\r\n"
    );
    stream.write_all(req.as_bytes()).ok()?;
    let mut buf = Vec::with_capacity(4096);
    stream.read_to_end(&mut buf).ok()?;
    let raw = String::from_utf8_lossy(&buf).into_owned();
    let split = raw.find("\r\n\r\n").map(|i| i + 4)?;
    Some(raw[split..].to_string())
}

// ─── Ollama install (Windows) ─────────────────────────────────────────

fn temp_installer_path() -> PathBuf {
    let mut p = std::env::temp_dir();
    p.push("InvigilOllamaSetup.exe");
    p
}

fn download_ollama_installer(app: &AppHandle) -> Result<PathBuf, String> {
    let out = temp_installer_path();
    let out_str = out.to_string_lossy().to_string();
    emit(app, SetupEvent::Log {
        message: format!("Downloading Ollama installer to {out_str}…"),
    });

    // PowerShell: BITS-style download, quieter than Invoke-WebRequest for large files.
    // We fall back to Invoke-WebRequest with progress off if BITS is unavailable.
    let script = format!(
        "$ErrorActionPreference='Stop'; \
         $ProgressPreference='SilentlyContinue'; \
         try {{ Start-BitsTransfer -Source '{url}' -Destination '{dst}' }} \
         catch {{ Invoke-WebRequest -Uri '{url}' -OutFile '{dst}' -UseBasicParsing }}",
        url = OLLAMA_INSTALLER_URL,
        dst = out_str.replace('\'', "''"),
    );
    let status = Command::new("powershell")
        .args(["-NoProfile", "-NonInteractive", "-Command", &script])
        .status()
        .map_err(|e| format!("Failed to spawn PowerShell: {e}"))?;
    if !status.success() {
        return Err(format!("Installer download failed (exit {})", status.code().unwrap_or(-1)));
    }
    if !out.exists() {
        return Err("Installer download reported success but file is missing.".into());
    }
    Ok(out)
}

fn run_ollama_installer(app: &AppHandle, installer: &PathBuf) -> Result<(), String> {
    emit(app, SetupEvent::Log { message: "Running Ollama installer (silent)…".into() });
    // OllamaSetup.exe uses Inno Setup — /VERYSILENT hides UI; /NORESTART avoids reboot prompts.
    let status = Command::new(installer)
        .args(["/VERYSILENT", "/SUPPRESSMSGBOXES", "/NORESTART"])
        .status()
        .map_err(|e| format!("Failed to launch installer: {e}"))?;
    if !status.success() {
        return Err(format!("Ollama installer exited with code {}", status.code().unwrap_or(-1)));
    }
    Ok(())
}

fn wait_for_ollama(app: &AppHandle, max_secs: u64) -> Result<(), String> {
    emit(app, SetupEvent::Log { message: "Waiting for Ollama service to start…".into() });
    let start = std::time::Instant::now();
    while start.elapsed().as_secs() < max_secs {
        if ollama_reachable() {
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(750));
    }
    Err("Ollama installed but the service didn't respond within 60s.".into())
}

// ─── Model pull (streamed) ────────────────────────────────────────────

#[derive(serde::Deserialize)]
struct PullChunk {
    status: Option<String>,
    #[serde(default)] total: u64,
    #[serde(default)] completed: u64,
}

fn pull_model(app: &AppHandle, model: &str) -> Result<(), String> {
    emit(app, SetupEvent::Log { message: format!("Pulling {model}…") });

    let addr: std::net::SocketAddr = OLLAMA_HOST.parse()
        .map_err(|e| format!("bad host: {e}"))?;
    let mut stream = TcpStream::connect_timeout(&addr, Duration::from_secs(5))
        .map_err(|e| format!("Can't reach Ollama: {e}"))?;
    stream.set_read_timeout(Some(Duration::from_secs(120))).ok();
    stream.set_write_timeout(Some(Duration::from_secs(10))).ok();

    let body = format!("{{\"name\":\"{model}\",\"stream\":true}}");
    let req = format!(
        "POST /api/pull HTTP/1.1\r\n\
         Host: {OLLAMA_HOST}\r\n\
         Content-Type: application/json\r\n\
         Content-Length: {}\r\n\
         Connection: close\r\n\r\n",
        body.len()
    );
    stream.write_all(req.as_bytes()).map_err(|e| e.to_string())?;
    stream.write_all(body.as_bytes()).map_err(|e| e.to_string())?;
    stream.flush().ok();

    // Read the response body incrementally and parse each JSON line as it arrives.
    let mut buf = Vec::with_capacity(8192);
    let mut chunk = [0u8; 4096];
    let mut header_split: Option<usize> = None;
    let mut last_emitted_pct: f64 = -1.0;

    loop {
        let n = stream.read(&mut chunk).map_err(|e| format!("read error: {e}"))?;
        if n == 0 { break; }
        buf.extend_from_slice(&chunk[..n]);

        if header_split.is_none() {
            if let Some(idx) = find_subseq(&buf, b"\r\n\r\n") {
                header_split = Some(idx + 4);
            }
        }
        let Some(hs) = header_split else { continue };

        // Split body-so-far by newlines (JSON per line, sometimes chunked-encoded).
        // We greedily strip lines that parse as JSON.
        while let Some(nl) = memchr_nl(&buf[hs..]) {
            let line_end = hs + nl;
            let line = std::str::from_utf8(&buf[hs..line_end]).unwrap_or("").trim();
            // The trailing \n stays; we mutate buf below.
            let ate = line_end + 1 - hs;
            let raw = line.to_string();
            // Drop consumed bytes from buf (keep header prefix).
            buf.drain(hs..hs + ate);

            if raw.is_empty() || !raw.starts_with('{') {
                continue;
            }
            if let Ok(pc) = serde_json::from_str::<PullChunk>(&raw) {
                let note = pc.status.clone().unwrap_or_default();
                if pc.total > 0 {
                    let pct = (pc.completed as f64) / (pc.total as f64) * 100.0;
                    if (pct - last_emitted_pct).abs() >= 0.5 || pct >= 99.9 {
                        emit(app, SetupEvent::Progress {
                            percent: pct,
                            transferred: pc.completed,
                            total: pc.total,
                            note,
                        });
                        last_emitted_pct = pct;
                    }
                } else if !note.is_empty() {
                    emit(app, SetupEvent::Log { message: note });
                }
            }
        }
    }

    if !model_present(model) {
        return Err("Pull finished but model isn't listed by Ollama.".into());
    }
    Ok(())
}

fn find_subseq(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).position(|w| w == needle)
}
fn memchr_nl(hay: &[u8]) -> Option<usize> {
    hay.iter().position(|&b| b == b'\n')
}

// ─── Orchestrator ─────────────────────────────────────────────────────

pub fn run_install(app: AppHandle) {
    let steps = 4u8;
    let step = |name: &str, i: u8| SetupEvent::Step {
        step: name.into(), index: i, total: steps,
    };

    // Step 1: install Ollama (skip if already running)
    if ollama_reachable() {
        emit(&app, SetupEvent::Log {
            message: "Ollama already running — skipping installer.".into(),
        });
        emit(&app, step("Ollama already installed", 2));
    } else {
        emit(&app, step("Downloading Ollama installer", 1));
        let installer = match download_ollama_installer(&app) {
            Ok(p) => p,
            Err(e) => { emit(&app, SetupEvent::Failed { reason: e }); return; }
        };
        emit(&app, step("Installing Ollama", 2));
        if let Err(e) = run_ollama_installer(&app, &installer) {
            emit(&app, SetupEvent::Failed { reason: e });
            return;
        }
        if let Err(e) = wait_for_ollama(&app, 60) {
            emit(&app, SetupEvent::Failed { reason: e });
            return;
        }
    }

    // Step 3: pull model
    emit(&app, step("Pulling gemma3:4b", 3));
    if model_present(MODEL) {
        emit(&app, SetupEvent::Log { message: "Model already present.".into() });
    } else if let Err(e) = pull_model(&app, MODEL) {
        emit(&app, SetupEvent::Failed { reason: e });
        return;
    }

    // Step 4: done
    emit(&app, step("Finishing up", 4));
    emit(&app, SetupEvent::Done);
}
