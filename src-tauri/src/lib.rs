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
use tauri::{AppHandle, Emitter, Manager, PhysicalPosition, PhysicalSize};

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

                if result.drift_triggered {
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
    Ok(summary)
}

#[tauri::command]
fn hide_drift_overlay(app: AppHandle) -> Result<(), String> {
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
        ])
        .setup(|app| {
            // Bring main window to front if found
            if let Some(window) = app.get_webview_window("main") {
                let _ = window.show();
                let _ = window.set_focus();
            }

            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
