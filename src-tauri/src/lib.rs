mod bounties;
mod classifier;
mod db;
mod llm;
mod monitor;
mod points;
mod session;

use db::Database;
use parking_lot::Mutex;
use session::SessionManager;
use std::sync::Arc;
use tauri::{
    AppHandle, Emitter, Manager, PhysicalPosition, PhysicalSize,
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    WindowEvent,
};

// ─── App state ───────────────────────────────────────────────────────

struct AppState {
    db: Database,
    session_mgr: SessionManager,
    tick_handle: Mutex<Option<std::thread::JoinHandle<()>>>,
    tick_running: Arc<Mutex<bool>>,
}

// ─── Tauri commands ──────────────────────────────────────────────────

// --- Session commands ---

#[tauri::command]
fn start_session(
    state: tauri::State<'_, AppState>,
    app: AppHandle,
    goal: String,
    description: String,
    profile_id: Option<String>,
    duration_min: Option<i64>,
) -> Result<session::SessionState, String> {
    let session_state = state.session_mgr.start(goal, description, profile_id, duration_min);

    // Start the background monitoring tick loop (every 5 seconds)
    *state.tick_running.lock() = true;
    let running = Arc::clone(&state.tick_running);
    let app_clone = app.clone();

    let handle = std::thread::spawn(move || {
        while *running.lock() {
            std::thread::sleep(std::time::Duration::from_secs(5));
            if !*running.lock() {
                break;
            }
            
            // Execute tick on background thread to keep main UI loop smooth
            let app_state = app_clone.state::<AppState>();
            if let Some(result) = app_state.session_mgr.tick() {
                let _ = app_clone.emit("session-tick-result", &result);

                // Skip when the user is already parked on the overlay itself — the window is
                // visible, `showCyberOverlay` would re-run its shake / roast-line pick / justify
                // reset, which reads as a second warning stacking on top of the first.
                if result.overlay_active && !result.on_overlay {
                    // Re-shown/re-focused every off-task tick, not just the first — dismissing the
                    // overlay ("Just a break") without actually switching away brings it
                    // right back on the next ~5s poll instead of going quiet for the rest of the drift.
                    if let Some(drift_win) = app_clone.get_webview_window("drift_overlay") {
                        // Size overlay to cover the entire monitor
                        if let Ok(Some(monitor)) = drift_win.current_monitor() {
                            let pos = monitor.position();
                            let size = monitor.size();
                            let _ = drift_win.set_position(PhysicalPosition::new(pos.x, pos.y));
                            let _ = drift_win.set_size(PhysicalSize::new(size.width, size.height));
                        }
                        let _ = drift_win.show();
                        let _ = drift_win.set_always_on_top(true);
                        let _ = drift_win.set_focus();
                    }
                    let _ = app_clone.emit("drift-detected", serde_json::json!({
                        "app": result.drift_app,
                        "detail": result.drift_detail,
                        "elapsed_sec": result.state.elapsed_sec,
                        "drift_count": result.state.drift_count,
                    }));
                }

                if result.session_expired {
                    let _ = app_clone.emit("session-expired", ());
                }
            }
        }
    });

    *state.tick_handle.lock() = Some(handle);

    Ok(session_state)
}

#[tauri::command]
fn end_session(state: tauri::State<'_, AppState>, app: AppHandle) -> Result<session::SessionSummary, String> {
    *state.tick_running.lock() = false;
    let _ = state.tick_handle.lock().take();

    if let Some(drift_win) = app.get_webview_window("drift_overlay") {
        let _ = drift_win.hide();
    }

    let summary = state.session_mgr.stop();

    // Any accepted bounty may have flipped to `completed` after this session — recompute
    // so the UI shows a Claim button as soon as they open Bounties again.
    let today = bounties::local_today();
    bounties::ensure_today_pool(&state.db);
    bounties::refresh_progress(&state.db, &today);

    Ok(summary)
}

#[tauri::command]
fn hide_drift_overlay(app: AppHandle, state: tauri::State<'_, AppState>) -> Result<(), String> {
    state.session_mgr.snooze_drift();
    if let Some(drift_win) = app.get_webview_window("drift_overlay") {
        let _ = drift_win.hide();
    }
    Ok(())
}

#[tauri::command]
fn session_tick(
    state: tauri::State<'_, AppState>,
) -> Result<Option<session::TickResult>, String> {
    Ok(state.session_mgr.tick())
}

#[tauri::command]
fn get_session_state(state: tauri::State<'_, AppState>) -> Result<session::SessionState, String> {
    Ok(state.session_mgr.get_state())
}

#[tauri::command]
fn correct_classification(
    state: tauri::State<'_, AppState>,
    new_status: String,
) -> Result<(), String> {
    state.session_mgr.correct_current(&new_status);
    Ok(())
}

