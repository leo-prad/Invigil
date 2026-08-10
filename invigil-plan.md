# Invigil — Full Build Plan

**Mascot:** Professor Xeno — cartoon alien professor, black-and-white suit, round spectacles, big cartoony eyes, chibi proportions.

---

## 0. What Invigil Actually Is

Invigil is a desktop app that sits on your HP Omnibook the whole time you're doing schoolwork, watching what's on your screen, and calling you out the moment you drift. Think of it as a focus-tracking daemon with a face — it runs quietly in the background or tray, and only becomes loud when you need it to.

**What you'd actually see, start to finish:**

1. **Start a session.** You open Invigil, pick or create a profile ("Calc HW," "CS Reading"), optionally set a timer, and type/confirm your goal for this block. Professor Xeno gives a small "let's lock in" acknowledgment and the session begins.
2. **You work, Invigil watches.** In the background, every 5 seconds it checks what app/window/tab is focused. As long as you're in-profile (Overleaf, Desmos, your notes), nothing happens — it's invisible. This is Tier 0, no AI, just pattern matching.
3. **You drift.** You open YouTube. After a short grace period (so a 10-second glance doesn't get punished), Invigil isn't sure if this is a quick reference check or you slipping — so it hands off to the local 4B model to read the tab title, and if that's still ambiguous, takes a quick screenshot to judge from context.
4. **It calls you out.** If it decides you're off-task, the cyberpunk overlay fires — pulsating red/cyan border, glitch warning text, screen-shake jitter, right on top of whatever you're doing. It fades once you're back on-task. Repeated drift in the same session makes it louder each time.
5. **Session ends.** You get a summary: on-task %, minutes locked in, points earned. Points go toward unlocking new cosmetic looks for Professor Xeno in the avatar shop — a genuine reward loop, not just a nag tool.
6. **Over days/weeks**, a dashboard shows your focus trends — which times of day you actually lock in, which subjects you drift on most, your streaks.

So functionally it's three things stitched together: a **behavioral sensor** (what are you doing right now), a **judgment layer** (is that okay, given your stated goal), and a **feedback loop** (a visceral alert now, plus long-term points/stats to make locking in feel like progress instead of punishment).

---

## 1. Core Architecture (fully local, tiered)

Three escalating tiers, each one only fires when the tier below it isn't confident. This keeps 95%+ of ticks essentially free.

**Tier 0 — Rule engine (every 5s, no model)**
- Active window title + process name + browser tab title, checked against a user-defined allowlist/denylist per session ("Calc HW" session → Overleaf, Desmos, course PDF = work; YouTube, Discord = play).
- Instant classification for anything unambiguous. This alone probably resolves 80% of ticks.

**Tier 1 — Local text LLM (only on ambiguous titles)**
- Your 4B model (Gemma 3 4B or Qwen3-4B, whichever feels snappier — both fine) gets just the window/tab title text and the stated goal, returns on-task/off-task/unsure.
- Still text-only, still sub-second.

**Tier 2 — Local vision escalation (only on "unsure")**
- Take a screenshot, downscale it hard (small resolution, this isn't a legibility task, just gist), feed it to the same model if it supports vision, or a small vision model if not (e.g. a quantized LLaVA/Moondream variant — worth checking what runs on your VRAM before committing).
- This is the right instinct — reserve it for genuinely ambiguous cases only, since vision inference is heavier. Maybe 5-10% of ticks max.

**Tier 3 — Optional Claude Code CLI ping (rare, opt-in)**
- Only for end-of-session summaries or after repeated off-task streaks where you want a sharper, more personalized nudge than a canned message. Shares your Pro subscription's 5-hour pool with regular chat use, so cap this to a handful of calls per session, never per-tick.
- Fully optional — the whole system works with Tier 3 off.

---

## 2. Work vs. Play Differentiation

This is the crux of the whole thing, so give it real structure instead of one global rule:

- **Session profiles**: each focus session declares a goal ("Calc HW," "CS Reading") and a set of allowed apps/sites/window-title-patterns. Reuse profiles across sessions.
- **Category tags**: maintain a personal tag library (Work, Research, Play, Ambiguous) that both the rule engine and Tier 1 model reference — lets the model reason "is Discord work right now?" contextually instead of a hardcoded blacklist.
- **Context override**: some apps are dual-use (browser, VSCode, Discord). For these, Tier 1 checks the *tab title / channel name*, not just the app name — "Discord — #cs101-study-group" reads differently than "Discord — #general."
- **Learning loop**: let yourself correct misclassifications (a quick keybind: "actually this is work") and log corrections — future version could fine-tune the ruleset or few-shot examples from your own corrections.

---

## 3. Feature List

### Monitoring & Detection
- 5s-cadence active window/tab tracking
- Tiered escalation (rules → text LLM → vision LLM → optional cloud ping)
- Session profiles with per-session allow/deny rules
- Manual correction/override keybind

### Focus Mode
- Timer-based sessions (Pomodoro-style or freeform) or continuous background mode
- Pre-session goal declaration prompt
- Grace period before flagging (e.g. 15-20s off-task before triggering, so you're not punished for a quick glance)

### Alerts (Cyberpunk Overlay)
- Full-screen transparent, click-through, always-on-top warning overlay
- Pulsating red/cyan border glow, glitch-text warning, CSS shake jitter (content only, not the real OS window)
- Escalating intensity — first nudge subtle, repeated off-task gets louder/more aggressive
- Auto-dismiss/fade once back on-task

### Gamification
- Points accrued per on-task minute (or per completed session, or both)
- Points spendable in an avatar shop — cosmetic skins/poses/accessories for Professor Xeno
- Streaks / daily-focus stats
- Maybe: point multiplier for hitting a full session with zero interruptions

### Analytics Dashboard
- Daily/weekly on-task % by category
- Time-of-day focus heatmap (when are you actually locking in?)
- Per-session history log (goal, duration, on-task %, points earned)

### Settings & Customization
- Editable allow/deny lists per profile
- Sensitivity slider (how aggressive the escalation/alert is)
- Model picker (swap Gemma/Qwen/whatever locally)
- Quiet hours / do-not-disturb override

### Stretch Ideas
- Export weekly report as a shareable card (flex your lock-in streak)
- "Study buddy" mode — shared session with a friend, both being monitored, light competitive points
- Voice nudge option instead of just visual (Professor Xeno TTS one-liner) — off by default since it could get annoying fast
- Browser extension companion for finer tab-level detail than window title alone gives you

---

## 4. Data Model

```
sessions(id, goal, profile_id, started_at, ended_at, points_earned)
intervals(id, session_id, status, category, start_ts, end_ts)
profiles(id, name, allow_patterns, deny_patterns)
corrections(id, interval_id, original_status, corrected_status, timestamp)
points_ledger(id, session_id, amount, reason, timestamp)
avatars(id, name, cost, unlocked, equipped)
```

---

## 5. Rough Build Order

1. Tier 0 rule engine + interval logging (no AI yet) — get the skeleton working and prove the DB/session model
2. Cyberpunk overlay as a standalone component, triggered manually first
3. Wire Tier 0 → overlay trigger
4. Add Tier 1 local LLM for ambiguous cases
5. Add points + basic avatar shop (even ugly placeholder avatars)
6. Add Tier 2 vision escalation
7. Analytics dashboard
8. Polish: profiles, sensitivity settings, corrections/learning loop
