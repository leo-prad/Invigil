use serde::{Deserialize, Serialize};

/// Info about the currently focused window.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct WindowInfo {
    pub title: String,
    pub process_name: String,
    pub exe_path: String,
}

// ─── Windows implementation ──────────────────────────────────────────

#[cfg(target_os = "windows")]
pub fn get_active_window() -> WindowInfo {
    use std::ffi::OsString;
    use std::os::windows::ffi::OsStringExt;
    use windows::Win32::Foundation::HWND;
    use windows::Win32::UI::WindowsAndMessaging::{
        GetForegroundWindow, GetWindowTextW, GetWindowThreadProcessId,
    };
    use windows::Win32::System::Threading::{
        OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
    };
    use windows::Win32::System::ProcessStatus::GetModuleFileNameExW;

    unsafe {
        let hwnd = GetForegroundWindow();
        if hwnd == HWND::default() {
            return WindowInfo::default();
        }

        // Window title
        let mut title_buf = [0u16; 512];
        let len = GetWindowTextW(hwnd, &mut title_buf);
        let title = if len > 0 {
            OsString::from_wide(&title_buf[..len as usize])
                .to_string_lossy()
                .into_owned()
        } else {
            String::new()
        };

        // Process name from PID
        let mut pid = 0u32;
        GetWindowThreadProcessId(hwnd, Some(&mut pid));
        let (process_name, exe_path) = if pid != 0 {
            if let Ok(handle) = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) {
                let mut path_buf = [0u16; 1024];
                let path_len = GetModuleFileNameExW(handle, None, &mut path_buf);
                let _ = windows::Win32::Foundation::CloseHandle(handle);
                if path_len > 0 {
                    let full = OsString::from_wide(&path_buf[..path_len as usize])
                        .to_string_lossy()
                        .into_owned();
                    let name = full
                        .rsplit('\\')
                        .next()
                        .unwrap_or(&full)
                        .to_string();
                    (name, full)
                } else {
                    (String::new(), String::new())
                }
            } else {
                (String::new(), String::new())
            }
        } else {
            (String::new(), String::new())
        };

        crate::appicon::remember(&process_name, &exe_path);
        WindowInfo { title, process_name, exe_path }
    }
}

// ─── Non-Windows stub (for compilation on macOS/Linux) ───────────────

#[cfg(not(target_os = "windows"))]
pub fn get_active_window() -> WindowInfo {
    // Stub for non-Windows builds — returns empty info.
    // In production this only runs on Windows where the real impl above is used.
    WindowInfo::default()
}

/// Extract the likely "app name" from a WindowInfo.
pub fn extract_app_name(window: &WindowInfo) -> String {
    let proc_lower = window.process_name.to_lowercase();
    if proc_lower.contains("antigravity") {
        return "Antigravity".to_string();
    }
    if proc_lower.contains("chrome") {
        return "Chrome".to_string();
    }
    if proc_lower.contains("code") {
        return "VS Code".to_string();
    }
    if proc_lower.contains("discord") {
        return "Discord".to_string();
    }
    if proc_lower.contains("invigil") {
        return "Invigil".to_string();
    }
    if proc_lower.contains("snipping") {
        return "Snipping Tool".to_string();
    }

    // Try common separators in title
    for sep in &[" — ", " - ", " | ", " · "] {
        if let Some(idx) = window.title.find(sep) {
            let left = window.title[..idx].trim();
            if !left.is_empty() {
                return left.to_string();
            }
        }
    }

    if !window.process_name.is_empty() {
        return window.process_name.replace(".exe", "");
    }

    window.title.clone()
}

