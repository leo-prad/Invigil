use crate::classifier::{self, Classification};
use crate::db::{Database, Interval, Session};
use crate::llm;
use crate::monitor;
use chrono::Utc;
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::sync::Arc;
use uuid::Uuid;

// ─── Session state ───────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionState {
    pub active: bool,
    pub session_id: String,
    pub goal: String,
    pub description: String,
    pub profile_id: Option<String>,
    pub started_at: String,
    pub duration_target_min: Option<i64>,
    pub elapsed_sec: i64,
    pub on_task_sec: i64,
    pub off_task_sec: i64,
    pub idle_sec: i64,
    pub drift_count: i64,
    pub current_status: String,
    pub current_app: String,
    pub current_detail: String,
    pub deep_focus_sec: i64,        // longest unbroken on-task streak
    pub current_streak_sec: i64,    // current unbroken on-task run
    pub grace_remaining_sec: i64,   // countdown before flagging off-task
    pub is_idle: bool,
    pub session_allowlist: HashSet<String>,
}

impl Default for SessionState {
    fn default() -> Self {
        Self {
            active: false,
            session_id: String::new(),
            goal: String::new(),
            description: String::new(),
            profile_id: None,
            started_at: String::new(),
            duration_target_min: None,
            elapsed_sec: 0,
            on_task_sec: 0,
            off_task_sec: 0,
            idle_sec: 0,
            drift_count: 0,
            current_status: "idle".into(),
            current_app: String::new(),
            current_detail: String::new(),
            deep_focus_sec: 0,
            current_streak_sec: 0,
            grace_remaining_sec: 0,
            is_idle: false,
            session_allowlist: HashSet::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TickResult {
    pub state: SessionState,
    pub drift_triggered: bool,   // true = just entered off-task, show overlay
    pub drift_app: String,
    pub drift_detail: String,
    pub session_expired: bool,   // true = timer ran out
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionSummary {
    pub session_id: String,
    pub goal: String,
    pub duration_min: i64,
    pub on_task_sec: i64,
    pub elapsed_sec: i64,
    pub on_task_pct: f64,
    pub drift_count: i64,
    pub deep_focus_min: i64,
    pub points_earned: i64,
    pub point_breakdown: PointBreakdown,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PointBreakdown {
    pub base_points: i64,
    pub drift_penalty: i64,
    pub streak_multiplier: f64,
    pub total: i64,
}

// ─── Session manager ─────────────────────────────────────────────────

pub struct SessionManager {
    state: Arc<Mutex<SessionState>>,
    db: Database,
    grace_period_sec: i64,
    idle_timeout_sec: i64,
    idle_counter: Arc<Mutex<i64>>,
    // Track the last interval so we can close it when window or status changes
    last_interval_id: Arc<Mutex<Option<String>>>,
    last_status: Arc<Mutex<String>>,
    last_category: Arc<Mutex<String>>,
    last_window_title: Arc<Mutex<String>>,
    // Tier 1 LLM availability (checked once at start)
    ollama_available: Arc<Mutex<bool>>,
    // Wall-clock timestamp of the last tick (for accurate elapsed time)
    last_tick_at: Arc<Mutex<Option<chrono::DateTime<Utc>>>>,
}

impl SessionManager {
    pub fn new(db: Database) -> Self {
        let settings = db.get_all_settings().unwrap_or_default();
        let ollama_up = llm::is_ollama_available();
        if ollama_up {
            log::info!("Tier 1 LLM (Ollama) is available");
        } else {
            log::info!("Tier 1 LLM (Ollama) not found — ambiguous windows default to on_task");
        }

        Self {
            state: Arc::new(Mutex::new(SessionState::default())),
            db,
            grace_period_sec: settings.grace_period_sec,
            idle_timeout_sec: settings.idle_timeout_sec,
            idle_counter: Arc::new(Mutex::new(0)),
            last_interval_id: Arc::new(Mutex::new(None)),
            last_status: Arc::new(Mutex::new(String::new())),
            last_category: Arc::new(Mutex::new(String::new())),
            last_window_title: Arc::new(Mutex::new(String::new())),
            ollama_available: Arc::new(Mutex::new(ollama_up)),
            last_tick_at: Arc::new(Mutex::new(None)),
        }
    }

    pub fn start(&self, goal: String, description: String, profile_id: Option<String>, duration_min: Option<i64>) -> SessionState {
        let session_id = Uuid::new_v4().to_string();
        let now = Utc::now().to_rfc3339();

        let session = Session {
            id: session_id.clone(),
            goal: goal.clone(),
            description: description.clone(),
            profile_id: profile_id.clone(),
            started_at: now.clone(),
            ended_at: None,
            duration_min: None,
            points_earned: 0,
            on_task_pct: 0.0,
            drift_count: 0,
        };
        let _ = self.db.create_session(&session);

        // Re-check Ollama availability at session start
        *self.ollama_available.lock() = llm::is_ollama_available();

        let new_state = SessionState {
            active: true,
            session_id,
            goal,
            description,
            profile_id,
            started_at: now,
            duration_target_min: duration_min,
            ..Default::default()
        };

        *self.state.lock() = new_state.clone();
        *self.last_interval_id.lock() = None;
        *self.last_status.lock() = String::new();
        *self.last_category.lock() = String::new();
        *self.last_window_title.lock() = String::new();
        *self.idle_counter.lock() = 0;
        *self.last_tick_at.lock() = Some(Utc::now());

        // Mark today as focused
        let today = Utc::now().format("%Y-%m-%d").to_string();
        let _ = self.db.mark_day_focused(&today);

        new_state
    }

    pub fn stop(&self) -> SessionSummary {
        let state = self.state.lock().clone();
        let now = Utc::now().to_rfc3339();

        // Close last interval
        if let Some(ref interval_id) = *self.last_interval_id.lock() {
            let _ = self.db.close_interval(interval_id, &now);
        }

        // Calculate points
        let duration_min = state.elapsed_sec / 60;
        let on_task_pct = if state.elapsed_sec > 0 {
            (state.on_task_sec as f64 / state.elapsed_sec as f64) * 100.0
        } else {
            0.0
        };

        let breakdown = calculate_points(
            state.on_task_sec / 60,
            state.drift_count,
            &self.db,
        );

        // Update session in DB
        let _ = self.db.end_session(
            &state.session_id,
            &now,
            breakdown.total,
            on_task_pct,
            state.drift_count,
            duration_min,
        );

        // Add points to ledger
        if breakdown.total != 0 {
            let entry = crate::db::PointsEntry {
                id: Uuid::new_v4().to_string(),
                session_id: state.session_id.clone(),
                amount: breakdown.total,
                reason: format!("Session: {}", state.goal),
                timestamp: now,
            };
            let _ = self.db.add_points(&entry);
        }

        // Reset state
        *self.state.lock() = SessionState::default();

        SessionSummary {
            session_id: state.session_id,
            goal: state.goal,
            duration_min,
            on_task_sec: state.on_task_sec,
            elapsed_sec: state.elapsed_sec,
            on_task_pct,
            drift_count: state.drift_count,
            deep_focus_min: state.deep_focus_sec / 60,
            points_earned: breakdown.total,
            point_breakdown: breakdown,
        }
    }

    pub fn allow_app_for_session(&self, app: &str) {
        let mut s = self.state.lock();
        s.session_allowlist.insert(app.to_lowercase());
    }

    /// Called every 5 seconds during an active session.
    pub fn tick(&self) -> Option<TickResult> {
        // Extract goal, description and check active state without holding the lock during network operations
        let (goal, description, _profile_id) = {
            let s = self.state.lock();
            if !s.active {
                return None;
            }
            (s.goal.clone(), s.description.clone(), s.profile_id.clone())
        };

        // Get current window
        let window = monitor::get_active_window();
        let app_name = monitor::extract_app_name(&window);
        let detail = monitor::extract_detail(&window);

        // Classify (Tier 0 rule engine)
        let profile = self.get_active_profile();
        let (classification, _matched) = classifier::classify_tier0(&window, &profile);
        let category = classifier::categorize(&window);

        // Check session-level allowlist (apps approved with "This is work" during this session)
        let is_session_allowed = {
            let s = self.state.lock();
            s.session_allowlist.contains(&app_name.to_lowercase())
                || s.session_allowlist.contains(&category.to_lowercase())
                || window.process_name.to_lowercase().contains("invigil")
        };

        let status = if is_session_allowed {
            "on_task"
        } else if !profile.allow_patterns.is_empty() {
            // User explicitly specified tools for this session.
            let title_lower = window.title.to_lowercase();
            let proc_lower = window.process_name.to_lowercase();
            let matches_allow = profile.allow_patterns.iter().any(|pat| {
                let p = pat.to_lowercase();
                title_lower.contains(&p) || proc_lower.contains(&p)
            });

            if matches_allow {
                "on_task"
            } else {
                "off_task"
            }
        } else {
            match classification {
                Classification::OnTask => "on_task",
                Classification::OffTask => "off_task",
                Classification::Ambiguous => {
                    let ollama_up = *self.ollama_available.lock();
                    if ollama_up {
                        match llm::classify_ambiguous(&goal, &description, &app_name, &window.title) {
                            Some(llm::LlmVerdict::OnTask) => "on_task",
                            Some(llm::LlmVerdict::OffTask) => "off_task",
                            None => "on_task",
                        }
                    } else {
                        "on_task"
                    }
                }
            }
        };

        // System idle detection for inactivity (does not erase active app name)
        let idle_sec = monitor::get_idle_seconds();
        let idle_limit = self.idle_timeout_sec as u64;
        let is_currently_idle = idle_limit > 0 && idle_sec >= idle_limit;

        // Compute actual wall-clock delta since last tick
        let now_ts = Utc::now();
        let delta = {
            let mut last = self.last_tick_at.lock();
            let d = last.map(|t| (now_ts - t).num_seconds()).unwrap_or(5);
            *last = Some(now_ts);
            d.max(1)
        };

        // Re-acquire lock to update session state safely
        let mut state = self.state.lock();
        if !state.active {
            return None;
        }

        state.is_idle = is_currently_idle;
        state.elapsed_sec += delta;
        state.current_app = app_name.clone();
        let mut final_detail = detail.clone();
        if is_currently_idle {
            final_detail = format!("(Away from controls) {}", detail);
        } else if idle_sec >= 20 && status == "on_task" {
            final_detail = format!("(Deep reading) {}", detail);
        }
        state.current_detail = final_detail;

        // Grace period logic
        let mut drift_triggered = false;
        let prev_status = self.last_status.lock().clone();

        if status == "off_task" {
            if prev_status != "off_task" {
                // Just started drifting — begin grace period
                state.grace_remaining_sec = self.grace_period_sec;
            } else if state.grace_remaining_sec > 0 {
                state.grace_remaining_sec -= delta;
            }

            if state.grace_remaining_sec <= 0 {
                // Grace expired — this is a real drift
                state.off_task_sec += delta;
                state.current_status = "off_task".into();
                state.current_streak_sec = 0;

                if prev_status != "off_task" || state.grace_remaining_sec == 0 - delta {
                    // First tick after grace expiry — trigger overlay
                    state.drift_count += 1;
                    drift_triggered = true;
                }
            } else {
                // Still in grace period — count as on-task
                state.on_task_sec += delta;
                state.current_status = "on_task".into();
                state.current_streak_sec += delta;
            }
        } else {
            // On task
            state.on_task_sec += delta;
            state.current_status = "on_task".into();
            state.current_streak_sec += delta;
            state.grace_remaining_sec = 0;

            if state.current_streak_sec > state.deep_focus_sec {
                state.deep_focus_sec = state.current_streak_sec;
            }
        }

        // Record interval change if status, category, or window title changed
        let effective_status = state.current_status.clone();
        let prev_status = self.last_status.lock().clone();
        let prev_cat = self.last_category.lock().clone();
        let prev_title = self.last_window_title.lock().clone();

        if effective_status != prev_status || category != prev_cat || window.title != prev_title {
            let now = Utc::now().to_rfc3339();

            // Close previous interval
            if let Some(ref old_id) = *self.last_interval_id.lock() {
                let _ = self.db.close_interval(old_id, &now);
            }

            // Open new interval
            let interval_id = Uuid::new_v4().to_string();
            let interval = Interval {
                id: interval_id.clone(),
                session_id: state.session_id.clone(),
                status: effective_status.clone(),
                category: category.clone(),
                window_title: window.title.clone(),
                process_name: window.process_name.clone(),
                start_ts: now,
                end_ts: None,
                tier_used: 0,
            };
            let _ = self.db.insert_interval(&interval);
            *self.last_interval_id.lock() = Some(interval_id);
            *self.last_status.lock() = effective_status;
            *self.last_category.lock() = category;
            *self.last_window_title.lock() = window.title.clone();
        }

        // Check if session timer expired
        let session_expired = if let Some(target) = state.duration_target_min {
            state.elapsed_sec >= target * 60
        } else {
            false
        };

        let result = TickResult {
            state: state.clone(),
            drift_triggered,
            drift_app: app_name,
            drift_detail: detail,
            session_expired,
        };

        Some(result)
    }

    pub fn get_state(&self) -> SessionState {
        self.state.lock().clone()
    }

    pub fn correct_current(&self, new_status: &str) {
        let mut state = self.state.lock();
        if !state.active {
            return;
        }

        let now = Utc::now().to_rfc3339();

        // If correcting to on_task, adjust counters
        if new_status == "on_task" && state.current_status == "off_task" {
            state.current_status = "on_task".into();
            // Undo the last drift count if it just happened
            if state.drift_count > 0 {
                state.drift_count -= 1;
            }
        }

        // Log correction
        if let Some(ref interval_id) = *self.last_interval_id.lock() {
            let correction = crate::db::Correction {
                id: Uuid::new_v4().to_string(),
                interval_id: interval_id.clone(),
                original_status: state.current_status.clone(),
                corrected_status: new_status.to_string(),
                timestamp: now,
            };
            let _ = self.db.insert_correction(&correction);
        }

        *self.last_status.lock() = new_status.to_string();
    }

    fn get_active_profile(&self) -> crate::db::Profile {
        let state = self.state.lock();
        let pid = state.profile_id.clone().unwrap_or("default".into());
        drop(state);

        self.db.get_profiles()
            .unwrap_or_default()
            .into_iter()
            .find(|p| p.id == pid)
            .unwrap_or(crate::db::Profile {
                id: "default".into(),
                name: "Default".into(),
                allow_patterns: vec![],
                deny_patterns: vec![],
            })
    }
}

impl Default for crate::db::Settings {
    fn default() -> Self {
        Self {
            idle_timeout_sec: 45,
            grace_period_sec: 15,
            sensitivity: 3,
            quiet_hours_start: None,
            quiet_hours_end: None,
            tier1_enabled: true,
            tier2_enabled: false,
            tier3_enabled: false,
        }
    }
}

// ─── Points calculation ──────────────────────────────────────────────

fn calculate_points(on_task_min: i64, drift_count: i64, db: &Database) -> PointBreakdown {
    let base = on_task_min * 50;          // 50 pts per focused minute
    let penalty = drift_count * -100;     // -100 per drift
    let streak = db.get_streak_info().map(|s| {
        if s.current >= 7 { 2.0 }
        else if s.current >= 3 { 1.5 }
        else { 1.0 }
    }).unwrap_or(1.0);

    let subtotal = base + penalty;
    let total = (subtotal as f64 * streak).round() as i64;

    PointBreakdown {
        base_points: base,
        drift_penalty: penalty,
        streak_multiplier: streak,
        total: total.max(0), // never negative
    }
}
