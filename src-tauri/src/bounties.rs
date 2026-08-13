use crate::db::{Bounty, Database, PointsEntry};
use chrono::{Duration as ChronoDuration, Local, TimeZone, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

// A day's pool holds this many bounties; enforce elsewhere too.
const POOL_SIZE: usize = 3;

/// Criterion is stored as the `criterion` TEXT column in the `bounties` table as JSON,
/// tagged by `type`. Values inside are absolute targets — `progress` on the row is the
/// fraction complete (0..1) so the UI never has to reparse this to render a bar.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Criterion {
    /// A single session with at least `target_min` on-task minutes (does not have to be
    /// contiguous within the session).
    SessionOnTaskMin { target_min: i64 },
    /// A completed session at least `min_len_min` minutes long with `drift_count == 0`.
    SessionZeroDrift { min_len_min: i64 },
    /// Sum of on-task minutes across today's completed sessions >= `target_min`.
    DailyOnTaskMin { target_min: i64 },
    /// Number of completed sessions today >= `target`.
    SessionsCount { target: i64 },
    /// Beat all-time deep-focus baseline: any completed session's deep_focus_min >= `target_min`.
    BeatBest { target_min: i64 },
    /// At least `target_sec` on-task seconds on the given category today (interval-based).
    UseCategory { category: String, target_sec: i64 },
    /// Complete a session >= `min_len_min` with zero off_task seconds on `category`.
    AvoidCategory { category: String, min_len_min: i64 },
}

// ─── Time helpers ────────────────────────────────────────────────────

pub fn local_today() -> String {
    Local::now().format("%Y-%m-%d").to_string()
}

/// Seconds until the next local-midnight. Frontend has its own JS countdown, but the
/// backend uses this to sanity-check that the current day's pool is still current.
pub fn seconds_until_local_midnight() -> i64 {
    let now = Local::now();
    let tomorrow = (now + ChronoDuration::days(1)).date_naive();
    let midnight = Local
        .from_local_datetime(&tomorrow.and_hms_opt(0, 0, 0).unwrap())
        .single()
        .unwrap_or(now);
    (midnight - now).num_seconds().max(0)
}

// ─── Generation ──────────────────────────────────────────────────────

/// Ensure today's 3 bounties exist. Existing rows for other days are ignored — we don't
/// prune (kept for a lightweight audit trail). If today already has bounties, no-op.
pub fn ensure_today_pool(db: &Database) {
    let day = local_today();
    let existing = db.get_bounties_for_day(&day).unwrap_or_default();
    if existing.len() >= POOL_SIZE { return; }

    let mut picks = build_pool(db);
    // Truncate if the pool builder over-produced.
    picks.truncate(POOL_SIZE);
    // Pad with a safe easy default if under-produced (only possible if history is empty
    // and the fallback templates got filtered out somehow).
    while picks.len() < POOL_SIZE {
        picks.push(easy_default());
    }

    for p in picks {
        let b = Bounty {
            id: Uuid::new_v4().to_string(),
            day: day.clone(),
            kind: p.kind,
            difficulty: p.difficulty,
            title: p.title,
            description: p.description,
            criterion: serde_json::to_string(&p.criterion).unwrap_or_else(|_| "{}".into()),
            reward: p.reward,
            status: "available".into(),
            progress: 0.0,
            progress_label: String::new(),
            accepted_at: None,
            completed_at: None,
            claimed_at: None,
        };
        let _ = db.insert_bounty(&b);
    }
}

struct Pick {
    kind: String,
    difficulty: String,
    title: String,
    description: String,
    criterion: Criterion,
    reward: i64,
}

fn easy_default() -> Pick {
    Pick {
        kind: "reinforcing".into(),
        difficulty: "easy".into(),
        title: "Warm up".into(),
        description: "Complete one 15-minute focus session today.".into(),
        criterion: Criterion::SessionOnTaskMin { target_min: 15 },
        reward: 300,
    }
}