/// Extract the detail/subtitle from a WindowInfo.
///
/// Browsers (Chrome, Edge, Firefox) put the tab title *before* the browser name:
/// `"Attention Span - YouTube - Google Chrome"`. The generic left→right split
/// grabs the browser name as the detail and drops the actual tab — the roast overlay
/// then reads "Chrome — Google Chrome" and can't do anything contextual.
/// Browser-specific rule: strip the browser suffix from the end and keep everything
/// before it. Falls back to the generic rule for everything else.
pub fn extract_detail(window: &WindowInfo) -> String {
    let proc_lower = window.process_name.to_lowercase();
    if proc_lower.contains("antigravity") {
        return window.title.clone();
    }

    // Browser suffix stripping — checked longest-first so multi-word suffixes win.
    let browser_suffixes: &[&str] = &[
        " - Google Chrome", " – Google Chrome",
        " - Microsoft\u{200B}\u{200E} Edge", " - Microsoft Edge", " – Microsoft Edge",
        " — Mozilla Firefox", " - Mozilla Firefox",
        " - Brave", " – Brave",
        " and 1 more page - Personal - Microsoft\u{200B}\u{200E} Edge",
    ];
    let is_browser = proc_lower.contains("chrome")
        || proc_lower.contains("edge")
        || proc_lower.contains("firefox")
        || proc_lower.contains("brave");
    if is_browser {
        let title = &window.title;
        for suffix in browser_suffixes {
            if let Some(stripped) = title.strip_suffix(suffix) {
                let cleaned = stripped.trim();
                if !cleaned.is_empty() {
                    return cleaned.to_string();
                }
            }
        }
        // Fallback for browser windows the suffix table missed: keep everything
        // before the LAST " - " separator (tab titles can themselves contain " - ").
        if let Some(idx) = title.rfind(" - ") {
            let left = title[..idx].trim();
            if !left.is_empty() {
                return left.to_string();
            }
        }
        return title.clone();
    }

    for sep in &[" — ", " - ", " | ", " · "] {
        if let Some(idx) = window.title.find(sep) {
            let right = window.title[idx + sep.len()..].trim();
            if !right.is_empty() {
                return right.to_string();
            }
        }
    }
    window.title.clone()
}

#[cfg(target_os = "windows")]
pub fn get_idle_seconds() -> u64 {
    use windows::Win32::UI::Input::KeyboardAndMouse::{GetLastInputInfo, LASTINPUTINFO};
    use windows::Win32::System::SystemInformation::GetTickCount;

    unsafe {
        let mut plii = LASTINPUTINFO {
            cbSize: std::mem::size_of::<LASTINPUTINFO>() as u32,
            dwTime: 0,
        };
        if GetLastInputInfo(&mut plii).as_bool() {
            let now = GetTickCount();
            if now >= plii.dwTime {
                return ((now - plii.dwTime) / 1000) as u64;
            }
        }
        0
    }
}

#[cfg(not(target_os = "windows"))]
pub fn get_idle_seconds() -> u64 {
    0
}

/// A small foreground screenshot, encoded without ever creating a disk file.
#[cfg(windows)]
pub fn capture_foreground() -> Option<String> {
    use base64::Engine;
    use windows::Win32::{Foundation::RECT, Graphics::Gdi::*, UI::WindowsAndMessaging::*};
    unsafe {
        let hwnd = GetForegroundWindow();
        let mut rect = RECT::default();
        GetWindowRect(hwnd, &mut rect).ok()?;
        let (sw, sh) = (rect.right - rect.left, rect.bottom - rect.top);
        if sw <= 0 || sh <= 0 { return None; }
        let scale = (1280.0 / sw as f64).min(900.0 / sh as f64).min(1.0);
        let (w,h) = ((sw as f64 * scale) as i32, (sh as f64 * scale) as i32);
        let screen = GetDC(None);
        if screen.is_invalid() { return None; }
        let dc = CreateCompatibleDC(screen);
        let bitmap = CreateCompatibleBitmap(screen, w, h);
        if dc.is_invalid() || bitmap.is_invalid() {
            if !bitmap.is_invalid() { let _ = DeleteObject(bitmap); }
            if !dc.is_invalid() { let _ = DeleteDC(dc); }
            ReleaseDC(None, screen);
            return None;
        }
        let old = SelectObject(dc, bitmap);
        let copied = StretchBlt(dc, 0, 0, w, h, screen, rect.left, rect.top, sw, sh, SRCCOPY).as_bool();
        SelectObject(dc, old);
        let mut pixels = vec![0u8; w as usize * h as usize * 4];
        let mut info = BITMAPINFO::default();
        info.bmiHeader.biSize = std::mem::size_of::<BITMAPINFOHEADER>() as u32;
        info.bmiHeader.biWidth = w;
        info.bmiHeader.biHeight = -h;
        info.bmiHeader.biPlanes = 1;
        info.bmiHeader.biBitCount = 32;
        let rows = if copied { GetDIBits(screen, bitmap, 0, h as u32, Some(pixels.as_mut_ptr().cast()), &mut info, DIB_RGB_COLORS) } else { 0 };
        let _ = DeleteObject(bitmap);
        let _ = DeleteDC(dc);
        ReleaseDC(None, screen);
        if rows != h { return None; }
        for px in pixels.chunks_exact_mut(4) { px.swap(0,2); px[3] = 255; }
        let mut png_bytes = Vec::new();
        {
            let mut enc = png::Encoder::new(&mut png_bytes, w as u32, h as u32);
            enc.set_color(png::ColorType::Rgba);
            enc.set_depth(png::BitDepth::Eight);
            enc.write_header().ok()?.write_image_data(&pixels).ok()?;
        }
        Some(base64::engine::general_purpose::STANDARD.encode(png_bytes))
    }
}
#[cfg(not(windows))]
pub fn capture_foreground() -> Option<String> { None }