#[tauri::command]
fn allow_app_this_session(state: tauri::State<'_, AppState>, app: String) -> Result<(), String> {
    state.session_mgr.allow_app_for_session(&app);
    Ok(())
}

// --- Dashboard commands ---

#[tauri::command]
fn get_dashboard_data(
    state: tauri::State<'_, AppState>,
) -> Result<DashboardData, String> {
    let db = &state.db;
    let streak = db.get_streak_info().unwrap_or(db::StreakInfo {
        current: 0,
        best: 0,
        month_total_minutes: 0,
    });
    let recent = db.get_recent_sessions(5).unwrap_or_default();
    let weekly_min = db.get_weekly_focus_minutes().unwrap_or(0);
    let distractions = db.get_week_distraction_stats().unwrap_or_default();
    let trend = db.get_attention_trend(14).unwrap_or_default();
    let points = db.get_total_points().unwrap_or(0);
    let today = chrono::Utc::now().format("%Y-%m-%d").to_string();
    let today_stats = db.get_day_stats(&today).unwrap_or(db::DayStats {
        date: today,
        total_minutes: 0,
        on_task_pct: 0.0,
        session_count: 0,
        points: 0,
    });

    Ok(DashboardData {
        streak,
        recent_sessions: recent,
        weekly_focus_minutes: weekly_min,
        distractions,
        trend_14d: trend,
        total_points: points,
        today: today_stats,
    })
}

#[derive(serde::Serialize)]
struct DashboardData {
    streak: db::StreakInfo,
    recent_sessions: Vec<db::Session>,
    weekly_focus_minutes: i64,
    distractions: Vec<db::DistractionStat>,
    trend_14d: Vec<db::DayStats>,
    total_points: i64,
    today: db::DayStats,
}

#[tauri::command]
fn get_day_stats(
    state: tauri::State<'_, AppState>,
    date: String,
) -> Result<db::DayStats, String> {
    state.db.get_day_stats(&date).map_err(|e| e.to_string())
}

#[tauri::command]
fn get_sessions_in_range(
    state: tauri::State<'_, AppState>,
    start: String,
    end: String,
) -> Result<Vec<db::Session>, String> {
    state.db.get_sessions_in_range(&start, &end).map_err(|e| e.to_string())
}

#[tauri::command]
fn rename_session(
    state: tauri::State<'_, AppState>,
    id: String,
    goal: String,
) -> Result<(), String> {
    state.db.update_session_goal(&id, &goal).map_err(|e| e.to_string())
}

#[tauri::command]
fn get_top_tools(state: tauri::State<'_, AppState>) -> Result<Vec<String>, String> {
    state.db.get_top_user_tools().map_err(|e| e.to_string())
}

// --- Profile commands ---

#[tauri::command]
fn get_profiles(state: tauri::State<'_, AppState>) -> Result<Vec<db::Profile>, String> {
    state.db.get_profiles().map_err(|e| e.to_string())
}

#[tauri::command]
fn save_profile(
    state: tauri::State<'_, AppState>,
    profile: db::Profile,
) -> Result<(), String> {
    state.db.upsert_profile(&profile).map_err(|e| e.to_string())
}

#[tauri::command]
fn delete_profile(
    state: tauri::State<'_, AppState>,
    id: String,
) -> Result<(), String> {
    state.db.delete_profile(&id).map_err(|e| e.to_string())
}

// --- Settings commands ---

#[tauri::command]
fn get_settings(state: tauri::State<'_, AppState>) -> Result<db::Settings, String> {
    state.db.get_all_settings().map_err(|e| e.to_string())
}

#[tauri::command]
fn update_setting(
    state: tauri::State<'_, AppState>,
    key: String,
    value: String,
) -> Result<(), String> {
    state.db.set_setting(&key, &value).map_err(|e| e.to_string())
}

// --- Points & avatar commands ---

#[tauri::command]
fn get_points_overview(state: tauri::State<'_, AppState>) -> Result<points::PointsOverview, String> {
    Ok(points::get_points_overview(&state.db))
}

#[tauri::command]
fn get_shop_items(state: tauri::State<'_, AppState>) -> Result<Vec<points::ShopItem>, String> {
    Ok(points::get_shop_items(&state.db))
}

#[tauri::command]
fn unlock_avatar(
    state: tauri::State<'_, AppState>,
    id: String,
) -> Result<bool, String> {
    state.db.unlock_avatar(&id).map_err(|e| e.to_string())
}

#[tauri::command]
fn equip_avatar(
    state: tauri::State<'_, AppState>,
    id: String,
) -> Result<(), String> {
    state.db.equip_avatar(&id).map_err(|e| e.to_string())
}

// --- Monitor commands ---