fn build_pool(db: &Database) -> Vec<Pick> {
    // Data the templates draw on. All optional — every read has a graceful "no history" path.
    let two_weeks_ago = (Utc::now() - ChronoDuration::days(14)).format("%Y-%m-%d").to_string();
    let categories_recent = db.get_used_categories_since(&two_weeks_ago).unwrap_or_default();
    let distractions = db.get_week_distraction_stats().unwrap_or_default();
    let best_deep = db.get_best_deep_focus_min().unwrap_or(0);

    // Deterministic-per-day so the same day always shows the same pool if regenerated.
    let seed_day = local_today();
    let seed: u64 = seed_day.bytes().fold(0u64, |a, b| a.wrapping_mul(31).wrapping_add(b as u64));
    let mut rng = MiniRng::new(seed);

    let mut picks: Vec<Pick> = Vec::with_capacity(POOL_SIZE);

    // ── Slot 1: an EASY bounty. Rotate through a few flavors so it's not the same daily. ──
    let easy_pool: Vec<Pick> = vec![
        Pick {
            kind: "reinforcing".into(),
            difficulty: "easy".into(),
            title: "Warm up".into(),
            description: "Complete one 15-minute focus session today.".into(),
            criterion: Criterion::SessionOnTaskMin { target_min: 15 },
            reward: 300,
        },
        Pick {
            kind: "reinforcing".into(),
            difficulty: "easy".into(),
            title: "Just show up".into(),
            description: "Start and finish any one session today.".into(),
            criterion: Criterion::SessionsCount { target: 1 },
            reward: 300,
        },
        Pick {
            kind: "reinforcing".into(),
            difficulty: "easy".into(),
            title: "First half hour".into(),
            description: "Rack up 30 total on-task minutes across today's sessions.".into(),
            criterion: Criterion::DailyOnTaskMin { target_min: 30 },
            reward: 350,
        },
    ];
    picks.push(rng.pick(easy_pool));

    // ── Slot 2: REINFORCING — lean on user's own patterns. Falls back to a generic
    //    medium goal if there's no usable history yet. ──
    let reinforcing = if let Some((cat, secs)) = categories_recent.first().cloned() {
        // Aim ~1.5x the average daily use of this category over 14 days, capped at 45 min.
        let daily_avg_min = (secs / 14 / 60).max(15);
        let target_min = ((daily_avg_min as f64 * 1.5).round() as i64).min(45).max(20);
        Pick {
            kind: "reinforcing".into(),
            difficulty: "medium".into(),
            title: format!("Back to {}", cat),
            description: format!(
                "You've been putting real time into {} lately. Push {} on-task minutes there today.",
                cat, target_min
            ),
            criterion: Criterion::UseCategory { category: cat, target_sec: target_min * 60 },
            reward: 750,
        }
    } else if best_deep > 0 {
        Pick {
            kind: "reinforcing".into(),
            difficulty: "medium".into(),
            title: "Match your best".into(),
            description: format!(
                "Your longest deep-focus run is {} min. Hit that in one session today.",
                best_deep
            ),
            criterion: Criterion::SessionOnTaskMin { target_min: best_deep },
            reward: 800,
        }
    } else {
        Pick {
            kind: "reinforcing".into(),
            difficulty: "medium".into(),
            title: "Uninterrupted".into(),
            description: "Complete a 25-minute session with zero drifts.".into(),
            criterion: Criterion::SessionZeroDrift { min_len_min: 25 },
            reward: 750,
        }
    };
    picks.push(reinforcing);

    // ── Slot 3: EXPLORATORY — deliberately not just repeating a pattern. Prefers an
    //    avoidance challenge (top known distraction), then a beat-best, then a variety
    //    prompt. ──
    let exploratory_pool: Vec<Pick> = {
        let mut pool: Vec<Pick> = Vec::new();
        if let Some(top) = distractions.first().cloned() {
            pool.push(Pick {
                kind: "exploratory".into(),
                difficulty: "medium".into(),
                title: format!("{}-free session", top.name),
                description: format!(
                    "Complete a 30-minute session without touching {} — not even to check.",
                    top.name
                ),
                criterion: Criterion::AvoidCategory { category: top.name.clone(), min_len_min: 30 },
                reward: 900,
            });
        }
        if best_deep >= 10 {
            let target = best_deep + 5;
            pool.push(Pick {
                kind: "exploratory".into(),
                difficulty: "hard".into(),
                title: "Beat your ceiling".into(),
                description: format!(
                    "Push past your best deep-focus streak — {} minutes in one uninterrupted run.",
                    target
                ),
                criterion: Criterion::BeatBest { target_min: target },
                reward: 1500,
            });
        }
        pool.push(Pick {
            kind: "exploratory".into(),
            difficulty: "medium".into(),
            title: "Zero-phone lockdown".into(),
            description: "Complete a 30-minute session with your phone in another room. No apps that mirror it either (Messages, Discord, etc.).".into(),
            criterion: Criterion::AvoidCategory { category: "Discord".into(), min_len_min: 30 },
            reward: 800,
        });
        pool.push(Pick {
            kind: "exploratory".into(),
            difficulty: "hard".into(),
            title: "The long haul".into(),
            description: "Finish a 45-minute session with zero drifts. No excuses, no context-switching.".into(),
            criterion: Criterion::SessionZeroDrift { min_len_min: 45 },
            reward: 1400,
        });
        pool.push(Pick {
            kind: "exploratory".into(),
            difficulty: "medium".into(),
            title: "Try three".into(),
            description: "Complete three separate sessions today — even short ones. Prove the switch is easy.".into(),
            criterion: Criterion::SessionsCount { target: 3 },
            reward: 900,
        });
        pool
    };
    picks.push(rng.pick(exploratory_pool));

    picks
}

