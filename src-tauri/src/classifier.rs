use crate::db::Profile;
use crate::monitor::WindowInfo;

/// Classification result from the rule engine.
#[derive(Debug, Clone, PartialEq)]
pub enum Classification {
    OnTask,
    OffTask,
    Ambiguous,
}

/// Tier 0 rule engine — no AI, pure pattern matching.
/// Returns (Classification, tier_used=0, matched_pattern).
pub fn classify_tier0(window: &WindowInfo, profile: &Profile) -> (Classification, String) {
    let title_lower = window.title.to_lowercase();
    let process_lower = window.process_name.to_lowercase();

    // Empty window = idle / desktop
    if title_lower.is_empty() && process_lower.is_empty() {
        return (Classification::Ambiguous, String::new());
    }

    // Check allow list
    for pattern in &profile.allow_patterns {
        let pat = pattern.to_lowercase();
        if matches_pattern(&title_lower, &process_lower, &pat) {
            return (Classification::OnTask, pattern.clone());
        }
    }

    // Known always-work apps (universally educational — never entertainment)
    let system_work = [
        "canvas", "blackboard", "anki",
    ];
    for app in &system_work {
        if title_lower.contains(app) || process_lower.contains(app) {
            return (Classification::OnTask, app.to_string());
        }
    }

    // No blanket allow for utility apps (Explorer, Snipping Tool, Notepad...): they fall
    // through to Ambiguous and get judged against the goal. The 10s drift-onset grace in
    // session.rs already covers quick visits.

    // Known always-play apps (system-level denylist)
    let system_play = [
        "tiktok", "netflix", "hulu", "twitch.tv", "crunchyroll",
        "disney+", "hbo max", "prime video",
        "youtube", "reddit", "instagram", "twitter", "facebook",
        "snapchat", "pinterest", "tumblr", "9gag", "imgur",
        "spotify", "soundcloud", "pandora",
    ];
    for app in &system_play {
        if title_lower.contains(app) {
            return (Classification::OffTask, app.to_string());
        }
    }

    // Dual-use apps need LLM inspection against the session goal
    let dual_use = [
        "chrome", "firefox", "edge", "brave", "safari", "discord", "slack",
        "overleaf", "desmos", "notion", "obsidian",
        "drawboard", "onenote", "goodnotes", "xodo", "acrobat",
        "latex", "texstudio", "texmaker", "zotero", "antigravity",
        "code", "visual studio", "cursor", "pycharm", "intellij", "sublime",
        "chatgpt", "claude", "github", "stack overflow",
    ];
    for app in &dual_use {
        if process_lower.contains(app) || title_lower.contains(app) {
            // For browsers, the tab title is the window title — check against lists again
            // but with the tab title portion only
            return (Classification::Ambiguous, app.to_string());
        }
    }

    // Default: ambiguous — let Tier 1 handle it
    (Classification::Ambiguous, String::new())
}

/// Utility apps you pass through on the way to work (finding a file, taking a snip).
/// session.rs keeps them on task while the window keeps changing; see UTILITY_STILL_SEC.
pub fn is_utility_app(window: &WindowInfo) -> bool {
    let p = window.process_name.to_lowercase();
    [
        "explorer.exe", "snippingtool", "screenclippinghost", "taskmgr", "calc.exe",
        "calculatorapp", "systemsettings", "shellexperiencehost", "startmenuexperiencehost",
        "searchhost",
    ]
    .iter()
    .any(|a| p.contains(a))
}

/// AI chat assistants whose desktop window title never shows conversation content (just the
/// app name, e.g. "Claude" or "ChatGPT"). Tier 1's small local model tends to give these the
/// benefit of the doubt ("it's an AI assistant, could be helping with the task") even though
/// there is literally no title content to judge — so these are routed straight to the
/// deterministic keyword heuristic instead of the LLM, skipping the unreliable judgment call.
pub fn is_opaque_ai_assistant(window: &WindowInfo) -> bool {
    let process_lower = window.process_name.to_lowercase();
    let title_lower = window.title.to_lowercase();
    let opaque_ai_apps = ["claude", "chatgpt", "copilot", "gemini", "perplexity"];
    opaque_ai_apps.iter().any(|app| process_lower.contains(app) || title_lower.contains(app))
}