/// A single bounded worker protects the session thread from hung accessibility providers.
/// Text is transient classification input, never returned to history or the frontend.
#[cfg(windows)]
pub fn foreground_context() -> String {
    use std::sync::{mpsc, OnceLock};
    type Request = (isize, mpsc::SyncSender<String>);
    static WORKER: OnceLock<mpsc::SyncSender<Request>> = OnceLock::new();
    let worker = WORKER.get_or_init(|| {
        let (tx, rx) = mpsc::sync_channel::<Request>(0);
        std::thread::spawn(move || {
            use windows::Win32::{System::Com::*, UI::Accessibility::*};
            unsafe {
                if CoInitializeEx(None, COINIT_MULTITHREADED).is_err() { return; }
                while let Ok((hwnd, reply)) = rx.recv() {
                    let text = (|| -> windows::core::Result<String> {
                        let automation: IUIAutomation = CoCreateInstance(&CUIAutomation, None, CLSCTX_INPROC_SERVER)?;
                        let root = automation.ElementFromHandle(windows::Win32::Foundation::HWND(hwnd as *mut _))?;
                        let walker = automation.ControlViewWalker()?;
                        let mut stack = vec![(root, 0usize)];
                        let mut text = String::new();
                        let start = std::time::Instant::now();
                        let mut visited = 0;
                        while let Some((node, depth)) = stack.pop() {
                            if visited >= 180 || text.len() >= 6000 || start.elapsed().as_millis() > 250 { break; }
                            visited += 1;
                            // Queue siblings before skipping hidden/password nodes, otherwise one
                            // hidden child truncates the remaining visible controls in its parent.
                            // Never follow root siblings, which belong to other app windows.
                            if depth > 0 {
                                if let Ok(next) = walker.GetNextSiblingElement(&node) { stack.push((next, depth)); }
                            }
                            if node.CurrentIsOffscreen().map(|v| v.as_bool()).unwrap_or(true)
                                || node.CurrentIsPassword().map(|v| v.as_bool()).unwrap_or(true) { continue; }
                            if let Ok(name) = node.CurrentName() {
                                let name = name.to_string();
                                if !name.trim().is_empty() {
                                    text.extend(name.chars().take(300)); text.push('\n');
                                }
                            }
                            if depth < 12 {
                                if let Ok(child) = walker.GetFirstChildElement(&node) { stack.push((child, depth + 1)); }
                            }
                        }
                        Ok(text)
                    })().unwrap_or_default();
                    let _ = reply.send(text);
                }
                CoUninitialize();
            }
        });
        tx
    });
    let hwnd = unsafe { windows::Win32::UI::WindowsAndMessaging::GetForegroundWindow() };
    let (tx, rx) = mpsc::sync_channel(1);
    if worker.try_send((hwnd.0 as isize, tx)).is_err() { return String::new(); }
    rx.recv_timeout(std::time::Duration::from_millis(400)).unwrap_or_default()
}
#[cfg(not(windows))]
pub fn foreground_context() -> String { String::new() }

#[cfg(all(test, windows))]
mod observation_tests {
    #[test]
    fn foreground_capture_is_an_in_memory_png() {
        use base64::Engine;
        let image = super::capture_foreground().expect("visible foreground window");
        let bytes = base64::engine::general_purpose::STANDARD.decode(image).unwrap();
        assert!(bytes.starts_with(b"\x89PNG\r\n\x1a\n"));
        let decoder = png::Decoder::new(std::io::Cursor::new(bytes));
        let reader = decoder.read_info().unwrap();
        assert!(reader.info().width <= 1280 && reader.info().height <= 900);
    }
    #[test]
    #[ignore = "requires local Ollama vision model and a visible desktop"]
    fn local_vision_accepts_foreground_capture() {
        let image = super::capture_foreground().expect("foreground screenshot");
        let window = super::get_active_window();
        let answer = crate::llm::classify_observation("Review photosynthesis", "I am studying the chloroplast reactions for my biology exam", &window.process_name, &window.title, &super::foreground_context(), Some(&image));
        assert!(answer.is_some(), "local model must accept PNG image and return a verdict");
    }
}