#[tauri::command]
fn get_active_window() -> Result<monitor::WindowInfo, String> {
    Ok(monitor::get_active_window())
}

#[tauri::command]
fn validate_goal(goal: String, description: String) -> Result<String, String> {
    Ok(llm::validate_goal(&goal, &description))
}

// --- Ollama commands ---

#[tauri::command]
fn get_ollama_status() -> Result<llm::OllamaStatus, String> {
    Ok(llm::get_ollama_status())
}

#[tauri::command]
fn try_launch_ollama() -> Result<bool, String> {
    llm::try_launch_ollama()
}

// --- Justification command ---

/// Cheap deterministic checks the AI has been observed to miss on smaller models. Returns
/// a rejection line if the reason is obviously not-a-reason (too short, filler-only, or an
/// admission that it's a break rather than work). None means "worth sending to the AI."
fn shallow_reason_check(reason: &str) -> Option<&'static str> {
    let words: Vec<&str> = reason
        .split_whitespace()
        .filter(|w| !w.is_empty())
        .collect();
    if words.len() < 4 {
        return Some("That's not a real reason — write at least a full sentence explaining how this window helps your task.");
    }
    if reason.chars().filter(|c| c.is_alphabetic()).count() < 12 {
        return Some("That's too short. Explain what this window is helping you do.");
    }
    let lower = reason.to_lowercase();
    // "I want / need a break" is admitting it's not work — the whole point of the flow is
    // that this IS work, so a break-admission rejects itself.
    const BREAK_PATTERNS: &[&str] = &[
        "a break", "take a break", "taking a break", "need a break", "want a break",
        "want a rest", "chill for", "relax for",
    ];
    if BREAK_PATTERNS.iter().any(|p| lower.contains(p)) {
        return Some("Breaks aren't work. If you need one, that's fine — click \"Just a break\" and take it, don't fake a reason.");
    }
    // Pure-filler shorthand. Anything that reads like a shrug rather than an explanation.
    const FILLER_ONLY: &[&str] = &[
        "cuz", "cause", "because", "coz", "just because", "just cuz", "just cause",
        "trust me", "it's related", "its related", "yeah", "yes", "sure",
        "i don't know", "idk", "dunno",
    ];
    let stripped = lower.trim_end_matches(|c: char| !c.is_alphanumeric());
    if FILLER_ONLY.iter().any(|f| stripped == *f) {
        return Some("That's not an explanation. Name what specifically about this window helps your task.");
    }
    None
}

/// Outcome of running a "this is actually work" justification through the local AI.
#[derive(serde::Serialize)]
struct JustificationOutcome {
    /// "accepted" (AI approved), "rejected" (AI called BS), "no_ai" (Ollama unreachable —
    /// fell back to accepting).
    verdict: String,
    /// Snark line shown to the user on rejection. None on other verdicts.
    message: Option<String>,
}


/// Run the user's "this is actually work" text through the local AI. If plausible (or the
/// AI is unavailable), apply the correction + allowlist server-side so the frontend
/// doesn't have to make three separate calls. If implausible, save nothing and return a
/// snarky rejection line for the overlay to display.
#[tauri::command]
fn submit_work_justification(
    state: tauri::State<'_, AppState>,
    reason: String,
) -> Result<JustificationOutcome, String> {
    let reason_trimmed = reason.trim().to_string();
    if reason_trimmed.is_empty() {
        return Ok(JustificationOutcome {
            verdict: "rejected".into(),
            message: Some("You have to actually explain. \"Trust me\" isn't a reason.".into()),
        });
    }

    // Cheap pre-checks the AI has been observed to miss on smaller models: reject reasons
    // that are too short, filler-only, or "I want a break." No round trip needed.
    if let Some(reject) = shallow_reason_check(&reason_trimmed) {
        return Ok(JustificationOutcome { verdict: "rejected".into(), message: Some(reject.into()) });
    }

    let session_state = state.session_mgr.get_state();
    if !session_state.active {
        return Ok(JustificationOutcome { verdict: "accepted".into(), message: None });
    }

    // Snapshot the window the user is defending — the same one that triggered the drift.
    let window = monitor::get_active_window();
    let app_name = monitor::extract_app_name(&window);

    let verdict = llm::validate_work_justification(
        &session_state.goal,
        &session_state.description,
        &app_name,
        &window.title,
        &reason_trimmed,
    );

    match verdict {
        Some(llm::JustifyVerdict::Implausible(why)) => {
            // Don't save, don't allowlist — the specific `why` line is what the AI told the
            // user to their face; forwarded so the overlay renders it under the textarea
            // exactly like the goal-description validator on the start-session modal.
            Ok(JustificationOutcome {
                verdict: "rejected".into(),
                message: Some(why),
            })
        }
        Some(llm::JustifyVerdict::Plausible) => {
            state.session_mgr.correct_current("on_task");
            state.session_mgr.allow_app_for_session(&app_name);
            state.session_mgr.record_work_justification(&reason_trimmed);
            Ok(JustificationOutcome { verdict: "accepted".into(), message: None })
        }
        None => {
            // AI unreachable — no way to verify, so give the benefit of the doubt.
            state.session_mgr.correct_current("on_task");
            state.session_mgr.allow_app_for_session(&app_name);
            state.session_mgr.record_work_justification(&reason_trimmed);
            Ok(JustificationOutcome { verdict: "no_ai".into(), message: None })
        }
    }
}