/// Simple pattern matching: checks if the pattern substring appears in
/// either the window title or the process name.
/// Supports a few special forms:
///   "Discord — #cs101"  →  must match both app and channel
///   "*.overleaf.com"    →  domain-style match in title
///   "code.exe"          →  process name match
fn matches_pattern(title: &str, process: &str, pattern: &str) -> bool {
    // "App — Detail" style pattern (e.g. "Discord — #cs101-study-group")
    if pattern.contains(" — ") || pattern.contains(" - ") {
        return title.contains(pattern);
    }

    // Process name match (e.g. "code.exe")
    if pattern.ends_with(".exe") {
        return process.contains(pattern);
    }

    // Domain-style match (e.g. "*.overleaf.com" or "overleaf.com")
    let clean = pattern.trim_start_matches("*.");
    if clean.contains('.') && !clean.contains(' ') {
        return title.contains(clean);
    }

    // Simple substring match
    title.contains(pattern) || process.contains(pattern)
}

/// Determine the "category" label for an interval (used for distraction stats).
/// Tries to pick the most recognizable app/site name.
pub fn categorize(window: &WindowInfo) -> String {
    let title = &window.title;
    let process = &window.process_name;

    // Well-known apps by process name
    let known = [
        ("discord", "Discord"),
        ("slack", "Slack"),
        ("code.exe", "VS Code"),
        ("code", "VS Code"),
        ("chrome", "Chrome"),
        ("firefox", "Firefox"),
        ("msedge", "Edge"),
        ("brave", "Brave"),
        ("spotify", "Spotify"),
    ];
    let proc_lower = process.to_lowercase();
    for (key, label) in &known {
        if proc_lower.contains(key) {
            // For browsers, prefer the site name from the title
            if ["chrome", "firefox", "msedge", "brave"].contains(key) {
                return extract_site_from_title(title).unwrap_or_else(|| label.to_string());
            }
            return label.to_string();
        }
    }

    // Fall back to extracting from title
    crate::monitor::extract_app_name(window)
}

/// Try to extract a recognizable site name from a browser tab title.
fn extract_site_from_title(title: &str) -> Option<String> {
    let lower = title.to_lowercase();
    let sites = [
        ("youtube", "YouTube"),
        ("reddit", "Reddit"),
        ("instagram", "Instagram"),
        ("twitter", "Twitter"),
        ("tiktok", "TikTok"),
        ("facebook", "Facebook"),
        ("khan academy", "Khan Academy"),
        ("overleaf", "Overleaf"),
        ("desmos", "Desmos"),
        ("canvas", "Canvas"),
        ("google docs", "Google Docs"),
        ("google sheets", "Google Sheets"),
        ("google slides", "Google Slides"),
        ("github", "GitHub"),
        ("stack overflow", "Stack Overflow"),
        ("wikipedia", "Wikipedia"),
        ("imessage", "iMessage web"),
        ("messages for web", "iMessage web"),
    ];
    for (key, label) in &sites {
        if lower.contains(key) {
            return Some(label.to_string());
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_profile() -> Profile {
        Profile {
            id: "test".into(),
            name: "Test".into(),
            allow_patterns: vec!["Overleaf".into(), "Desmos".into(), "Khan Academy".into()],
            deny_patterns: vec!["YouTube".into(), "Discord — #general".into(), "Instagram".into()],
        }
    }

    #[test]
    fn test_on_task() {
        let w = WindowInfo {
            title: "Overleaf — ch04_practice.tex".into(),
            process_name: "chrome.exe".into(),
            exe_path: String::new(),
        };
        let (cls, _) = classify_tier0(&w, &test_profile());
        assert_eq!(cls, Classification::OnTask);
    }

    #[test]
    fn test_off_task() {
        let w = WindowInfo {
            title: "YouTube — 3Blue1Brown".into(),
            process_name: "chrome.exe".into(),
            exe_path: String::new(),
        };
        let (cls, _) = classify_tier0(&w, &test_profile());
        assert_eq!(cls, Classification::OffTask);
    }

    #[test]
    fn test_ambiguous() {
        let w = WindowInfo {
            title: "Chrome — Some Random Page".into(),
            process_name: "chrome.exe".into(),
            exe_path: String::new(),
        };
        let (cls, _) = classify_tier0(&w, &test_profile());
        assert_eq!(cls, Classification::Ambiguous);
    }
}
