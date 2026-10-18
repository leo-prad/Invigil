# Invigil 0.7.0

Released 2026-10-17. First packaged installer for Invigil.

## Install
Run `Invigil Setup 0.7.0.exe`. It installs for your account. Windows SmartScreen may warn because it isn't signed: click **More info**, then **Run anyway**.

For AI drift detection, install [Ollama](https://ollama.com) and pull `gemma3:4b`. Without it, Invigil falls back to its built-in rules.

## What's in it
- Focus sessions with goals, a live timer, and recent-task suggestions
- Distraction detection: built-in rules first, then a local AI check for unclear windows
- Drift overlay when you wander off task, after a short grace period
- Points, streak multipliers, daily bounties, and an avatar shop
- Dashboard with attention trends and a color-coded calendar
- Inline session rename, and each app's real Windows icon in activity lists
- Plain scrolling with no springy bounce at the top and bottom
- AI chat apps (Claude, ChatGPT, Copilot, Gemini, Perplexity) now count as off task unless the window title matches your goal
- Desktop notch: a tiny pill at the top of the screen that expands into the Focus bar on hover (replaces the in-app leaf bar)
- Smarter utility tracking: Explorer, Snipping Tool and similar get 15 seconds per window and a 2-minute total before counting as a drift
- Invigil's own overlay and notch never count as drifts or activity rows
- Session graph: thin smooth line that dips below the midline (red) during drifts
- Timers tick every second, and only changing digits spin
- Tray icon with Open and Quit, and only one icon
- Dashboard Focus time shows the total for the range, so it matches the graph
- Removed the distraction denylist; the AI judges every app against your goal

Installer checksum: `SHA256.txt`.
