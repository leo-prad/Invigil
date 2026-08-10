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
pub fn extract_detail(window: &WindowInfo) -> String {
    let proc_lower = window.process_name.to_lowercase();
    if proc_lower.contains("antigravity") {
        return window.title.clone();
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