// --- Bounty commands ---

#[derive(serde::Serialize)]
struct BountiesPayload {
    bounties: Vec<db::Bounty>,
    /// Local seconds remaining until midnight, when the pool refreshes.
    seconds_until_refresh: i64,
}

#[tauri::command]
fn get_bounties(state: tauri::State<'_, AppState>) -> Result<BountiesPayload, String> {
    let today = bounties::local_today();
    bounties::ensure_today_pool(&state.db);
    bounties::refresh_progress(&state.db, &today);
    let list = state.db.get_bounties_for_day(&today).map_err(|e| e.to_string())?;
    Ok(BountiesPayload {
        bounties: list,
        seconds_until_refresh: bounties::seconds_until_local_midnight(),
    })
}

#[tauri::command]
fn accept_bounty(state: tauri::State<'_, AppState>, id: String) -> Result<(), String> {
    bounties::accept(&state.db, &id)
}

#[tauri::command]
fn claim_bounty(state: tauri::State<'_, AppState>, id: String) -> Result<i64, String> {
    bounties::claim(&state.db, &id)
}

// Temp demo helpers — remove when demo mode is retired.
#[tauri::command]
fn debug_reset_bounties(state: tauri::State<'_, AppState>) -> Result<(), String> {
    bounties::debug_reset(&state.db)
}

#[tauri::command]
fn debug_complete_bounty(state: tauri::State<'_, AppState>, id: String) -> Result<(), String> {
    bounties::debug_complete(&state.db, &id)
}

#[tauri::command]
fn get_session_intervals(
    state: tauri::State<'_, AppState>,
    session_id: String,
) -> Result<Vec<db::Interval>, String> {
    state.db.get_session_intervals(&session_id).map_err(|e| e.to_string())
}

// ─── App entry ───────────────────────────────────────────────────────

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    env_logger::init();

    let db = Database::new().expect("Failed to initialize database");
    let session_mgr = SessionManager::new(db.clone());

    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .manage(AppState {
            db,
            session_mgr,
            tick_handle: Mutex::new(None),
            tick_running: Arc::new(Mutex::new(false)),
        })
        .invoke_handler(tauri::generate_handler![
            // Session
            start_session,
            end_session,
            session_tick,
            get_session_state,
            correct_classification,
            allow_app_this_session,
            rename_session,
            get_top_tools,
            // Dashboard
            get_dashboard_data,
            get_day_stats,
            get_sessions_in_range,
            // Profiles
            get_profiles,
            save_profile,
            delete_profile,
            // Settings
            get_settings,
            update_setting,
            // Points & avatars
            get_points_overview,
            get_shop_items,
            unlock_avatar,
            equip_avatar,
            // Monitor
            get_active_window,
            get_session_intervals,
            validate_goal,
            hide_drift_overlay,
            get_ollama_status,
            try_launch_ollama,
            submit_work_justification,
            // Bounties
            get_bounties,
            accept_bounty,
            claim_bounty,
            debug_reset_bounties,
            debug_complete_bounty,
        ])
        .setup(|app| {
            // Bring main window to front if found
            if let Some(window) = app.get_webview_window("main") {
                let _ = window.show();
                let _ = window.set_focus();
            }

            // Build system tray icon with click-to-restore
            let app_handle = app.handle().clone();
            TrayIconBuilder::new()
                .tooltip("Invigil — Focus Daemon")
                .icon(app.default_window_icon().cloned().unwrap())
                .on_tray_icon_event(move |_tray, event| {
                    if let TrayIconEvent::Click {
                        button: MouseButton::Left,
                        button_state: MouseButtonState::Up,
                        ..
                    } = event
                    {
                        if let Some(win) = app_handle.get_webview_window("main") {
                            let _ = win.show();
                            let _ = win.unminimize();
                            let _ = win.set_focus();
                        }
                    }
                })
                .build(app)?;

            Ok(())
        })
        .on_window_event(|window, event| {
            if window.label() == "main" {
                if let WindowEvent::CloseRequested { .. } = event {
                    window.app_handle().exit(0);
                }
            }
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