// ─── Progress evaluation ─────────────────────────────────────────────

/// Recompute progress + status for every bounty on `day`. Cheap: reads today's sessions
/// (a handful of rows) + intervals for at most those sessions. Called on demand from the
/// UI and on session end.
pub fn refresh_progress(db: &Database, day: &str) {
    let bounties = match db.get_bounties_for_day(day) {
        Ok(v) => v,
        Err(_) => return,
    };
    let sessions = db.get_sessions_in_range(day, day).unwrap_or_default();

    for mut b in bounties {
        // Terminal states never regress.
        if b.status == "claimed" || b.status == "completed" { continue; }
        // Only accepted bounties get progress; available ones show 0 / their target.
        let criterion: Criterion = match serde_json::from_str(&b.criterion) {
            Ok(c) => c,
            Err(_) => continue,
        };

        let (progress, label, done) = evaluate(db, &criterion, &sessions);
        b.progress = progress;
        b.progress_label = label;
        if done && b.status == "accepted" {
            b.status = "completed".into();
            b.completed_at = Some(Utc::now().to_rfc3339());
        }
        let _ = db.update_bounty(&b);
    }
}

fn evaluate(
    db: &Database,
    criterion: &Criterion,
    sessions: &[crate::db::Session],
) -> (f64, String, bool) {
    match criterion {
        Criterion::SessionOnTaskMin { target_min } => {
            let best = sessions.iter()
                .filter(|s| s.ended_at.is_some())
                .map(|s| session_on_task_min(s))
                .max().unwrap_or(0);
            let target = *target_min;
            (frac(best, target), format!("{} / {} min", best, target), best >= target)
        }
        Criterion::SessionZeroDrift { min_len_min } => {
            let done = sessions.iter().any(|s|
                s.ended_at.is_some()
                && s.drift_count == 0
                && s.duration_min.unwrap_or(0) >= *min_len_min
            );
            let best_min = sessions.iter()
                .filter(|s| s.ended_at.is_some() && s.drift_count == 0)
                .map(|s| s.duration_min.unwrap_or(0)).max().unwrap_or(0);
            (
                if done { 1.0 } else { frac(best_min, *min_len_min) },
                format!("{} / {} min (0 drifts)", best_min, min_len_min),
                done,
            )
        }
        Criterion::DailyOnTaskMin { target_min } => {
            let total: i64 = sessions.iter().filter(|s| s.ended_at.is_some())
                .map(|s| session_on_task_min(s)).sum();
            (frac(total, *target_min), format!("{} / {} min today", total, target_min), total >= *target_min)
        }
        Criterion::SessionsCount { target } => {
            let n = sessions.iter().filter(|s| s.ended_at.is_some()).count() as i64;
            (frac(n, *target), format!("{} / {} sessions", n, target), n >= *target)
        }
        Criterion::BeatBest { target_min } => {
            // Approximate deep_focus from the session's on-task minutes since we don't
            // persist deep_focus per session. Good enough for "did you push past your ceiling".
            let best = sessions.iter().filter(|s| s.ended_at.is_some())
                .map(|s| session_on_task_min(s)).max().unwrap_or(0);
            (frac(best, *target_min), format!("{} / {} min", best, target_min), best >= *target_min)
        }
        Criterion::UseCategory { category, target_sec } => {
            let mut secs = 0i64;
            for s in sessions {
                let ivs = db.get_session_intervals(&s.id).unwrap_or_default();
                for iv in ivs {
                    if iv.status == "on_task"
                        && iv.category.eq_ignore_ascii_case(category)
                    {
                        if let (Ok(st), Some(en)) = (
                            chrono::DateTime::parse_from_rfc3339(&iv.start_ts),
                            iv.end_ts.as_deref()
                                .and_then(|e| chrono::DateTime::parse_from_rfc3339(e).ok()),
                        ) {
                            secs += (en - st).num_seconds().max(0);
                        }
                    }
                }
            }
            let target_min = target_sec / 60;
            let cur_min = secs / 60;
            (frac(cur_min, target_min), format!("{} / {} min on {}", cur_min, target_min, category), secs >= *target_sec)
        }
        Criterion::AvoidCategory { category, min_len_min } => {
            let mut done = false;
            let mut best_len = 0i64;
            for s in sessions {
                if s.ended_at.is_none() { continue; }
                let len = s.duration_min.unwrap_or(0);
                if len < *min_len_min { continue; }
                let ivs = db.get_session_intervals(&s.id).unwrap_or_default();
                let touched = ivs.iter().any(|iv|
                    iv.category.eq_ignore_ascii_case(category)
                    && iv.status != "on_task"
                );
                if !touched { done = true; best_len = best_len.max(len); }
            }
            (
                if done { 1.0 } else { 0.0 },
                if done { format!("Clean {} min session — no {}", best_len, category) }
                else { format!("No qualifying session yet — avoid {}", category) },
                done,
            )
        }
    }
}

