use chrono::Utc;
use rusqlite::{params, Connection, Result as SqlResult};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::Arc;
use parking_lot::Mutex;

// ─── Types ───────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Profile {
    pub id: String,
    pub name: String,
    pub allow_patterns: Vec<String>,
    pub deny_patterns: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Session {
    pub id: String,
    pub goal: String,
    pub description: String,
    pub profile_id: Option<String>,
    pub started_at: String,
    pub ended_at: Option<String>,
    pub duration_min: Option<i64>,
    pub points_earned: i64,
    pub on_task_pct: f64,
    pub drift_count: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Interval {
    pub id: String,
    pub session_id: String,
    pub status: String,      // "on_task", "off_task", "ambiguous", "idle"
    pub category: String,    // app/site name
    pub window_title: String,
    pub process_name: String,
    pub start_ts: String,
    pub end_ts: Option<String>,
    pub tier_used: i32,      // 0, 1, 2, 3
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Correction {
    pub id: String,
    pub interval_id: String,
    pub original_status: String,
    pub corrected_status: String,
    pub timestamp: String,
    // Free-text user reason from the "this is actually work" flow. None for silent overrides.
    pub justification: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PointsEntry {
    pub id: String,
    pub session_id: String,
    pub amount: i64,
    pub reason: String,
    pub timestamp: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Avatar {
    pub id: String,
    pub name: String,
    pub description: String,
    pub cost: i64,
    pub unlocked: bool,
    pub equipped: bool,
    pub sprite_key: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DayStats {
    pub date: String,
    pub total_minutes: i64,
    pub on_task_pct: f64,
    pub session_count: i64,
    pub points: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StreakInfo {
    pub current: i64,
    pub best: i64,
    pub month_total_minutes: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DistractionStat {
    pub name: String,
    pub minutes: i64,
    pub seconds: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Bounty {
    pub id: String,
    pub day: String,              // Local-date YYYY-MM-DD; the whole row is discarded past midnight.
    pub kind: String,             // "reinforcing" | "exploratory"
    pub difficulty: String,       // "easy" | "medium" | "hard"
    pub title: String,
    pub description: String,
    pub criterion: String,        // JSON blob; shape lives in bounties::Criterion
    pub reward: i64,
    pub status: String,           // "available" | "accepted" | "completed" | "claimed"
    pub progress: f64,            // 0.0 .. 1.0
    pub progress_label: String,   // e.g. "12 / 25 min"
    pub accepted_at: Option<String>,
    pub completed_at: Option<String>,
    pub claimed_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Settings {
    pub idle_timeout_sec: i64,
    pub grace_period_sec: i64,
    pub sensitivity: i32,       // 1-5
    pub quiet_hours_start: Option<String>,
    pub quiet_hours_end: Option<String>,
    pub tier1_enabled: bool,
    pub tier2_enabled: bool,
    pub tier3_enabled: bool,
}

// ─── Database ────────────────────────────────────────────────────────

#[derive(Clone)]
pub struct Database {
    conn: Arc<Mutex<Connection>>,
}

impl Database {
    pub fn new() -> SqlResult<Self> {
        let path = Self::db_path();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).ok();
        }
        let conn = Connection::open(&path)?;
        let db = Self { conn: Arc::new(Mutex::new(conn)) };
        db.init_tables()?;
        db.seed_defaults()?;
        Ok(db)
    }

    fn db_path() -> PathBuf {
        let mut p = dirs::data_local_dir().unwrap_or_else(|| PathBuf::from("."));
        p.push("Invigil");
        p.push("invigil.db");
        p
    }

    fn init_tables(&self) -> SqlResult<()> {
        let conn = self.conn.lock();
        conn.execute_batch("
            CREATE TABLE IF NOT EXISTS profiles (
                id TEXT PRIMARY KEY,
                name TEXT NOT NULL,
                allow_patterns TEXT NOT NULL DEFAULT '[]',
                deny_patterns TEXT NOT NULL DEFAULT '[]'
            );

            CREATE TABLE IF NOT EXISTS sessions (
                id TEXT PRIMARY KEY,
                goal TEXT NOT NULL,
                description TEXT NOT NULL DEFAULT '',
                profile_id TEXT,
                started_at TEXT NOT NULL,
                ended_at TEXT,
                duration_min INTEGER,
                points_earned INTEGER NOT NULL DEFAULT 0,
                on_task_pct REAL NOT NULL DEFAULT 0.0,
                drift_count INTEGER NOT NULL DEFAULT 0,
                FOREIGN KEY (profile_id) REFERENCES profiles(id)
            );

            CREATE TABLE IF NOT EXISTS intervals (
                id TEXT PRIMARY KEY,
                session_id TEXT NOT NULL,
                status TEXT NOT NULL,
                category TEXT NOT NULL DEFAULT '',
                window_title TEXT NOT NULL DEFAULT '',
                process_name TEXT NOT NULL DEFAULT '',
                start_ts TEXT NOT NULL,
                end_ts TEXT,
                tier_used INTEGER NOT NULL DEFAULT 0,
                FOREIGN KEY (session_id) REFERENCES sessions(id)
            );

            CREATE TABLE IF NOT EXISTS corrections (
                id TEXT PRIMARY KEY,
                interval_id TEXT NOT NULL,
                original_status TEXT NOT NULL,
                corrected_status TEXT NOT NULL,
                timestamp TEXT NOT NULL,
                FOREIGN KEY (interval_id) REFERENCES intervals(id)
            );

            CREATE TABLE IF NOT EXISTS points_ledger (
                id TEXT PRIMARY KEY,
                session_id TEXT NOT NULL,
                amount INTEGER NOT NULL,
                reason TEXT NOT NULL,
                timestamp TEXT NOT NULL,
                FOREIGN KEY (session_id) REFERENCES sessions(id)
            );

            CREATE TABLE IF NOT EXISTS avatars (
                id TEXT PRIMARY KEY,
                name TEXT NOT NULL,
                description TEXT NOT NULL DEFAULT '',
                cost INTEGER NOT NULL DEFAULT 0,
                unlocked INTEGER NOT NULL DEFAULT 0,
                equipped INTEGER NOT NULL DEFAULT 0,
                sprite_key TEXT NOT NULL DEFAULT 'default'
            );

            CREATE TABLE IF NOT EXISTS settings (
                key TEXT PRIMARY KEY,
                value TEXT NOT NULL
            );

            CREATE TABLE IF NOT EXISTS streaks (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                date TEXT NOT NULL UNIQUE,
                focused INTEGER NOT NULL DEFAULT 0
            );

            CREATE TABLE IF NOT EXISTS bounties (
                id TEXT PRIMARY KEY,
                day TEXT NOT NULL,
                kind TEXT NOT NULL,
                difficulty TEXT NOT NULL,
                title TEXT NOT NULL,
                description TEXT NOT NULL,
                criterion TEXT NOT NULL,
                reward INTEGER NOT NULL DEFAULT 0,
                status TEXT NOT NULL DEFAULT 'available',
                progress REAL NOT NULL DEFAULT 0.0,
                progress_label TEXT NOT NULL DEFAULT '',
                accepted_at TEXT,
                completed_at TEXT,
                claimed_at TEXT
            );
        ")?;
        
        // Safely check and add description column to sessions table if missing
        let has_desc = conn.prepare("SELECT description FROM sessions LIMIT 1").is_ok();
        if !has_desc {
            let _ = conn.execute("ALTER TABLE sessions ADD COLUMN description TEXT NOT NULL DEFAULT ''", params![]);
        }
        // Nullable justification column for the "this is actually work" flow.
        let has_just = conn.prepare("SELECT justification FROM corrections LIMIT 1").is_ok();
        if !has_just {
            let _ = conn.execute("ALTER TABLE corrections ADD COLUMN justification TEXT", params![]);
        }
        Ok(())
    }

    fn seed_defaults(&self) -> SqlResult<()> {
        let conn = self.conn.lock();

        // Default settings
        let defaults = vec![
            ("idle_timeout_sec", "45"),
            ("grace_period_sec", "0"),
            ("sensitivity", "3"),
            ("tier1_enabled", "true"),
            ("tier2_enabled", "false"),
            ("tier3_enabled", "false"),
            ("ai_setup_seen", "false"),
            ("ai_enabled", "false"),
            ("ai_model", "gemma3:4b"),
        ];
        for (k, v) in defaults {
            conn.execute(
                "INSERT OR IGNORE INTO settings (key, value) VALUES (?1, ?2)",
                params![k, v],
            )?;
        }

        // Default avatars
        let avatars = vec![
            ("default", "Professor Xeno", "The classic look — black suit, round spectacles.", 0, 1, 1, "default"),
            ("lab_coat", "Lab Coat Xeno", "White lab coat with beaker stains.", 500, 0, 0, "lab_coat"),
            ("cyber", "Cyber Xeno", "Neon-trimmed hoodie, glowing visor.", 1500, 0, 0, "cyber"),
            ("astro", "Astro Xeno", "Full spacesuit with helmet. For the truly locked in.", 3000, 0, 0, "astro"),
            ("sensei", "Sensei Xeno", "Traditional robes with a wisdom scroll.", 2000, 0, 0, "sensei"),
            ("stealth", "Stealth Xeno", "All-black tactical gear. Zero distractions.", 2500, 0, 0, "stealth"),
        ];
        for (id, name, desc, cost, unlocked, equipped, sprite) in avatars {
            conn.execute(
                "INSERT OR IGNORE INTO avatars (id, name, description, cost, unlocked, equipped, sprite_key) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![id, name, desc, cost, unlocked, equipped, sprite],
            )?;
        }

        // Default profile
        conn.execute(
            "INSERT OR IGNORE INTO profiles (id, name, allow_patterns, deny_patterns) VALUES (?1, ?2, ?3, ?4)",
            params![
                "default",
                "General",
                r#"["Overleaf","Desmos","Canvas","VS Code","Google Docs","Khan Academy"]"#,
                r#"["YouTube","Instagram","Reddit","TikTok","Discord — #general"]"#,
            ],
        )?;

        // Seed demo data if database has no sessions yet
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM sessions", [], |r| r.get(0))
            .unwrap_or(0);
        if count == 0 {
            let now = Utc::now();
            let today_str = now.format("%Y-%m-%d").to_string();

            // Seed streaks for the past 12 days
            for i in 0..12 {
                let d = (now - chrono::Duration::days(i)).format("%Y-%m-%d").to_string();
                conn.execute(
                    "INSERT OR IGNORE INTO streaks (date, focused) VALUES (?1, 1)",
                    params![d],
                )?;
            }

            // Seed sample past sessions
            let sample_sessions = vec![
                ("s1", "Calc HW - Integration", "Integral practice on Desmos & Overleaf", "2026-08-09T14:00:00Z", "2026-08-09T14:45:00Z", 45, 750, 92.0, 1),
                ("s2", "CS Reading - Data Structures", "Trees and Graph algorithms", "2026-08-08T10:00:00Z", "2026-08-08T11:00:00Z", 60, 900, 88.0, 2),
                ("s3", "Physics Lab - Electromagnetism", "Data collection & calculations", "2026-08-07T16:00:00Z", "2026-08-07T17:15:00Z", 75, 1100, 95.0, 0),
                ("s4", "Linear Algebra - Matrix Multiplication", "Chapter 4 problem set", "2026-08-06T11:00:00Z", "2026-08-06T12:00:00Z", 60, 850, 85.0, 3),
                ("s5", "Essay Outline - History", "Primary source analysis", "2026-08-05T15:00:00Z", "2026-08-05T16:00:00Z", 60, 608, 90.0, 1),
            ];

            for (id, goal, desc, start_ts, end_ts, dur, pts, pct, drift) in sample_sessions {
                conn.execute(
                    "INSERT INTO sessions (id, goal, description, profile_id, started_at, ended_at, duration_min, points_earned, on_task_pct, drift_count)
                     VALUES (?1, ?2, ?3, 'default', ?4, ?5, ?6, ?7, ?8, ?9)",
                    params![id, goal, desc, start_ts, end_ts, dur, pts, pct, drift],
                )?;
            }

            // Seed sample distraction intervals (for biggest distractions card)
            let distractions = vec![
                ("YouTube", 134),
                ("Discord", 88),
                ("Instagram", 55),
                ("Reddit", 36),
                ("iMessage web", 22),
            ];

            let mut idx = 0;
            for (app_name, mins) in distractions {
                let interval_count = (mins * 60) / 5;
                for _ in 0..interval_count {
                    idx += 1;
                    let i_id = format!("int_{}", idx);
                    conn.execute(
                        "INSERT INTO intervals (id, session_id, status, category, window_title, process_name, start_ts, end_ts, tier_used)
                         VALUES (?1, 's1', 'off_task', ?2, ?2, ?2, ?3, ?3, 0)",
                        params![i_id, app_name, today_str],
                    )?;
                }
            }

            // Seed points entry
            conn.execute(
                "INSERT INTO points_ledger (id, session_id, amount, reason, timestamp)
                 VALUES ('p1', 's1', 4208, 'Initial balance', ?1)",
                params![today_str],
            )?;
        }

        Ok(())
    }

    // ─── Profiles ────────────────────────────────────────────────────

    pub fn get_profiles(&self) -> SqlResult<Vec<Profile>> {
        let conn = self.conn.lock();
        let mut stmt = conn.prepare("SELECT id, name, allow_patterns, deny_patterns FROM profiles ORDER BY name")?;
        let rows = stmt.query_map([], |row| {
            let allow_str: String = row.get(2)?;
            let deny_str: String = row.get(3)?;
            Ok(Profile {
                id: row.get(0)?,
                name: row.get(1)?,
                allow_patterns: serde_json::from_str(&allow_str).unwrap_or_default(),
                deny_patterns: serde_json::from_str(&deny_str).unwrap_or_default(),
            })
        })?;
        rows.collect()
    }

    pub fn upsert_profile(&self, profile: &Profile) -> SqlResult<()> {
        let conn = self.conn.lock();
        let allow_json = serde_json::to_string(&profile.allow_patterns).unwrap_or_default();
        let deny_json = serde_json::to_string(&profile.deny_patterns).unwrap_or_default();
        conn.execute(
            "INSERT INTO profiles (id, name, allow_patterns, deny_patterns) VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(id) DO UPDATE SET name=?2, allow_patterns=?3, deny_patterns=?4",
            params![profile.id, profile.name, allow_json, deny_json],
        )?;
        Ok(())
    }

    pub fn delete_profile(&self, id: &str) -> SqlResult<()> {
        let conn = self.conn.lock();
        conn.execute("DELETE FROM profiles WHERE id = ?1", params![id])?;
        Ok(())
    }

    // ─── Sessions ────────────────────────────────────────────────────

    pub fn create_session(&self, session: &Session) -> SqlResult<()> {
        let conn = self.conn.lock();
        conn.execute(
            "INSERT INTO sessions (id, goal, description, profile_id, started_at, ended_at, duration_min, points_earned, on_task_pct, drift_count)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            params![
                session.id, session.goal, session.description, session.profile_id, session.started_at,
                session.ended_at, session.duration_min, session.points_earned,
                session.on_task_pct, session.drift_count,
            ],
        )?;
        Ok(())
    }

    pub fn end_session(&self, id: &str, ended_at: &str, points: i64, on_task_pct: f64, drift_count: i64, duration_min: i64) -> SqlResult<()> {
        let conn = self.conn.lock();
        conn.execute(
            "UPDATE sessions SET ended_at=?2, points_earned=?3, on_task_pct=?4, drift_count=?5, duration_min=?6 WHERE id=?1",
            params![id, ended_at, points, on_task_pct, drift_count, duration_min],
        )?;
        Ok(())
    }

    pub fn update_session_goal(&self, id: &str, new_goal: &str) -> SqlResult<()> {
        let conn = self.conn.lock();
        conn.execute("UPDATE sessions SET goal = ?2 WHERE id = ?1", params![id, new_goal])?;
        Ok(())
    }

    pub fn get_recent_sessions(&self, limit: i64) -> SqlResult<Vec<Session>> {
        let conn = self.conn.lock();
        let mut stmt = conn.prepare(
            "SELECT id, goal, COALESCE(description, ''), profile_id, started_at, ended_at, duration_min, points_earned, on_task_pct, drift_count
             FROM sessions ORDER BY started_at DESC LIMIT ?1"
        )?;
        let rows = stmt.query_map(params![limit], |row| {
            Ok(Session {
                id: row.get(0).unwrap_or_default(),
                goal: row.get(1).unwrap_or_default(),
                description: row.get(2).unwrap_or_default(),
                profile_id: row.get(3).ok(),
                started_at: row.get(4).unwrap_or_default(),
                ended_at: row.get(5).ok(),
                duration_min: row.get(6).ok(),
                points_earned: row.get(7).unwrap_or(0),
                on_task_pct: row.get(8).unwrap_or(0.0),
                drift_count: row.get(9).unwrap_or(0),
            })
        })?;
        Ok(rows.filter_map(|r| r.ok()).collect())
    }

    pub fn get_sessions_in_range(&self, start: &str, end: &str) -> SqlResult<Vec<Session>> {
        let conn = self.conn.lock();
        let mut stmt = conn.prepare(
            "SELECT id, goal, COALESCE(description, ''), profile_id, started_at, ended_at, duration_min, points_earned, on_task_pct, drift_count
             FROM sessions WHERE date(started_at) >= ?1 AND date(started_at) <= ?2 ORDER BY started_at DESC"
        )?;
        let rows = stmt.query_map(params![start, end], |row| {
            Ok(Session {
                id: row.get(0).unwrap_or_default(),
                goal: row.get(1).unwrap_or_default(),
                description: row.get(2).unwrap_or_default(),
                profile_id: row.get(3).ok(),
                started_at: row.get(4).unwrap_or_default(),
                ended_at: row.get(5).ok(),
                duration_min: row.get(6).ok(),
                points_earned: row.get(7).unwrap_or(0),
                on_task_pct: row.get(8).unwrap_or(0.0),
                drift_count: row.get(9).unwrap_or(0),
            })
        })?;
        Ok(rows.filter_map(|r| r.ok()).collect())
    }

    // ─── Intervals ───────────────────────────────────────────────────

    pub fn insert_interval(&self, interval: &Interval) -> SqlResult<()> {
        let conn = self.conn.lock();
        conn.execute(
            "INSERT INTO intervals (id, session_id, status, category, window_title, process_name, start_ts, end_ts, tier_used)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                interval.id, interval.session_id, interval.status, interval.category,
                interval.window_title, interval.process_name, interval.start_ts,
                interval.end_ts, interval.tier_used,
            ],
        )?;
        Ok(())
    }

    pub fn close_interval(&self, id: &str, end_ts: &str) -> SqlResult<()> {
        let conn = self.conn.lock();
        conn.execute(
            "UPDATE intervals SET end_ts=?2 WHERE id=?1",
            params![id, end_ts],
        )?;
        Ok(())
    }

    pub fn get_session_intervals(&self, session_id: &str) -> SqlResult<Vec<Interval>> {
        let conn = self.conn.lock();
        let mut stmt = conn.prepare(
            "SELECT id, session_id, status, category, window_title, process_name, start_ts, end_ts, tier_used
             FROM intervals WHERE session_id=?1 ORDER BY start_ts"
        )?;
        let rows = stmt.query_map(params![session_id], |row| {
            Ok(Interval {
                id: row.get(0)?,
                session_id: row.get(1)?,
                status: row.get(2)?,
                category: row.get(3)?,
                window_title: row.get(4)?,
                process_name: row.get(5)?,
                start_ts: row.get(6)?,
                end_ts: row.get(7)?,
                tier_used: row.get(8)?,
            })
        })?;
        rows.collect()
    }

    // ─── Corrections ─────────────────────────────────────────────────

    pub fn insert_correction(&self, correction: &Correction) -> SqlResult<()> {
        let conn = self.conn.lock();
        conn.execute(
            "INSERT INTO corrections (id, interval_id, original_status, corrected_status, timestamp, justification)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![correction.id, correction.interval_id, correction.original_status, correction.corrected_status, correction.timestamp, correction.justification],
        )?;
        // Also update the interval itself
        conn.execute(
            "UPDATE intervals SET status=?2 WHERE id=?1",
            params![correction.interval_id, correction.corrected_status],
        )?;
        Ok(())
    }

    // ─── Points ──────────────────────────────────────────────────────

    pub fn add_points(&self, entry: &PointsEntry) -> SqlResult<()> {
        let conn = self.conn.lock();
        conn.execute(
            "INSERT INTO points_ledger (id, session_id, amount, reason, timestamp) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![entry.id, entry.session_id, entry.amount, entry.reason, entry.timestamp],
        )?;
        Ok(())
    }

    pub fn get_total_points(&self) -> SqlResult<i64> {
        let conn = self.conn.lock();
        let total: i64 = conn.query_row(
            "SELECT COALESCE(SUM(amount), 0) FROM points_ledger", [], |row| row.get(0)
        )?;
        Ok(total)
    }

    // ─── Avatars ─────────────────────────────────────────────────────

    pub fn get_avatars(&self) -> SqlResult<Vec<Avatar>> {
        let conn = self.conn.lock();
        let mut stmt = conn.prepare("SELECT id, name, description, cost, unlocked, equipped, sprite_key FROM avatars ORDER BY cost")?;
        let rows = stmt.query_map([], |row| {
            Ok(Avatar {
                id: row.get(0)?,
                name: row.get(1)?,
                description: row.get(2)?,
                cost: row.get(3)?,
                unlocked: row.get::<_, i32>(4)? != 0,
                equipped: row.get::<_, i32>(5)? != 0,
                sprite_key: row.get(6)?,
            })
        })?;
        rows.collect()
    }

    pub fn unlock_avatar(&self, id: &str) -> SqlResult<bool> {
        let conn = self.conn.lock();
        let cost: i64 = conn.query_row("SELECT cost FROM avatars WHERE id=?1", params![id], |row| row.get(0))?;
        let total = {
            let t: i64 = conn.query_row("SELECT COALESCE(SUM(amount), 0) FROM points_ledger", [], |row| row.get(0))?;
            t
        };
        let spent: i64 = conn.query_row(
            "SELECT COALESCE(SUM(cost), 0) FROM avatars WHERE unlocked=1 AND id != 'default'", [], |row| row.get(0)
        )?;
        if total - spent >= cost {
            conn.execute("UPDATE avatars SET unlocked=1 WHERE id=?1", params![id])?;
            Ok(true)
        } else {
            Ok(false)
        }
    }

    pub fn equip_avatar(&self, id: &str) -> SqlResult<()> {
        let conn = self.conn.lock();
        conn.execute("UPDATE avatars SET equipped=0", [])?;
        conn.execute("UPDATE avatars SET equipped=1 WHERE id=?1", params![id])?;
        Ok(())
    }

    // ─── Settings ────────────────────────────────────────────────────

    pub fn get_setting(&self, key: &str) -> SqlResult<String> {
        let conn = self.conn.lock();
        conn.query_row("SELECT value FROM settings WHERE key=?1", params![key], |row| row.get(0))
    }

    pub fn set_setting(&self, key: &str, value: &str) -> SqlResult<()> {
        let conn = self.conn.lock();
        conn.execute(
            "INSERT INTO settings (key, value) VALUES (?1, ?2) ON CONFLICT(key) DO UPDATE SET value=?2",
            params![key, value],
        )?;
        Ok(())
    }

    pub fn get_all_settings(&self) -> SqlResult<Settings> {
        Ok(Settings {
            idle_timeout_sec: self.get_setting("idle_timeout_sec").unwrap_or("45".into()).parse().unwrap_or(45),
            grace_period_sec: self.get_setting("grace_period_sec").unwrap_or("0".into()).parse().unwrap_or(0),
            sensitivity: self.get_setting("sensitivity").unwrap_or("3".into()).parse().unwrap_or(3),
            quiet_hours_start: self.get_setting("quiet_hours_start").ok(),
            quiet_hours_end: self.get_setting("quiet_hours_end").ok(),
            tier1_enabled: self.get_setting("tier1_enabled").unwrap_or("true".into()) == "true",
            tier2_enabled: self.get_setting("tier2_enabled").unwrap_or("false".into()) == "true",
            tier3_enabled: self.get_setting("tier3_enabled").unwrap_or("false".into()) == "true",
        })
    }

    // ─── Streaks ─────────────────────────────────────────────────────

    pub fn mark_day_focused(&self, date: &str) -> SqlResult<()> {
        let conn = self.conn.lock();
        conn.execute(
            "INSERT INTO streaks (date, focused) VALUES (?1, 1) ON CONFLICT(date) DO UPDATE SET focused=1",
            params![date],
        )?;
        Ok(())
    }

    pub fn get_streak_info(&self) -> SqlResult<StreakInfo> {
        let conn = self.conn.lock();

        // Current streak: count consecutive focused days ending at the most recent focused day.
        // If today has no row yet (user hasn't started a session today), start from yesterday
        // so the streak still counts what they built up. Only breaks when a whole day is skipped.
        let today = Utc::now().format("%Y-%m-%d").to_string();
        let mut current = 0i64;
        let mut check_date = chrono::NaiveDate::parse_from_str(&today, "%Y-%m-%d").unwrap_or_default();
        let today_focused: i64 = conn.query_row(
            "SELECT COALESCE(focused, 0) FROM streaks WHERE date=?1",
            params![check_date.format("%Y-%m-%d").to_string()],
            |row| row.get(0),
        ).unwrap_or(0);
        if today_focused == 0 {
            check_date -= chrono::Duration::days(1);
        }
        loop {
            let ds = check_date.format("%Y-%m-%d").to_string();
            let focused: i64 = conn.query_row(
                "SELECT COALESCE(focused, 0) FROM streaks WHERE date=?1",
                params![ds],
                |row| row.get(0),
            ).unwrap_or(0);
            if focused > 0 {
                current += 1;
                check_date -= chrono::Duration::days(1);
            } else {
                break;
            }
        }

        // Best streak ever
        let mut best = 0i64;
        let mut stmt = conn.prepare("SELECT date FROM streaks WHERE focused=1 ORDER BY date")?;
        let dates: Vec<String> = stmt.query_map([], |row| row.get(0))?.filter_map(|r| r.ok()).collect();
        let mut run = 0i64;
        let mut prev: Option<chrono::NaiveDate> = None;
        for d in &dates {
            if let Ok(nd) = chrono::NaiveDate::parse_from_str(d, "%Y-%m-%d") {
                if let Some(p) = prev {
                    if nd - p == chrono::Duration::days(1) {
                        run += 1;
                    } else {
                        run = 1;
                    }
                } else {
                    run = 1;
                }
                if run > best { best = run; }
                prev = Some(nd);
            }
        }

        // This month total minutes
        let month_start = format!("{}-01", &today[..7]);
        let month_min: i64 = conn.query_row(
            "SELECT COALESCE(SUM(duration_min), 0) FROM sessions WHERE date(started_at) >= ?1",
            params![month_start],
            |row| row.get(0),
        ).unwrap_or(0);

        Ok(StreakInfo { current, best, month_total_minutes: month_min })
    }

    // ─── Dashboard stats ─────────────────────────────────────────────

    pub fn get_day_stats(&self, date: &str) -> SqlResult<DayStats> {
        let conn = self.conn.lock();
        let total_min: i64 = conn.query_row(
            "SELECT COALESCE(SUM(duration_min), 0) FROM sessions WHERE date(started_at) = ?1 AND ended_at IS NOT NULL",
            params![date], |row| row.get(0),
        ).unwrap_or(0);
        let avg_pct: f64 = conn.query_row(
            "SELECT COALESCE(AVG(on_task_pct), 0) FROM sessions WHERE date(started_at) = ?1 AND ended_at IS NOT NULL",
            params![date], |row| row.get(0),
        ).unwrap_or(0.0);
        let count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM sessions WHERE date(started_at) = ?1 AND ended_at IS NOT NULL",
            params![date], |row| row.get(0),
        ).unwrap_or(0);
        let pts: i64 = conn.query_row(
            "SELECT COALESCE(SUM(points_earned), 0) FROM sessions WHERE date(started_at) = ?1",
            params![date], |row| row.get(0),
        ).unwrap_or(0);
        Ok(DayStats {
            date: date.to_string(),
            total_minutes: total_min,
            on_task_pct: avg_pct,
            session_count: count,
            points: pts,
        })
    }

    pub fn get_week_distraction_stats(&self) -> SqlResult<Vec<DistractionStat>> {
        let conn = self.conn.lock();
        let week_ago = (Utc::now() - chrono::Duration::days(7)).format("%Y-%m-%d").to_string();
        let mut stmt = conn.prepare(
            "SELECT category, COUNT(*) * 5 as secs FROM intervals
             WHERE status='off_task' AND start_ts >= ?1
             GROUP BY category ORDER BY secs DESC LIMIT 10"
        )?;
        let rows = stmt.query_map(params![week_ago], |row| {
            let secs: i64 = row.get(1)?;
            Ok(DistractionStat {
                name: row.get(0)?,
                minutes: secs / 60,
                seconds: secs,
            })
        })?;
        rows.collect()
    }

    pub fn get_weekly_focus_minutes(&self) -> SqlResult<i64> {
        let conn = self.conn.lock();
        let week_ago = (Utc::now() - chrono::Duration::days(7)).format("%Y-%m-%d").to_string();
        let mins: i64 = conn.query_row(
            "SELECT COALESCE(SUM(duration_min), 0) FROM sessions WHERE date(started_at) >= ?1 AND ended_at IS NOT NULL",
            params![week_ago], |row| row.get(0),
        ).unwrap_or(0);
        Ok(mins)
    }

    pub fn get_top_user_tools(&self) -> SqlResult<Vec<String>> {
        let conn = self.conn.lock();
        let mut stmt = conn.prepare(
            "SELECT category FROM intervals WHERE category IS NOT NULL AND category != '' AND category != 'Invigil' GROUP BY category ORDER BY COUNT(*) DESC LIMIT 8"
        )?;
        let rows = stmt.query_map([], |row| row.get(0))?;
        let mut tools: Vec<String> = rows.filter_map(|r| r.ok()).collect();
        let defaults = vec!["Overleaf", "Desmos", "Canvas", "Google Docs", "ChatGPT", "VS Code"];
        for d in defaults {
            if !tools.iter().any(|t| t.eq_ignore_ascii_case(d)) {
                tools.push(d.to_string());
            }
        }
        Ok(tools)
    }

    // ─── Bounties ────────────────────────────────────────────────────

    pub fn get_bounties_for_day(&self, day: &str) -> SqlResult<Vec<Bounty>> {
        let conn = self.conn.lock();
        let mut stmt = conn.prepare(
            "SELECT id, day, kind, difficulty, title, description, criterion, reward,
                    status, progress, progress_label, accepted_at, completed_at, claimed_at
             FROM bounties WHERE day = ?1 ORDER BY id"
        )?;
        let rows = stmt.query_map(params![day], |row| {
            Ok(Bounty {
                id: row.get(0)?,
                day: row.get(1)?,
                kind: row.get(2)?,
                difficulty: row.get(3)?,
                title: row.get(4)?,
                description: row.get(5)?,
                criterion: row.get(6)?,
                reward: row.get(7)?,
                status: row.get(8)?,
                progress: row.get(9)?,
                progress_label: row.get(10)?,
                accepted_at: row.get(11)?,
                completed_at: row.get(12)?,
                claimed_at: row.get(13)?,
            })
        })?;
        rows.collect()
    }

    pub fn insert_bounty(&self, b: &Bounty) -> SqlResult<()> {
        let conn = self.conn.lock();
        conn.execute(
            "INSERT INTO bounties (id, day, kind, difficulty, title, description, criterion, reward,
                                    status, progress, progress_label, accepted_at, completed_at, claimed_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)",
            params![
                b.id, b.day, b.kind, b.difficulty, b.title, b.description, b.criterion, b.reward,
                b.status, b.progress, b.progress_label, b.accepted_at, b.completed_at, b.claimed_at,
            ],
        )?;
        Ok(())
    }

    pub fn update_bounty(&self, b: &Bounty) -> SqlResult<()> {
        let conn = self.conn.lock();
        conn.execute(
            "UPDATE bounties SET status=?2, progress=?3, progress_label=?4,
                                  accepted_at=?5, completed_at=?6, claimed_at=?7
             WHERE id=?1",
            params![
                b.id, b.status, b.progress, b.progress_label,
                b.accepted_at, b.completed_at, b.claimed_at,
            ],
        )?;
        Ok(())
    }

    pub fn delete_bounties_for_day(&self, day: &str) -> SqlResult<()> {
        let conn = self.conn.lock();
        conn.execute("DELETE FROM bounties WHERE day = ?1", params![day])?;
        // Also drop any ledger rows tied to those bounties so a reset doesn't leave
        // stranded points behind. Bounty ledger rows use session_id = "bounty:<uuid>".
        conn.execute("DELETE FROM points_ledger WHERE session_id LIKE 'bounty:%'", params![])?;
        Ok(())
    }

    pub fn get_bounty(&self, id: &str) -> SqlResult<Option<Bounty>> {
        let conn = self.conn.lock();
        let mut stmt = conn.prepare(
            "SELECT id, day, kind, difficulty, title, description, criterion, reward,
                    status, progress, progress_label, accepted_at, completed_at, claimed_at
             FROM bounties WHERE id = ?1"
        )?;
        let mut rows = stmt.query_map(params![id], |row| {
            Ok(Bounty {
                id: row.get(0)?,
                day: row.get(1)?,
                kind: row.get(2)?,
                difficulty: row.get(3)?,
                title: row.get(4)?,
                description: row.get(5)?,
                criterion: row.get(6)?,
                reward: row.get(7)?,
                status: row.get(8)?,
                progress: row.get(9)?,
                progress_label: row.get(10)?,
                accepted_at: row.get(11)?,
                completed_at: row.get(12)?,
                claimed_at: row.get(13)?,
            })
        })?;
        Ok(rows.next().transpose()?)
    }

    /// Categories the user has spent >=5 min on across intervals, sorted by frequency desc.
    /// `since` is an ISO date string ("YYYY-MM-DD"); rows earlier than that are excluded.
    pub fn get_used_categories_since(&self, since: &str) -> SqlResult<Vec<(String, i64)>> {
        let conn = self.conn.lock();
        let mut stmt = conn.prepare(
            "SELECT category, COUNT(*) * 5 as secs FROM intervals
             WHERE date(start_ts) >= ?1 AND category != '' AND category != 'Invigil' AND status = 'on_task'
             GROUP BY category HAVING secs >= 300 ORDER BY secs DESC LIMIT 20"
        )?;
        let rows = stmt.query_map(params![since], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
        })?;
        rows.collect()
    }

    pub fn get_best_deep_focus_min(&self) -> SqlResult<i64> {
        // Longest single on-task run across all history, rounded down to minutes. Derived
        // from consecutive on_task intervals; approximate but good enough for a bounty goal.
        let conn = self.conn.lock();
        let mut stmt = conn.prepare(
            "SELECT start_ts, end_ts, status, session_id FROM intervals ORDER BY session_id, start_ts"
        )?;
        let rows = stmt.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, Option<String>>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
            ))
        })?;
        let mut best = 0i64;
        let mut run_start: Option<chrono::DateTime<chrono::Utc>> = None;
        let mut last_sid: Option<String> = None;
        for r in rows.filter_map(|r| r.ok()) {
            let (start_ts, end_ts, status, sid) = r;
            if Some(&sid) != last_sid.as_ref() {
                run_start = None;
                last_sid = Some(sid.clone());
            }
            let st = chrono::DateTime::parse_from_rfc3339(&start_ts)
                .ok().map(|d| d.with_timezone(&chrono::Utc));
            if status == "on_task" {
                if run_start.is_none() { run_start = st; }
                if let (Some(rs), Some(end)) = (
                    run_start,
                    end_ts.as_deref()
                        .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
                        .map(|d| d.with_timezone(&chrono::Utc)),
                ) {
                    let secs = (end - rs).num_seconds().max(0);
                    if secs / 60 > best { best = secs / 60; }
                }
            } else {
                run_start = None;
            }
        }
        Ok(best)
    }

    pub fn get_attention_trend(&self, days: i64) -> SqlResult<Vec<DayStats>> {
        let conn = self.conn.lock();
        let now = Utc::now();
        let mut map = std::collections::HashMap::new();

        let start = (now - chrono::Duration::days(days - 1)).format("%Y-%m-%d").to_string();
        let mut stmt = conn.prepare(
            "SELECT date(started_at) as d, COALESCE(SUM(duration_min),0), COALESCE(AVG(on_task_pct),0), COUNT(*), COALESCE(SUM(points_earned),0)
             FROM sessions WHERE date(started_at) >= ?1 AND ended_at IS NOT NULL GROUP BY d"
        )?;
        let rows = stmt.query_map(params![start], |row| {
            let d: String = row.get(0)?;
            let total_min: i64 = row.get(1)?;
            let avg_pct: f64 = row.get(2)?;
            let count: i64 = row.get(3)?;
            let pts: i64 = row.get(4)?;
            Ok((d.clone(), DayStats {
                date: d,
                total_minutes: total_min,
                on_task_pct: avg_pct,
                session_count: count,
                points: pts,
            }))
        })?;

        for r in rows {
            if let Ok((d, stats)) = r {
                map.insert(d, stats);
            }
        }

        // Fill consecutive 14 days
        let mut out = Vec::new();
        for i in (0..days).rev() {
            let date_str = (now - chrono::Duration::days(i)).format("%Y-%m-%d").to_string();
            if let Some(stats) = map.remove(&date_str) {
                out.push(stats);
            } else {
                out.push(DayStats {
                    date: date_str,
                    total_minutes: 0,
                    on_task_pct: 0.0,
                    session_count: 0,
                    points: 0,
                });
            }
        }

        Ok(out)
    }
}