fn frac(cur: i64, target: i64) -> f64 {
    if target <= 0 { return 0.0; }
    ((cur as f64) / (target as f64)).clamp(0.0, 1.0)
}

/// Approximate a session's on-task minutes as its duration × on_task_pct.
/// The exact on_task_sec isn't stored on the sessions row, so we reconstruct from what is.
fn session_on_task_min(s: &crate::db::Session) -> i64 {
    let dur = s.duration_min.unwrap_or(0) as f64;
    ((dur * (s.on_task_pct / 100.0)).round() as i64).max(0)
}

// ─── Claim ───────────────────────────────────────────────────────────

/// Claim a completed bounty: writes a positive ledger entry and marks the row `claimed`.
/// Returns the reward on success, or an error string the frontend can surface.
pub fn claim(db: &Database, id: &str) -> Result<i64, String> {
    let mut b = match db.get_bounty(id) {
        Ok(Some(b)) => b,
        Ok(None) => return Err("Bounty not found".into()),
        Err(e) => return Err(e.to_string()),
    };
    if b.status == "claimed" { return Err("Already claimed".into()); }
    if b.status != "completed" { return Err("Not completed yet".into()); }

    let entry = PointsEntry {
        id: Uuid::new_v4().to_string(),
        // Bounty rewards are not tied to a single session — use the bounty id in the
        // session_id column so the ledger row has a stable, unique reference.
        session_id: format!("bounty:{}", b.id),
        amount: b.reward,
        reason: format!("Bounty: {}", b.title),
        timestamp: Utc::now().to_rfc3339(),
    };
    db.add_points(&entry).map_err(|e| e.to_string())?;

    b.status = "claimed".into();
    b.claimed_at = Some(Utc::now().to_rfc3339());
    db.update_bounty(&b).map_err(|e| e.to_string())?;
    Ok(b.reward)
}

pub fn accept(db: &Database, id: &str) -> Result<(), String> {
    let mut b = match db.get_bounty(id) {
        Ok(Some(b)) => b,
        Ok(None) => return Err("Bounty not found".into()),
        Err(e) => return Err(e.to_string()),
    };
    if b.status != "available" { return Err("Cannot accept this bounty".into()); }
    b.status = "accepted".into();
    b.accepted_at = Some(Utc::now().to_rfc3339());
    db.update_bounty(&b).map_err(|e| e.to_string())?;
    Ok(())
}

// ─── Deterministic tiny PRNG ─────────────────────────────────────────

/// xorshift64 seeded once per day, so the same date yields the same pool if regenerated
/// mid-day (e.g. after a crash). We can't use `rand` — the project has zero non-listed
/// deps and I'd rather not add one for four templated bounties.
struct MiniRng(u64);

impl MiniRng {
    fn new(seed: u64) -> Self { Self(seed.max(1)) }
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13; x ^= x >> 7; x ^= x << 17;
        self.0 = x;
        x
    }
    fn pick<T>(&mut self, mut items: Vec<T>) -> T {
        let idx = (self.next() as usize) % items.len();
        items.swap_remove(idx)
    }
}
