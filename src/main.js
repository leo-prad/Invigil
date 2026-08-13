// ─────────────────────────────────────────────────────────────────────
// Invigil — main.js
// Frontend logic wired to Tauri backend commands + event listeners.
// Falls back to demo data when the backend hasn't collected real data yet.
// ─────────────────────────────────────────────────────────────────────

const { invoke } = window.__TAURI__.core;
const { listen, emit } = window.__TAURI__.event;

// Desktop app, not a webpage — kill the WebView's default right-click menu ("Refresh",
// "Save as", "Print" via WebView2), and let the browser drag start image/link previews
// nowhere. Text inputs are exempt so the user can still right-click paste into the
// justification textarea.
window.addEventListener('contextmenu', (e) => {
  const t = e.target;
  if (t && (t.tagName === 'INPUT' || t.tagName === 'TEXTAREA' || t.isContentEditable)) return;
  e.preventDefault();
});
window.addEventListener('dragstart', (e) => e.preventDefault());

// ─── Data (demo fallback until real sessions exist) ──────────────────

const TODAY = new Date().toISOString().slice(0, 10);

// Demo data — shown when the DB is empty
const demoDayData = {
  // Will be replaced with real data from get_dashboard_data
};

const demoTrend14 = [];

// ─── Live state ──────────────────────────────────────────────────────

let liveData = null;     // DashboardData from backend
let sessionActive = false;
let timerInterval = null;
let elapsedSec = 0;
let currentGoal = '';    // kept in sync from session-tick-result, used by the drift overlay
let currentDriftApp = ''; // most recent drift app; used by "This is actually work" to allowlist it
let prevElapsedText = '';
let prevLeafTimeText = '--:--';
const state = { mode: 'overall', start: null, end: null };

// ─── Init ────────────────────────────────────────────────────────────

async function init() {
  if (window.location.hash === '#overlay-leaf') {
    document.body.classList.add('overlay-mode');
    document.querySelector('.page-wrap')?.remove();
    document.getElementById('cyberOverlay')?.remove();
    const leaf = document.getElementById('leafLetoutCard');
    if (leaf) leaf.style.display = 'flex';
    setupListeners();
    return;
  }
  if (window.location.hash === '#overlay-drift') {
    document.body.classList.add('overlay-mode');
    document.querySelector('.page-wrap')?.remove();
    document.getElementById('leafLetoutCard')?.remove();
    // Do NOT force #cyberOverlay visible here — it stays at its default display:none
    // until a real drift-detected event fills it in and shows it. Forcing it on at
    // load meant the static placeholder markup was what got shown if that event was
    // ever missed, which read as "the app is showing hardcoded fake data."
    setupOverlayListeners();
    return;
  }

  try {
    liveData = await invoke('get_dashboard_data');
  } catch (e) {
    console.warn('No backend data yet:', e);
  }

  buildCalendar();

  // Calendar month navigation
  const calNav = document.querySelector('.cal-nav');
  if (calNav) {
    const [prevBtn, nextBtn] = calNav.querySelectorAll('button');
    prevBtn.addEventListener('click', () => {
      calViewMonth--;
      if (calViewMonth < 0) { calViewMonth = 11; calViewYear--; }
      buildCalendar();
    });
    nextBtn.addEventListener('click', () => {
      calViewMonth++;
      if (calViewMonth > 11) { calViewMonth = 0; calViewYear++; }
      buildCalendar();
    });
  }
  document.addEventListener('keydown', (e) => {
    const dashboardVisible = document.getElementById('dashboard')?.style.display !== 'none';
    if (!dashboardVisible) return;
    if (e.key === 'ArrowLeft') {
      calViewMonth--;
      if (calViewMonth < 0) { calViewMonth = 11; calViewYear--; }
      buildCalendar();
    } else if (e.key === 'ArrowRight') {
      calViewMonth++;
      if (calViewMonth > 11) { calViewMonth = 0; calViewYear++; }
      buildCalendar();
    }
  });

  setMode({ mode: 'overall', start: null, end: null });
  updateGreeting();
  updateStatTiles();

  // Check if a session is already running (e.g. app restart)
  try {
    const sessionState = await invoke('get_session_state');
    if (sessionState.active) {
      sessionActive = true;
      currentSessionId = sessionState.session_id;
      navigateTo('session');
      applySessionState(sessionState);
    }
  } catch (e) {
    console.warn('Could not get session state:', e);
  }

  setSessionOffState();
  renderDashActivity();
  renderInsights();
  setupListeners();
  setupSettings();
  setupAdvPanel();
  checkOllamaOnLoad();
}

// Detect whether the local AI (Ollama) is reachable at app start. If it's installed but
// not running, launch it silently in the background. If it isn't installed at all, show a
// gentle banner suggesting install for better classifications — nothing about it is
// required, everything still works via the rule/heuristic fallback.
async function checkOllamaOnLoad() {
  const tip = document.getElementById('ollamaTip');
  let status;
  try { status = await invoke('get_ollama_status'); } catch(e) { return; }

  if (status === 'running') {
    if (tip) tip.style.display = 'none';
    return;
  }

  if (status === 'installed_not_running') {
    try {
      const launched = await invoke('try_launch_ollama');
      if (launched) {
        if (tip) tip.style.display = 'none';
        return;
      }
    } catch(e) {}
    // Fall through to the not-installed banner if launch didn't stick.
  }

  if (tip) {
    tip.innerHTML = `
      <span class="ico">💡</span>
      <div>
        <b>Local AI isn't set up.</b> Invigil falls back to a keyword-match rule when it can't ask a local model — that misses a lot of nuance (e.g. "watching a Khan Academy math video" looks like YouTube, not studying).
        Install <a href="https://ollama.com" target="_blank" rel="noopener">Ollama</a> and pull the <code>gemma:e4b</code> model for much better classification.
      </div>
      <button class="close" id="ollamaTipClose" title="Dismiss">×</button>
    `;
    tip.style.display = 'flex';
    document.getElementById('ollamaTipClose')?.addEventListener('click', () => { tip.style.display = 'none'; });
  }
}

// ─── Greeting ────────────────────────────────────────────────────────

// Pick a warm, hour-aware greeting for the dashboard headline. Picks a bucket by wall-clock
// hour, then picks a line inside the bucket by day-of-month so the same day always renders
// the same line (no flicker between polls) but the collection rotates day to day.
function pickTimeGreeting() {
  const now = new Date();
  const h = now.getHours();
  const doy = now.getDate();  // day-of-month drives rotation
  const emph = t => `<em>${t}</em>`;
  const wee   = [`Wee hours, ${emph('Leo?')}`, `Up already — or ${emph('still up?')}`, `The world's asleep, ${emph('Leo.')}`];
  const early = [`Early bird, ${emph('Leo.')}`, `Dawn patrol, ${emph('Leo.')}`, `Sunrise scholar, ${emph('Leo.')}`];
  const morn  = [`Good morning, Leo — ${emph("let's lock in.")}`, `Rise and grind, ${emph('Leo.')}`, `Morning shift, ${emph('Leo.')}`];
  const noon  = [`Midday, Leo — ${emph('halfway there.')}`, `Lunch-hour lock-in, ${emph('Leo?')}`, `Afternoon push, ${emph('Leo.')}`];
  const aft   = [`Good afternoon, Leo — ${emph("stay sharp.")}`, `The 3pm slog, ${emph('Leo.')}`, `Afternoon focus, ${emph('Leo.')}`];
  const eve   = [`Good evening, Leo — ${emph("still going?")}`, `Golden-hour grind, ${emph('Leo.')}`, `Evening session, ${emph('Leo.')}`];
  const night = [`Night owl, ${emph('Leo?')}`, `Burning the midnight oil, ${emph('Leo.')}`, `Late-night lock-in, ${emph('Leo.')}`];
  let pool;
  if      (h < 5)  pool = wee;
  else if (h < 8)  pool = early;
  else if (h < 12) pool = morn;
  else if (h < 14) pool = noon;
  else if (h < 17) pool = aft;
  else if (h < 21) pool = eve;
  else             pool = night;
  return pool[doy % pool.length];
}

// Warm session-page headline. Extracts a subject keyword from the goal / description and
// wraps it in a "you've got this" phrasing so the header doesn't read as raw task instructions.
// If no subject keyword lands, falls back to a generic warm line + the raw goal underneath.
function pickSessionGreeting(goal, description) {
  const text = `${goal || ''} ${description || ''}`.toLowerCase();
  const subjects = [
    { re: /\b(calc(ulus)?|derivative|integral|integ)/, label: 'calc' },
    { re: /\b(algebra|linear algebra|matrix|matrices)/, label: 'algebra' },
    { re: /\b(geometry|trig|trigonometry|proof)/, label: 'geometry' },
    { re: /\b(math|chapter\s*\d|problem\s*set|homework|hw)/, label: 'math' },
    { re: /\b(phys(ics)?|kinematics|mechanics)/, label: 'physics' },
    { re: /\b(chem(istry)?|reaction|stoichiometry|molar)/, label: 'chem' },
    { re: /\b(bio(logy)?|cell|mitosis|enzyme|dna)/, label: 'bio' },
    { re: /\b(hist(ory)?|civil war|revolution|world war)/, label: 'history' },
    { re: /\b(essay|paper|writing|thesis|draft)/, label: 'your writing' },
    { re: /\b(code|coding|program|dev|leetcode|debug|feature)/, label: 'code' },
    { re: /\b(spanish|french|german|japanese|language|vocab)/, label: 'language' },
    { re: /\b(read(ing)?|book|novel|chapter\s+of)/, label: 'reading' },
    { re: /\b(study|review|revise|exam|test|midterm|final)/, label: 'study' },
  ];
  const emph = t => `<em>${t}</em>`;
  const goalSafe = (goal || '').replace(/</g, '&lt;').slice(0, 140);
  for (const s of subjects) {
    if (s.re.test(text)) {
      const openers = [
        `Back into ${emph(s.label)}?`,
        `Locked in on ${emph(s.label)}.`,
        `Deep work on ${emph(s.label)} — you've got this.`,
        `${emph(s.label.charAt(0).toUpperCase() + s.label.slice(1))} time.`,
      ];
      const doy = new Date().getDate();
      return `${openers[doy % openers.length]}<div class="session-goal-tag">${goalSafe}</div>`;
    }
  }
  // No keyword landed — still warmer than raw goal.
  return `Session in progress — <em>focus mode.</em><div class="session-goal-tag">${goalSafe}</div>`;
}

function updateGreeting() {
  const streakDays = liveData?.streak?.current ?? 0;
  const greeting = document.querySelector('h1.greeting');
  const sub = document.querySelector('.greeting-sub');
  if (!greeting || !sub) return;

  greeting.innerHTML = pickTimeGreeting();
  if (streakDays > 1) {
    sub.textContent = `${streakDays} day${streakDays === 1 ? '' : 's'} locked in. ${formatDateNice(TODAY)} · no session running.`;
  } else {
    sub.textContent = `${formatDateNice(TODAY)} · no session running.`;
  }

  // Update streak card
  const streakNum = document.querySelector('.streak-num');
  const streakWord = document.querySelector('.streak-word');
  if (streakNum) streakNum.textContent = streakDays;
  if (streakWord) streakWord.textContent = streakDays === 1 ? 'day' : 'days in a row';

  // Streak side stats
  const sides = document.querySelectorAll('.streak-side span b');
  if (sides.length >= 2 && liveData?.streak) {
    sides[0].textContent = liveData.streak.best;
    const totalH = Math.floor(liveData.streak.month_total_minutes / 60);
    const totalM = liveData.streak.month_total_minutes % 60;
    sides[1].textContent = totalH > 0 ? `${totalH}h ${totalM}m` : `${totalM}m`;
  }

  // Update mascot bubble
  const bubble = document.querySelector('.mascot-bubble');
  if (bubble && streakDays > 0) {
    bubble.innerHTML = `${streakDays} day${streakDays === 1 ? '' : 's'}. Don't break the chain.<span class="sig">— Professor Xeno</span>`;
  }

  // Update stat tiles
  updateStatTiles();
}

// Tiny inline arrow SVGs shown next to a tile's headline number to indicate direction of
// change vs. the compare-baseline. `arrowSvg('up')` renders green up-arrow, 'down' is orange.
// Kept small (11px) so it sits beside the serif number without stealing focus.
function arrowSvg(dir) {
  if (dir !== 'up' && dir !== 'down') return '';
  const cls = dir === 'up' ? 'trend-arrow up' : 'trend-arrow down';
  // Up arrow is a chevron pointing up; down is the flipped variant.
  const path = dir === 'up' ? 'M3 8 L7 4 L11 8' : 'M3 4 L7 8 L11 4';
  return `<svg class="${cls}" viewBox="0 0 14 12" width="12" height="12" fill="none" stroke="currentColor" stroke-width="2.2" stroke-linecap="round" stroke-linejoin="round"><path d="${path}"/></svg>`;
}

// Given current and baseline numbers, return 'up' | 'down' | null. Threshold guards against
// noise — if the delta is within 3% of the baseline (or ±1 for tiny values) we call it flat.
function trendDir(cur, prev) {
  if (prev == null || cur == null) return null;
  const diff = cur - prev;
  const threshold = Math.max(1, Math.abs(prev) * 0.03);
  if (diff > threshold) return 'up';
  if (diff < -threshold) return 'down';
  return null;
}

function updateStatTiles() {
  if (!liveData) return;
  const tiles = document.querySelectorAll('.tile');
  if (tiles.length < 4) return;

  // Trend baselines come from the 14-day series. This-week = last 7 vs. previous 7,
  // on-task avg = today vs. 14-day mean, points = current session sign (no history yet).
  const t14 = liveData.trend_14d || [];
  const last7 = t14.slice(-7);
  const prev7 = t14.slice(-14, -7);
  const sum = arr => arr.reduce((s, d) => s + (d.total_minutes || 0), 0);
  const avgPctOf = arr => arr.length ? arr.reduce((s, d) => s + (d.on_task_pct || 0), 0) / arr.length : 0;

  // This week
  const totalMin = liveData.weekly_focus_minutes || 0;
  const weekH = Math.floor(totalMin / 60);
  const weekM = totalMin % 60;
  const weekDir = trendDir(sum(last7), sum(prev7));
  const weekArrow = arrowSvg(weekDir);
  if (totalMin < 60) {
    tiles[0].querySelector('.v').innerHTML = `${totalMin}<small>m</small>${weekArrow}`;
    tiles[0].querySelector('.sub').textContent = 'this week';
  } else {
    tiles[0].querySelector('.v').innerHTML = `${weekH}h<small>${weekM}m</small>${weekArrow}`;
    tiles[0].querySelector('.sub').textContent = `${weekH}h ${weekM}m total`;
  }

  // On-task avg
  const avgPct = Math.round(liveData.today.on_task_pct || (liveData.recent_sessions?.[0]?.on_task_pct) || 0);
  const baseline = avgPctOf(t14);
  const otDir = trendDir(avgPct, baseline);
  tiles[1].querySelector('.v').innerHTML = `${avgPct}<small>%</small>${arrowSvg(otDir)}`;
  const sessionCount = liveData.recent_sessions?.length || 0;
  tiles[1].querySelector('.sub').textContent = sessionCount <= 1 ? 'today\'s focus' : 'overall average';

  // Top drift
  if (liveData.distractions && liveData.distractions.length > 0) {
    tiles[2].querySelector('.v').textContent = liveData.distractions[0].name;
    tiles[2].querySelector('.v').style.fontSize = '19px';
    const d0 = liveData.distractions[0];
    const s = d0.seconds ?? d0.minutes * 60;
    const t = s < 60 ? `${s}s` : s < 3600 ? `${Math.floor(s/60)}m` : `${Math.floor(s/3600)}h ${Math.floor((s%3600)/60)}m`;
    tiles[2].querySelector('.sub').textContent = `${t} this week`;
  } else {
    tiles[2].querySelector('.v').textContent = 'None';
    tiles[2].querySelector('.v').style.fontSize = '24px';
    tiles[2].querySelector('.sub').textContent = '0m this week';
  }

  // Points — arrow reflects sign of the latest session's points_earned (up = gained, down = lost).
  // Old code hardcoded a `+` prefix that read as `+-42` for negative points; sign is now
  // derived from the number itself so `-42` renders correctly.
  const lastPts = liveData.recent_sessions?.[0]?.points_earned ?? 0;
  const pointsDir = lastPts > 0 ? 'up' : lastPts < 0 ? 'down' : null;
  tiles[3].querySelector('.v').innerHTML = `${liveData.total_points.toLocaleString()}${arrowSvg(pointsDir)}`;
  const prefix = lastPts > 0 ? '+' : '';
  tiles[3].querySelector('.sub').textContent = `${prefix}${lastPts} last session`;
}

// ─── Calendar ────────────────────────────────────────────────────────

let calViewYear = new Date().getFullYear();
let calViewMonth = new Date().getMonth();

function buildCalendar() {
  const root = document.getElementById('calGrid');
  const now = new Date();
  const year = calViewYear;
  const month = calViewMonth;
  const daysInMonth = new Date(year, month + 1, 0).getDate();
  const firstDow = new Date(year, month, 1).getDay(); // 0=Sun

  const monthNames = ['January','February','March','April','May','June','July','August','September','October','November','December'];
  const titleEl = document.querySelector('.cal-title-row');
  if (titleEl) titleEl.textContent = `${monthNames[month]} ${year}`;

  const dow = ['S','M','T','W','T','F','S'];
  const parts = dow.map(d => `<div class="cal-dow">${d}</div>`);

  // Blank cells before day 1
  for (let i = 0; i < firstDow; i++) parts.push(`<div class="cal-cell blank"></div>`);

  const todayStr = now.toISOString().slice(0, 10);

  for (let d = 1; d <= daysInMonth; d++) {
    const ds = `${year}-${String(month + 1).padStart(2, '0')}-${String(d).padStart(2, '0')}`;
    const isToday = ds === todayStr;
    const isPast = ds < todayStr;
    const isFuture = ds > todayStr;

    // Determine focus level from real data
    let level = 0;
    if (liveData?.trend_14d) {
      const dayEntry = liveData.trend_14d.find(t => t.date === ds);
      if (dayEntry) {
        if (dayEntry.total_minutes >= 90) level = 3;
        else if (dayEntry.total_minutes >= 45) level = 2;
        else if (dayEntry.total_minutes > 0) level = 1;
      }
    }

    const classes = ['cal-cell'];
    if (level > 0) classes.push('f' + level);
    else if (isPast) classes.push('past');
    if (isFuture) classes.push('future');
    if (isToday) classes.push('today');
    const dot = isToday ? '<span class="today-dot"></span>' : '';
    parts.push(`<div class="${classes.join(' ')}" data-date="${ds}">${d}${dot}</div>`);
  }
  root.innerHTML = parts.join('');

  // Overlay layers
  const confirmedLayer = document.createElement('div');
  confirmedLayer.className = 'range-overlay-layer';
  confirmedLayer.id = 'rangeOverlayConfirmed';
  const hoverLayer = document.createElement('div');
  hoverLayer.className = 'range-overlay-layer';
  hoverLayer.id = 'rangeOverlayHover';
  root.appendChild(confirmedLayer);
  root.appendChild(hoverLayer);

  root.querySelectorAll('.cal-cell[data-date]').forEach(cell => {
    const date = cell.dataset.date;
    cell.addEventListener('click', (e) => {
      if (cell.classList.contains('future')) return;
      if (e.shiftKey && anchorDate) {
        let s = anchorDate, en = date;
        if (en < s) [s, en] = [en, s];
        setMode({ mode: 'range', start: s, end: en });
      } else {
        anchorDate = date;
        setMode({ mode: 'single', start: date, end: date });
      }
      clearHoverRange();
    });
    cell.addEventListener('mouseenter', () => {
      if (cell.classList.contains('future')) return;
      lastHoveredDate = date;
      if (shiftDown && anchorDate) showHoverRange(anchorDate, date);
    });
  });

  root.addEventListener('mouseleave', () => {
    lastHoveredDate = null;
    clearHoverRange();
  });
}

let anchorDate = null;
let lastHoveredDate = null;
let shiftDown = false;

const ROW_H = 34, GAP = 5;

function getCalBlanks() {
  const now = new Date();
  return new Date(now.getFullYear(), now.getMonth(), 1).getDay();
}

function dateToRowCol(dateStr) {
  const day = parseInt(dateStr.slice(-2), 10);
  const blanks = getCalBlanks();
  const pos = day - 1 + blanks;
  return { row: Math.floor(pos / 7), col: pos % 7 };
}

function paintRangeOverlay(layer, startDate, endDate, kind) {
  layer.innerHTML = '';
  if (!startDate || !endDate) return;
  let s = startDate, e = endDate;
  if (e < s) [s, e] = [e, s];
  const { row: r0, col: c0 } = dateToRowCol(s);
  const { row: r1, col: c1 } = dateToRowCol(e);
  const gridEl = document.getElementById('calGrid');
  const totalW = gridEl.clientWidth;
  const colW = (totalW - 6 * GAP) / 7;
  const headerOffset = ROW_H + GAP;
  for (let r = r0; r <= r1; r++) {
    const colStart = (r === r0) ? c0 : 0;
    const colEnd = (r === r1) ? c1 : 6;
    const rect = document.createElement('div');
    rect.className = 'range-rect ' + kind;
    rect.style.left = (colStart * (colW + GAP) - 2) + 'px';
    rect.style.width = ((colEnd - colStart + 1) * colW + (colEnd - colStart) * GAP + 4) + 'px';
    rect.style.top = (headerOffset + r * (ROW_H + GAP)) + 'px';
    rect.style.height = ROW_H + 'px';
    layer.appendChild(rect);
  }
}

function showHoverRange(a, b) {
  paintRangeOverlay(document.getElementById('rangeOverlayHover'), a, b, 'hover');
}
function clearHoverRange() {
  const layer = document.getElementById('rangeOverlayHover');
  if (layer) layer.innerHTML = '';
}

document.addEventListener('keydown', (e) => {
  if (e.key !== 'Shift') return;
  shiftDown = true;
  if (anchorDate && lastHoveredDate) showHoverRange(anchorDate, lastHoveredDate);
});
document.addEventListener('keyup', (e) => {
  if (e.key !== 'Shift') return;
  shiftDown = false;
  clearHoverRange();
});

function updateCalendarSelection() {
  document.querySelectorAll('.cal-cell[data-date]').forEach(cell => cell.classList.remove('selected'));
  const confirmedLayer = document.getElementById('rangeOverlayConfirmed');
  if (state.mode === 'overall') { confirmedLayer.innerHTML = ''; return; }
  if (state.mode === 'single') {
    confirmedLayer.innerHTML = '';
    const cell = document.querySelector(`.cal-cell[data-date="${state.start}"]`);
    if (cell) cell.classList.add('selected');
    return;
  }
  if (state.mode === 'range') {
    paintRangeOverlay(confirmedLayer, state.start, state.end, 'confirmed');
    const s = document.querySelector(`.cal-cell[data-date="${state.start}"]`);
    const e = document.querySelector(`.cal-cell[data-date="${state.end}"]`);
    if (s) s.classList.add('selected');
    if (e) e.classList.add('selected');
  }
}

// ─── Trend chart ─────────────────────────────────────────────────────

function fmtMonthDay(iso) {
  const [_, m, d] = iso.split('-');
  const monthNames = ['Jan','Feb','Mar','Apr','May','Jun','Jul','Aug','Sep','Oct','Nov','Dec'];
  return `${monthNames[+m - 1]} ${+d}`;
}
function daysBetween(a, b) { return Math.round((Date.parse(b) - Date.parse(a)) / 86400000); }
function dateRange(a, b) {
  const out = []; const start = Date.parse(a); const n = daysBetween(a, b);
  for (let i = 0; i <= n; i++) {
    const d = new Date(start + i * 86400000);
    out.push(d.toISOString().slice(0,10));
  }
  return out;
}

function smoothPath(points) {
  if (points.length < 3) {
    return points.map((p, i) => (i === 0 ? 'M' : 'L') + p[0].toFixed(2) + ',' + p[1].toFixed(2)).join(' ');
  }
  let d = `M${points[0][0].toFixed(2)},${points[0][1].toFixed(2)}`;
  for (let i = 0; i < points.length - 1; i++) {
    const p0 = points[i - 1] || points[i];
    const p1 = points[i];
    const p2 = points[i + 1];
    const p3 = points[i + 2] || p2;
    const c1x = p1[0] + (p2[0] - p0[0]) / 6;
    const c1y = p1[1] + (p2[1] - p0[1]) / 6;
    const c2x = p2[0] - (p3[0] - p1[0]) / 6;
    const c2y = p2[1] - (p3[1] - p1[1]) / 6;
    d += ` C${c1x.toFixed(2)},${c1y.toFixed(2)} ${c2x.toFixed(2)},${c2y.toFixed(2)} ${p2[0].toFixed(2)},${p2[1].toFixed(2)}`;
  }
  return d;
}

const ENERGY_BOLT = '<path d="M13 2 3 14h6l-1 8 10-12h-6l1-8Z" fill="var(--peak-line)"/>';

function renderLineChart(viz, labels, values, xlabels) {
  // Chart SVG stretches horizontally (preserveAspectRatio="none") so the curve fills the card,
  // but that same stretch was mangling the y-axis tick text — a font drawn once at 10px inside
  // the stretched SVG got squashed into a wide, weird slab at display time. Fix: keep the SVG
  // for lines + area only, and render tick labels as HTML positioned absolutely, so they use
  // the page's real font at a real aspect ratio.
  const W = 600, H = 150, PAD_L = 6, PAD_R = 4, PAD_T = 14, PAD_B = 14;
  // Left inset (in the OUTER container, not SVG units) where labels sit + grid starts.
  const HTML_LEFT_INSET = 34;
  const n = values.length;
  if (n === 0) { viz.innerHTML = '<div class="empty-state">No data yet.</div>'; labels.innerHTML = ''; return; }

  // Domain: always include 0 so negative streaks show against a clear baseline. When all
  // values are 0 the y-mapping would divide by 0 — collapse to a symmetric ±1 span.
  const rawMax = Math.max(...values);
  const rawMin = Math.min(...values);
  let dMax = Math.max(rawMax, 0);
  let dMin = Math.min(rawMin, 0);
  if (dMax === dMin) { dMax = 1; dMin = -1; }
  const padSpan = (dMax - dMin) * 0.08;
  dMax += padSpan; dMin -= padSpan;

  const step = (W - PAD_L - PAD_R) / (n - 1 || 1);
  const y = v => PAD_T + (1 - (v - dMin) / (dMax - dMin)) * (H - PAD_T - PAD_B);
  const pts = values.map((v, i) => [PAD_L + i * step, y(v)]);
  const curve = smoothPath(pts);
  const baselineY = y(0);
  const areaPath = curve
    + ` L${pts[pts.length-1][0].toFixed(1)},${baselineY.toFixed(1)}`
    + ` L${pts[0][0].toFixed(1)},${baselineY.toFixed(1)} Z`;
  const last = pts[pts.length - 1];
  const dotXPct = (last[0] / W) * 100;
  const dotYPct = (last[1] / H) * 100;

  // Tick positions computed in SVG-y, converted to percentage for HTML placement.
  const tickVals = [dMax - padSpan, (dMax + dMin) / 2, dMin + padSpan];
  const ticks = tickVals.map(v => ({ v, posPct: (y(v) / H) * 100 }));

  const gridSvg = ticks.map(t => `
    <line x1="${PAD_L}" y1="${y(t.v).toFixed(1)}" x2="${W}" y2="${y(t.v).toFixed(1)}" stroke="var(--line)" stroke-dasharray="1 5"/>
  `).join('');
  const zeroLineSvg = (rawMin < 0 && rawMax > 0)
    ? `<line x1="${PAD_L}" y1="${baselineY.toFixed(1)}" x2="${W}" y2="${baselineY.toFixed(1)}" stroke="var(--ink-muted)" stroke-opacity="0.35" stroke-width="1"/>`
    : '';
  const tickHtml = ticks.map(t => `
    <span class="y-tick-label" style="top:${t.posPct.toFixed(2)}%;">${Math.round(t.v)}</span>
  `).join('');

  viz.innerHTML = `
    <div class="chart-inner" style="position:absolute; inset:0 0 0 ${HTML_LEFT_INSET}px;">
      <svg class="chart-svg" viewBox="0 0 ${W} ${H}" preserveAspectRatio="none" aria-hidden="true">
        <defs>
          <linearGradient id="trendFill" x1="0" y1="0" x2="0" y2="1">
            <stop offset="0%" stop-color="var(--moss)" stop-opacity="0.32"/>
            <stop offset="100%" stop-color="var(--moss)" stop-opacity="0"/>
          </linearGradient>
        </defs>
        ${gridSvg}
        ${zeroLineSvg}
        <path d="${areaPath}" fill="url(#trendFill)"/>
        <path d="${curve}" fill="none" stroke="var(--moss-deep)" stroke-width="1.8" stroke-linejoin="round" stroke-linecap="round" vector-effect="non-scaling-stroke"/>
      </svg>
      <div class="session-dot" style="left:${dotXPct.toFixed(2)}%; top:${dotYPct.toFixed(2)}%; width:9px; height:9px; box-shadow: 0 0 0 5px color-mix(in oklab, var(--moss) 25%, transparent);"></div>
    </div>
    <div class="y-tick-labels" style="width:${HTML_LEFT_INSET}px;">${tickHtml}</div>
  `;
  const idxs = [0, Math.floor(n/3), Math.floor(2*n/3), n-1].filter((v,i,a) => a.indexOf(v) === i);
  labels.innerHTML = idxs.map(i => `<span>${xlabels[i]}</span>`).join('');
}

// Which stat the chart shows. User picks it in the trend card's dropdown; persists across
// mode changes (single/range/overall) within the session so the picker feels sticky.
let trendStat = 'focus_min';

// One display name per stat — the trend card's title uses `label` too so it never disagrees
// with the picker's selected option. (Old code had a distinct `overallTitle: 'Attention span'`
// for focus_min, which read as a broken cross-reference against the picker's "Focus time".)
const TREND_STAT_META = {
  focus_min:   { label: 'Focus time',    get: d => d.total_minutes,          unit: 'min' },
  on_task_pct: { label: 'On-task %',     get: d => Math.round(d.on_task_pct), unit: '%' },
  sessions:    { label: 'Sessions/day',  get: d => d.session_count,          unit: '' },
  points:      { label: 'Points/day',    get: d => d.points || 0,            unit: '' },
};

function renderTrend() {
  const viz = document.getElementById('trendViz');
  const labels = document.getElementById('trendLabels');
  const title = document.getElementById('trendTitle');
  const num = document.getElementById('trendNum');
  const delta = document.getElementById('trendDelta');
  const sub = document.getElementById('trendSub');
  const meta = TREND_STAT_META[trendStat] || TREND_STAT_META.focus_min;

  // Overall base data: last 14 days. Range mode charts the range's slice; single mode
  // shows a 7-day window centered on the picked date so the chart still makes sense.
  const all = liveData?.trend_14d || [];
  if (all.length === 0) {
    title.textContent = meta.label;
    sub.textContent = 'no data yet — start a session';
    num.innerHTML = `0<span class="pct"> ${meta.unit}</span>`;
    delta.innerHTML = '';
    viz.innerHTML = '<div class="empty-state">Complete sessions to see your trend.</div>';
    labels.innerHTML = '';
    return;
  }

  // Slice the 14-day series to what the current mode wants.
  let series = all;
  if (state.mode === 'single' && state.start) {
    // 7 days ending on the picked day (or all we have, whichever is shorter).
    const idx = all.findIndex(t => t.date === state.start);
    if (idx >= 0) {
      const from = Math.max(0, idx - 6);
      series = all.slice(from, idx + 1);
    } else {
      // Picked date isn't in the 14-day window (e.g. before the app existed) — show empty
      // instead of silently falling back to the last 14 days, which read as "the chart lies."
      title.innerHTML = `<span style="font-family:var(--font-serif);font-style:italic;">${fmtMonthDay(state.start)}</span> · ${meta.label.toLowerCase()}`;
      sub.textContent = 'no data recorded on that date';
      num.innerHTML = `0<span class="pct"> ${meta.unit}</span>`;
      delta.innerHTML = '';
      viz.innerHTML = '<div class="empty-state">No data for that date.</div>';
      labels.innerHTML = '';
      return;
    }
  } else if (state.mode === 'range' && state.start && state.end) {
    series = all.filter(t => t.date >= state.start && t.date <= state.end);
    if (series.length === 0) {
      title.innerHTML = `${fmtMonthDay(state.start)} — ${fmtMonthDay(state.end)}`;
      sub.textContent = 'no data recorded in that range';
      num.innerHTML = `0<span class="pct"> ${meta.unit}</span>`;
      delta.innerHTML = '';
      viz.innerHTML = '<div class="empty-state">No data in that range.</div>';
      labels.innerHTML = '';
      return;
    }
  }

  // Headline title reflects mode; sub-copy reflects the picked stat.
  if (state.mode === 'overall') {
    title.textContent = meta.label;
    sub.textContent = `last ${series.length} days · ${meta.label.toLowerCase()}`;
  } else if (state.mode === 'single') {
    title.innerHTML = `<span style="font-family:var(--font-serif);font-style:italic;">${fmtMonthDay(state.start)}</span> · ${meta.label.toLowerCase()}`;
    sub.textContent = `7-day window · ${meta.label.toLowerCase()}`;
  } else {
    title.innerHTML = `${fmtMonthDay(state.start)} — ${fmtMonthDay(state.end)}`;
    sub.textContent = `${series.length} days · ${meta.label.toLowerCase()}`;
  }

  // Headline number: average of the selected stat over the current series. For percentages
  // this is a straight arithmetic mean; for focus minutes it's the daily average.
  const values = series.map(meta.get);
  const avg = values.length ? Math.round(values.reduce((s, v) => s + v, 0) / values.length) : 0;
  num.innerHTML = `${avg}<span class="pct"> ${meta.unit}</span>`;
  delta.innerHTML = `<svg width="10" height="10" viewBox="0 0 12 12" fill="none" stroke="currentColor" stroke-width="2"><path d="M2 8l4-4 4 4"/></svg> avg`;

  // Chart is always rendered now, regardless of mode — that's the main fix. Range/single
  // used to blank out `viz` entirely, which read as "the graph broke."
  renderLineChart(viz, labels, values, series.map(t => fmtMonthDay(t.date)));
}

// ─── Sessions list ───────────────────────────────────────────────────

window.editSessionName = async function(id, currentGoal) {
  const newName = prompt('Rename session goal:', currentGoal);
  if (!newName || !newName.trim() || newName.trim() === currentGoal) return;
  try {
    await invoke('rename_session', { id, goal: newName.trim() });
    liveData = await invoke('get_dashboard_data');
    renderSessions();
  } catch (e) {
    console.error('Failed to rename session:', e);
  }
};

function renderSessions() {
  const list = document.getElementById('sessionList');
  const title = document.getElementById('sessionsTitle');
  const meta = document.getElementById('sessionsMeta');

  if (state.mode === 'overall') {
    title.textContent = 'Recent sessions';
    const sessions = liveData?.recent_sessions ?? [];
    meta.textContent = sessions.length > 0 ? `last ${sessions.length}` : 'none yet';
    if (sessions.length === 0) {
      list.innerHTML = '<div class="empty-state">No sessions yet. Start your first one!</div>';
      return;
    }
    list.innerHTML = sessions.map(s => {
      const pct = Math.round(s.on_task_pct);
      const dur = s.duration_min ? `${s.duration_min}m` : '—';
      const when = formatRelativeDate(s.started_at);
      const safeGoal = (s.goal || '').replace(/'/g, "\\'").replace(/"/g, '&quot;');
      return `<div class="session-row">
        <div class="goal">
          <span class="goal-text">${s.goal}</span>
          <button class="session-edit-btn" title="Rename session" onclick="editSessionName('${s.id}', '${safeGoal}')">
            <svg viewBox="0 0 24 24" width="13" height="13" fill="none" stroke="currentColor" stroke-width="2"><path d="M11 4H4a2 2 0 0 0-2 2v14a2 2 0 0 0 2 2h14a2 2 0 0 0 2-2v-7"/><path d="M18.5 2.5a2.121 2.121 0 0 1 3 3L12 15l-4 1 1-4 9.5-9.5z"/></svg>
          </button>
        </div>
        <div class="when">${when}</div>
        <div class="dur">${dur}</div>
        <div class="pct ${pct < 75 ? 'low' : ''}">${pct}%</div>
      </div>`;
    }).join('');
  } else if (state.mode === 'single' || state.mode === 'range') {
    const start = state.start, end = state.end || state.start;
    title.innerHTML = `Sessions <span style="font-family:var(--font-serif);font-style:italic;">${fmtMonthDay(start)}${state.mode === 'range' ? ' — ' + fmtMonthDay(end) : ''}</span>`;
    invoke('get_sessions_in_range', { start, end }).then(sessions => {
      meta.textContent = `${sessions.length} session${sessions.length === 1 ? '' : 's'}`;
      if (sessions.length === 0) {
        list.innerHTML = '<div class="empty-state">Nothing here yet.</div>';
        return;
      }
      list.innerHTML = sessions.map(s => {
        const pct = Math.round(s.on_task_pct);
        const dur = s.duration_min ? `${s.duration_min}m` : '—';
        const safeGoal = (s.goal || '').replace(/'/g, "\\'").replace(/"/g, '&quot;');
        return `<div class="session-row">
          <div class="goal">
            <span class="goal-text">${s.goal}</span>
            <button class="session-edit-btn" title="Rename session" onclick="editSessionName('${s.id}', '${safeGoal}')">
              <svg viewBox="0 0 24 24" width="13" height="13" fill="none" stroke="currentColor" stroke-width="2"><path d="M11 4H4a2 2 0 0 0-2 2v14a2 2 0 0 0 2 2h14a2 2 0 0 0 2-2v-7"/><path d="M18.5 2.5a2.121 2.121 0 0 1 3 3L12 15l-4 1 1-4 9.5-9.5z"/></svg>
            </button>
          </div>
          <div class="when">${formatRelativeDate(s.started_at)}</div>
          <div class="dur">${dur}</div>
          <div class="pct ${pct < 75 ? 'low' : ''}">${pct}%</div>
        </div>`;
      }).join('');
    }).catch(() => {
      list.innerHTML = '<div class="empty-state">Could not load.</div>';
    });
  }
}

function formatRelativeDate(isoStr) {
  if (!isoStr) return '';
  const d = new Date(isoStr);
  const today = new Date();
  const todayStr = today.toISOString().slice(0, 10);
  const dateStr = d.toISOString().slice(0, 10);
  const time = d.toLocaleTimeString('en-US', { hour: 'numeric', minute: '2-digit' });
  if (dateStr === todayStr) return `today, ${time}`;
  const yesterday = new Date(today - 86400000).toISOString().slice(0, 10);
  if (dateStr === yesterday) return `yesterday, ${time}`;
  return `${fmtMonthDay(dateStr)}, ${time}`;
}

function formatDateNice(iso) {
  const d = new Date(iso + 'T12:00:00');
  const days = ['Sunday','Monday','Tuesday','Wednesday','Thursday','Friday','Saturday'];
  const months = ['Jan','Feb','Mar','Apr','May','Jun','Jul','Aug','Sep','Oct','Nov','Dec'];
  return `${days[d.getDay()]}, ${months[d.getMonth()]} ${d.getDate()}`;
}

// ─── Distractions bar chart ──────────────────────────────────────────

function renderDistractions() {
  const barList = document.querySelector('.bar-list');
  if (!barList) return;
  // Filter out entries with zero recorded time — the old integer-minutes column collapsed
  // anything under 60s to "0m" which read as broken. Sub-minute drifts now show in seconds.
  const distractions = (liveData?.distractions ?? []).filter(d => (d.seconds ?? d.minutes * 60) > 0);
  if (distractions.length === 0) {
    barList.innerHTML = '<div class="empty-state">No distractions tracked yet.</div>';
    return;
  }
  const secsOf = d => d.seconds ?? d.minutes * 60;
  const maxS = Math.max(...distractions.map(secsOf), 1);
  barList.innerHTML = distractions.slice(0, 5).map(d => {
    const s = secsOf(d);
    const pct = Math.round((s / maxS) * 100);
    const timeStr = s < 60 ? `${s}s`
                  : s < 3600 ? `${Math.floor(s/60)}m`
                  : `${Math.floor(s/3600)}h ${Math.floor((s%3600)/60)}m`;
    return `<div class="bar-row"><span class="n">${d.name}</span><div class="bar"><div class="fill" style="width:${pct}%"></div></div><span class="t">${timeStr}</span></div>`;
  }).join('');
}

// ─── Mode switching ──────────────────────────────────────────────────

function setMode(next) {
  Object.assign(state, next);
  const isOverall = state.mode === 'overall';
  document.querySelectorAll('.overall-btn').forEach(b => b.classList.toggle('active', isOverall));
  updateCalendarSelection();
  renderTrend();
  renderSessions();
  renderDistractions();
  renderDashActivity();
}

document.getElementById('overallBtnGlobal').addEventListener('click', () => setMode({ mode: 'overall', start: null, end: null }));
document.getElementById('overallBtnTrend').addEventListener('click', () => setMode({ mode: 'overall', start: null, end: null }));

// Custom dropdown for the trend stat picker — button + list panel toggled by JS. Native
// <select> popups on Windows can't be styled to match the app palette, so we bypass them.
const trendStatPicker = document.getElementById('trendStatPicker');
const trendStatLabel = document.getElementById('trendStatLabel');
const trendStatMenu = document.getElementById('trendStatMenu');
if (trendStatPicker && trendStatMenu && trendStatLabel) {
  const closePicker = () => {
    trendStatPicker.classList.remove('open');
    trendStatPicker.setAttribute('aria-expanded', 'false');
  };
  const openPicker = () => {
    trendStatPicker.classList.add('open');
    trendStatPicker.setAttribute('aria-expanded', 'true');
  };
  trendStatPicker.addEventListener('click', (e) => {
    // Ignore bubbles from item clicks — the item handler will close after picking.
    if (e.target.closest('.stat-picker-menu li')) return;
    if (trendStatPicker.classList.contains('open')) closePicker(); else openPicker();
  });
  trendStatPicker.addEventListener('keydown', (e) => {
    if (e.key === 'Enter' || e.key === ' ') { e.preventDefault(); openPicker(); }
    if (e.key === 'Escape') closePicker();
  });
  trendStatMenu.querySelectorAll('li').forEach(li => {
    li.addEventListener('click', () => {
      trendStat = li.dataset.value;
      trendStatMenu.querySelectorAll('li').forEach(x => x.classList.toggle('active', x === li));
      trendStatLabel.textContent = li.textContent;
      closePicker();
      renderTrend();
    });
  });
  // Click-outside dismiss.
  document.addEventListener('click', (e) => {
    if (!trendStatPicker.contains(e.target)) closePicker();
  });
}

// ─── Page navigation ─────────────────────────────────────────────────

function navigateTo(page) {
  document.querySelectorAll('.nav-item[data-page]').forEach(i => i.classList.remove('active'));
  const navItem = document.querySelector(`.nav-item[data-page="${page}"]`);
  if (navItem) navItem.classList.add('active');
  ['dashboard','session','bounties','settings'].forEach(p => {
    const el = document.getElementById('page-' + p);
    if (!el) return;
    el.style.display = p === page ? '' : 'none';
    if (p === page) { el.classList.remove('page-enter'); void el.offsetWidth; el.classList.add('page-enter'); }
  });
  if (page === 'session') renderSessionTimeline();
  if (page === 'bounties') refreshBounties();
}

document.querySelectorAll('.nav-item[data-page]').forEach(item => {
  item.addEventListener('click', () => navigateTo(item.dataset.page));
});

// ─── Bounties ────────────────────────────────────────────────────────

let bountyCountdownInterval = null;
let bountyRefreshTargetTs = null;

async function refreshBounties() {
  const grid = document.getElementById('bountyGrid');
  if (!grid) return;
  try {
    const payload = await invoke('get_bounties');
    // Backend gives us seconds-until-midnight; convert to a wall-clock target so the
    // JS timer keeps counting down accurately without another IPC round-trip per second.
    bountyRefreshTargetTs = Date.now() + payload.seconds_until_refresh * 1000;
    startBountyCountdown();
    renderBountyGrid(payload.bounties || []);
  } catch (e) {
    console.warn('get_bounties failed:', e);
    grid.innerHTML = '<div class="bounty-empty">Could not load bounties. Try again shortly.</div>';
  }
}

function renderBountyGrid(bounties) {
  const grid = document.getElementById('bountyGrid');
  if (!grid) return;
  if (!bounties.length) {
    grid.innerHTML = '<div class="bounty-empty">No bounties yet — check back after midnight.</div>';
    return;
  }
  grid.innerHTML = bounties.map(cardHtml).join('');
  // Wire buttons after render (event delegation would work too; direct is simpler with 3 cards).
  grid.querySelectorAll('[data-action="accept"]').forEach(b => {
    b.addEventListener('click', () => acceptBounty(b.dataset.id));
  });
  grid.querySelectorAll('[data-action="claim"]').forEach(b => {
    b.addEventListener('click', () => claimBounty(b.dataset.id));
  });
}

function cardHtml(b) {
  const kindLabel = b.kind === 'reinforcing' ? 'Reinforcing' : 'Exploratory';
  const diffLabel = b.difficulty.charAt(0).toUpperCase() + b.difficulty.slice(1);
  const pct = Math.max(0, Math.min(100, (b.progress || 0) * 100));
  const showProgress = b.status === 'accepted' || b.status === 'completed' || b.status === 'claimed';
  let btn;
  if (b.status === 'available') {
    btn = `<button class="bounty-btn accept" data-action="accept" data-id="${b.id}">Accept bounty</button>`;
  } else if (b.status === 'accepted') {
    btn = `<button class="bounty-btn in-progress" disabled>In progress…</button>`;
  } else if (b.status === 'completed') {
    btn = `<button class="bounty-btn claim" data-action="claim" data-id="${b.id}">Claim +${b.reward}</button>`;
  } else {
    btn = `<button class="bounty-btn claimed" disabled>Claimed ✓</button>`;
  }
  return `
    <div class="bounty-card ${b.status}">
      <div class="bounty-badges">
        <span class="bounty-badge kind-${b.kind}">${kindLabel}</span>
        <span class="bounty-badge diff-${b.difficulty}">${diffLabel}</span>
      </div>
      <div class="bounty-title">${escapeHtml(b.title)}</div>
      <div class="bounty-desc">${escapeHtml(b.description)}</div>
      <div class="bounty-reward">
        <span class="num">+${b.reward}</span>
        <span class="lbl">pts</span>
      </div>
      ${showProgress ? `
        <div class="bounty-progress">
          <div class="bounty-progress-track"><div class="bounty-progress-fill" style="width:${pct}%"></div></div>
          <div class="bounty-progress-label">${escapeHtml(b.progress_label || '')}</div>
        </div>` : ''}
      ${btn}
    </div>
  `;
}

async function acceptBounty(id) {
  try {
    await invoke('accept_bounty', { id });
    await refreshBounties();
  } catch (e) {
    console.warn('accept_bounty failed:', e);
  }
}

async function claimBounty(id) {
  try {
    const reward = await invoke('claim_bounty', { id });
    // Refresh dashboard totals silently so the points count elsewhere is current.
    try { liveData = await invoke('get_dashboard_data'); } catch(e) {}
    await refreshBounties();
    console.log(`Bounty claimed: +${reward} pts`);
  } catch (e) {
    console.warn('claim_bounty failed:', e);
  }
}

function startBountyCountdown() {
  if (bountyCountdownInterval) return;
  const tick = () => {
    const el = document.getElementById('bountyCountdown');
    if (!el || bountyRefreshTargetTs == null) return;
    let secs = Math.max(0, Math.round((bountyRefreshTargetTs - Date.now()) / 1000));
    if (secs === 0) {
      // Midnight hit — pull the fresh pool. Backend will regenerate today's row.
      refreshBounties();
      return;
    }
    const h = Math.floor(secs / 3600);
    const m = Math.floor((secs % 3600) / 60);
    const s = secs % 60;
    el.textContent = `${String(h).padStart(2,'0')}:${String(m).padStart(2,'0')}:${String(s).padStart(2,'0')}`;
  };
  tick();
  bountyCountdownInterval = setInterval(tick, 1000);
}

function escapeHtml(s) {
  return String(s).replace(/[&<>"']/g, c => ({
    '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;'
  })[c]);
}

// ─── Session timeline ────────────────────────────────────────────────

function renderSessionTimeline() {
  const viz = document.getElementById('sessionTimelineViz');
  if (!viz) return;

  if (!sessionActive) {
    viz.innerHTML = '<div class="empty-state">Start a session to see the timeline.</div>';
    viz.removeAttribute('data-rendered');
    return;
  }

  // Fetch real intervals for the active session
  const state_lock = document.getElementById('sMetOnTask');
  // We'll render from the session activity list since intervals are live
  // For now, show a simple on-task/off-task bar chart from intervals
  const sessionId = currentSessionId;
  if (!sessionId) {
    viz.innerHTML = '<div class="empty-state">Waiting for first tick…</div>';
    return;
  }

  invoke('get_session_intervals', { sessionId }).then(intervals => {
    if (!intervals || intervals.length === 0) {
      viz.innerHTML = '<div class="empty-state">Collecting data…</div>';
      return;
    }

    // Build a focus timeline bar — segments colored by status
    const W = 600, H = 40;
    const totalDur = intervals.reduce((sum, iv) => {
      const s = new Date(iv.start_ts).getTime();
      const e = iv.end_ts ? new Date(iv.end_ts).getTime() : Date.now();
      return sum + (e - s);
    }, 0) || 1;

    let rects = '';
    let x = 0;
    intervals.forEach(iv => {
      const s = new Date(iv.start_ts).getTime();
      const e = iv.end_ts ? new Date(iv.end_ts).getTime() : Date.now();
      const dur = e - s;
      const w = (dur / totalDur) * W;
      const color = iv.status === 'on_task' ? 'var(--moss)' : 'var(--terracotta)';
      rects += `<rect x="${x}" y="4" width="${Math.max(w, 1)}" height="32" rx="3" fill="${color}" opacity="0.7"/>`;
      x += w;
    });

    viz.innerHTML = `
      <svg viewBox="0 0 ${W} ${H}" preserveAspectRatio="none" aria-hidden="true">
        ${rects}
      </svg>
    `;
  }).catch(() => {
    viz.innerHTML = '<div class="empty-state">Could not load timeline.</div>';
  });
}

let currentSessionId = null;

// ─── Session start/stop ──────────────────────────────────────────────

const startModal = document.getElementById('startModalBackdrop');
const summaryModal = document.getElementById('summaryModalBackdrop');
const startSessionBtn = document.getElementById('startSessionBtn');
const demoDriftBtn = document.getElementById('demoDriftBtn');

function setSessionUI(active) {
  sessionActive = active;
  const leafCard = document.getElementById('leafLetoutCard');
  if (active) {
    startSessionBtn.innerHTML = `<svg viewBox="0 0 24 24" fill="currentColor"><rect x="6" y="6" width="12" height="12" rx="2"/></svg> End session`;
    startSessionBtn.classList.add('danger');
    demoDriftBtn.style.display = 'inline-flex';
    document.getElementById('sessionNowBar').style.display = '';
    document.getElementById('sessionActivityCard').style.display = '';
    if (leafCard) leafCard.style.display = 'flex';
    const advP = document.getElementById('advPanel');
    if (advP) advP.style.display = '';
  } else {
    startSessionBtn.innerHTML = `<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8"><path d="M13 2 3 14h6l-1 8 10-12h-6l1-8Z"/></svg> Start session`;
    startSessionBtn.classList.remove('danger');
    demoDriftBtn.style.display = 'none';
    document.getElementById('sessionNowBar').style.display = 'none';
    document.getElementById('sessionActivityCard').style.display = 'none';
    if (leafCard) leafCard.style.display = 'none';
    const advP2 = document.getElementById('advPanel');
    if (advP2) { advP2.style.display = 'none'; advP2.classList.remove('open'); advPanelOpen = false; }
    setSessionOffState();
  }
}

startSessionBtn.addEventListener('click', async () => {
  if (sessionActive) {
    // End session
    try {
      const summary = await invoke('end_session');
      currentSessionId = null;
      showSummaryAnimated(summary);
    } catch (e) {
      console.error('Failed to end session:', e);
      currentSessionId = null;
      setSessionUI(false);
    }
  } else {
    try {
      const topTools = await invoke('get_top_tools');
      if (topTools && topTools.length > 0) {
        startModalToolField.updateSuggestions(topTools);
      }
    } catch(e) {}
    startModal.style.display = 'flex';
  }
});

startModal.addEventListener('click', (e) => {
  if (e.target === startModal) startModal.style.display = 'none';
});
summaryModal.addEventListener('click', (e) => {
  if (e.target === summaryModal) {
    summaryModal.style.display = 'none';
    setSessionUI(false);
    resetSummaryAnimation();
  }
});
document.getElementById('summaryDoneBtn').addEventListener('click', async () => {
  summaryModal.style.display = 'none';
  setSessionUI(false);
  resetSummaryAnimation();
  // Refresh dashboard data with await so trends and tiles render latest data
  try { liveData = await invoke('get_dashboard_data'); } catch(e) {}
  // Bounty progress recomputes server-side on end_session, but if the user's already on
  // the bounties page (they aren't here, but for future navigation) it's cheap to prime.
  try { await invoke('get_bounties'); } catch(e) {}
  navigateTo('dashboard');
  setMode({ mode: 'overall', start: null, end: null });
  updateGreeting();
  renderInsights();
});

document.addEventListener('keydown', (e) => {
  if (e.key !== 'Escape') return;
  if (startModal.style.display !== 'none') startModal.style.display = 'none';
  if (summaryModal.style.display !== 'none') {
    summaryModal.style.display = 'none';
    setSessionUI(false);
    resetSummaryAnimation();
  }
});

// Description character counter listener
const startDescriptionInput = document.getElementById('startDescriptionInput');
const descCharCounter = document.getElementById('descCharCount');
const descInputBox = document.getElementById('descInputBox');

if (startDescriptionInput && descCharCounter) {
  startDescriptionInput.addEventListener('input', () => {
    const len = startDescriptionInput.value.trim().length;
    descCharCounter.textContent = `${len} / 25 min chars`;
    if (len >= 25) {
      descCharCounter.classList.add('valid');
      descCharCounter.classList.remove('invalid');
      if (descInputBox) descInputBox.classList.remove('error');
    } else {
      descCharCounter.classList.remove('valid');
    }
  });
}

document.getElementById('modalStartBtn').addEventListener('click', async () => {
  const goalInput = startModal.querySelector('input[type="text"]');
  const goal = goalInput?.value?.trim();
  const description = startDescriptionInput ? startDescriptionInput.value.trim() : '';

  if (!goal) {
    goalInput.classList.remove('shake-input');
    void goalInput.offsetWidth;
    goalInput.classList.add('shake-input');
    goalInput.focus();
    goalInput.placeholder = 'Type a goal first…';
    return;
  }

  // Validate description length >= 25
  if (description.length < 25) {
    if (descInputBox) {
      descInputBox.classList.remove('error');
      void descInputBox.offsetWidth;
      descInputBox.classList.add('error');
    }
    if (descCharCounter) descCharCounter.classList.add('invalid');
    if (startDescriptionInput) startDescriptionInput.focus();
    return;
  }

  const modalStartBtn = document.getElementById('modalStartBtn');
  modalStartBtn.disabled = true;
  modalStartBtn.innerHTML = `<span>Validating with AI…</span>`;

  // Validate task goal and description using Gemma AI
  try {
    const reason = await invoke('validate_goal', { goal, description });
    if (reason) {
      if (descInputBox) {
        descInputBox.classList.remove('error');
        void descInputBox.offsetWidth;
        descInputBox.classList.add('error');
      }
      if (descCharCounter) descCharCounter.classList.add('invalid');
      const denialEl = document.getElementById('descDenialReason');
      if (denialEl) {
        denialEl.textContent = reason;
        denialEl.style.display = '';
      }
      modalStartBtn.disabled = false;
      modalStartBtn.innerHTML = `<svg viewBox="0 0 24 24" fill="currentColor"><path d="M13 2 3 14h6l-1 8 10-12h-6l1-8Z"/></svg> Start`;
      return;
    }
  } catch (e) {
    console.warn('Goal validation check failed:', e);
  }
  // Clear any previous denial reason
  const denialEl = document.getElementById('descDenialReason');
  if (denialEl) denialEl.style.display = 'none';

  startModal.style.display = 'none';
  modalStartBtn.disabled = false;
  modalStartBtn.innerHTML = `<svg viewBox="0 0 24 24" fill="currentColor"><path d="M13 2 3 14h6l-1 8 10-12h-6l1-8Z"/></svg> Start`;

  // Get selected duration
  let durationMin = null;
  const selectedPill = document.querySelector('.time-pill.selected');
  if (selectedPill) {
    const v = selectedPill.dataset.min;
    if (v !== 'endless') durationMin = parseInt(v, 10);
  }
  const customInput = document.getElementById('timeCustomInput');
  const customBox = document.getElementById('timeCustomBox');
  if (customBox.classList.contains('selected') && customInput.value) {
    durationMin = parseInt(customInput.value, 10);
  }

  try {
    const sessionState = await invoke('start_session', {
      goal,
      description,
      profileId: 'default',
      durationMin,
    });
    sessionActive = true;
    currentSessionId = sessionState.session_id;
    setSessionUI(true);
    navigateTo('session');
    // Blank last session's activity list so it doesn't appear as this session's data for the
    // first ~5s until the tick loop paints real intervals.
    _lastActivityKey = '';
    _lastTimelineKey = '';
    const sActList = document.getElementById('sessionActivityList');
    if (sActList) sActList.innerHTML = '<div class="empty-state">Waiting for first tick…</div>';
    const sViz = document.getElementById('sessionTimelineViz');
    if (sViz) sViz.innerHTML = '';
    applySessionState(sessionState);

    // Update session page goal text
    const goalEl = document.querySelector('.session-goal');
    if (goalEl) goalEl.innerHTML = pickSessionGreeting(goal, description);
    const metaEl = document.querySelector('.session-goal-meta');
    if (metaEl) {
      const now = new Date();
      const time = now.toLocaleTimeString('en-US', { hour: 'numeric', minute: '2-digit' });
      const durText = durationMin ? `${durationMin}-minute timer` : 'Endless mode';
      metaEl.innerHTML = `<span>Started ${time}</span><span class="sep">·</span><span>${durText}</span>`;
    }

    // Start local timer for smooth UI updates between ticks
    startLocalTimer(durationMin);
  } catch (e) {
    console.error('Failed to start session:', e);
  }
});

// ─── Cyberpunk drift overlay ─────────────────────────────────────────

const cyberOverlay = document.getElementById('cyberOverlay');
const cyberContent = document.getElementById('cyberContent');
const cyberHero = document.getElementById('cyberHero');
const cyberSub = document.getElementById('cyberSub');

// Cynical/mean roast lines. Structured as (matcher, lines) pairs — the first matcher
// that fits the current app or window title wins, otherwise we fall back to GENERIC.
// The matcher checks both app-name and detail so browser tabs ("YouTube - foo") route
// to the YouTube pool even when the process is just "chrome" or "firefox".
const APP_ROASTS = [
  {
    match: /discord|slack|whatsapp|telegram|imessage/i,
    lines: [
      (a, d) => `NOBODY IN ${(a || 'CHAT').toUpperCase()} IS DOING YOUR HOMEWORK FOR YOU.`,
      () => `THE GROUP CHAT WILL STILL BE THERE. YOUR GRADE MIGHT NOT.`,
      (a) => `STILL SCROLLING ${(a || 'CHAT').toUpperCase()}. STILL BEHIND.`,
      () => `NO ONE'S SAYING ANYTHING IMPORTANT. CLOSE IT.`,
    ],
  },
  {
    match: /youtube|netflix|twitch|hulu|primevideo|disney/i,
    lines: [
      (a) => `${(a || 'VIDEOS').toUpperCase()} ≠ STUDYING. NICE TRY.`,
      (a, d) => `"${(d || 'this video').slice(0, 32)}" CAN WAIT UNTIL YOU'RE DONE.`,
      () => `AUTOPLAY IS EATING YOUR EVENING.`,
      () => `ONE MORE VIDEO. YEAH RIGHT.`,
    ],
  },
  {
    match: /reddit|twitter|x\.com|instagram|tiktok|facebook|snapchat/i,
    lines: [
      (a) => `${(a || 'SOCIAL').toUpperCase()} IS NOT ON THE STUDY GUIDE.`,
      () => `THE ALGORITHM KNOWS YOU DON'T WANT TO WORK. IT'S WINNING.`,
      (a) => `HOW'S THAT ${(a || 'FEED').toUpperCase()} DOOM SCROLL TREATING YOUR GPA?`,
      () => `INFINITE SCROLL. FINITE TIME.`,
    ],
  },
  {
    match: /steam|epicgames|riot|leagueoflegends|valorant|minecraft|fortnite|roblox|battle\.net/i,
    lines: [
      (a) => `${(a || 'THIS GAME').toUpperCase()} IS FUN. FAILING ISN'T.`,
      () => `RANK UP AT WHATEVER. RANK DOWN IN CLASS.`,
      () => `PIXELS AREN'T POINTS. GAME LATER.`,
      () => `IMAGINE EXPLAINING THIS TO YOUR PARENTS.`,
    ],
  },
  {
    match: /gmail|outlook|mail\.google|yahoo mail/i,
    lines: [
      () => `EMAIL IS NOT WORK. IT'S THE ILLUSION OF WORK.`,
      () => `INBOX ZERO WON'T FINISH YOUR HOMEWORK.`,
      () => `REPLYING TO EMAILS IS PROCRASTINATION IN A DRESS SHIRT.`,
    ],
  },
  {
    match: /explorer|finder|file explorer/i,
    lines: [
      () => `"ORGANIZING FILES" IS THE OLDEST TRICK IN THE BOOK.`,
      () => `THE FOLDER ISN'T THE ASSIGNMENT. OPEN IT.`,
    ],
  },
  {
    match: /spotify|apple music|youtube music/i,
    lines: [
      () => `PICKING THE PERFECT PLAYLIST IS NOT STUDYING.`,
      () => `PUT ON SOMETHING. GO BACK TO WORK.`,
    ],
  },
];

// Generic fallback pool — used when nothing in APP_ROASTS matches.
const GENERIC_ROASTS = [
  (a) => `SERIOUSLY? ${(a || 'THIS').toUpperCase()}?`,
  () => `PATHETIC. GET BACK TO WORK.`,
  (a) => `${(a || 'THAT').toUpperCase()} ISN'T YOUR JOB.`,
  () => `WOW. GREAT COMMITMENT TO FAILING.`,
  (a) => `YOU CHOSE ${(a || 'THIS').toUpperCase()} OVER YOUR OWN FUTURE.`,
  () => `THIS IS EMBARRASSING TO WATCH.`,
  (a) => `${(a || 'IT').toUpperCase()} CAN WAIT. YOUR DEADLINE CAN'T.`,
  () => `STOP LYING TO YOURSELF.`,
  () => `NOBODY IS COMING TO SAVE YOUR GRADE.`,
  (a) => `${(a || 'THIS')} AGAIN? REALLY?`,
];

let lastRoastIdx = -1;

function pickRoastLine(app, detail) {
  const haystack = `${app || ''} ${detail || ''}`;
  const bucket = APP_ROASTS.find(r => r.match.test(haystack));
  const pool = bucket ? bucket.lines : GENERIC_ROASTS;
  let idx = Math.floor(Math.random() * pool.length);
  if (pool.length > 1 && idx === lastRoastIdx) {
    idx = (idx + 1) % pool.length;
  }
  lastRoastIdx = idx;
  return pool[idx](app, detail);
}

function showCyberOverlay(app, detail, elapsedSec, goal) {
  currentDriftApp = app || '';
  // If the overlay is already up, this is a re-tick of the same drift — don't wipe the
  // user's justification input, re-run the shake, or pick a fresh roast line. Only refresh
  // the sub-line in case the window title changed. This keeps a single, stable warning
  // instead of what looks like a second warning stacking on top every 5s.
  const alreadyVisible = cyberOverlay.style.display === 'flex';
  cyberOverlay.style.display = 'flex';

  if (!alreadyVisible) {
    resetCyberJustify();
    cyberContent.classList.remove('shake');
    void cyberContent.offsetWidth;
    cyberContent.classList.add('shake');

    if (cyberHero) {
      const line = pickRoastLine(app, detail);
      cyberHero.textContent = line;
      cyberHero.setAttribute('data-text', line);
    }
  }

  if (cyberSub) {
    // Dropped the "Nm off-task" tag: with a 15s dismiss cooldown, the user gets caught long
    // before "minutes" is meaningful, and the number was actually total session-elapsed,
    // not off-task time — misleading either way.
    const goalPart = goal ? ` — you said you'd be doing <em>${goal}</em>` : '';
    cyberSub.innerHTML = `${app || 'Unknown app'} — <em>${detail || 'unknown'}</em>${goalPart}`;
  }
}

function hideCyberOverlay() {
  if (cyberOverlay) cyberOverlay.style.display = 'none';
  try { invoke('hide_drift_overlay'); } catch(e) {}
}

demoDriftBtn.addEventListener('click', () => showCyberOverlay('Discord', '#general', 47, 'Calc HW'));

// "This is actually work" runs a two-step flow: click reveals a textarea; submit sends
// the reason through the local AI. The backend decides — if the AI calls it BS, we keep
// the overlay up and show the rejection message. All the correct/allowlist bookkeeping
// happens on the backend as part of that same call, so the frontend just reacts to the
// verdict.
const cyberActionsEl = document.getElementById('cyberActions');
const cyberJustifyEl = document.getElementById('cyberJustify');
const cyberJustifyInput = document.getElementById('cyberJustifyInput');
const cyberJustifySubmit = document.getElementById('cyberJustifySubmit');
const cyberJustifyCancel = document.getElementById('cyberJustifyCancel');
const cyberJustifyError = document.getElementById('cyberJustifyError');

function resetCyberJustify() {
  if (cyberActionsEl) cyberActionsEl.style.display = '';
  if (cyberJustifyEl) cyberJustifyEl.style.display = 'none';
  if (cyberJustifyInput) {
    cyberJustifyInput.value = '';
    cyberJustifyInput.disabled = false;
  }
  if (cyberJustifySubmit) {
    cyberJustifySubmit.disabled = false;
    cyberJustifySubmit.textContent = "Confirm — it's work";
  }
  if (cyberJustifyError) {
    cyberJustifyError.style.display = 'none';
    cyberJustifyError.textContent = '';
  }
}

function showJustifyError(msg) {
  if (!cyberJustifyError) return;
  cyberJustifyError.textContent = msg;
  cyberJustifyError.style.display = 'block';
  // Retrigger shake by removing/re-adding the class.
  cyberJustifyError.style.animation = 'none';
  void cyberJustifyError.offsetWidth;
  cyberJustifyError.style.animation = '';
}

async function submitWorkClaim(reason) {
  const trimmed = (reason || '').trim();
  if (cyberJustifySubmit) {
    cyberJustifySubmit.disabled = true;
    cyberJustifySubmit.textContent = 'Checking with local AI…';
  }
  if (cyberJustifyInput) cyberJustifyInput.disabled = true;
  if (cyberJustifyError) cyberJustifyError.style.display = 'none';

  let outcome = null;
  try {
    outcome = await invoke('submit_work_justification', { reason: trimmed });
  } catch(e) {
    outcome = { verdict: 'no_ai', message: null };
    // Fall back to raw correct + allowlist locally so the click isn't a total no-op.
    try { await invoke('correct_classification', { newStatus: 'on_task' }); } catch(_){}
    if (currentDriftApp) {
      try { await invoke('allow_app_this_session', { app: currentDriftApp }); } catch(_){}
    }
  }

  if (outcome?.verdict === 'rejected') {
    // Keep the overlay up, put the user back in the textarea to try again.
    if (cyberJustifySubmit) {
      cyberJustifySubmit.disabled = false;
      cyberJustifySubmit.textContent = "Confirm — it's work";
    }
    if (cyberJustifyInput) cyberJustifyInput.disabled = false;
    showJustifyError(outcome.message || 'That didn\'t land. Try again — or click "Just a break" and take it.');
    if (cyberJustifyInput) setTimeout(() => cyberJustifyInput.focus(), 0);
    return;
  }

  // Accepted or no-ai fallback: backend applied correct/allowlist already, dismiss.
  hideCyberOverlay();
  resetCyberJustify();
}

document.getElementById('cyberWorkBtn').addEventListener('click', () => {
  if (cyberJustifyEl && cyberActionsEl) {
    cyberActionsEl.style.display = 'none';
    cyberJustifyEl.style.display = 'flex';
    if (cyberJustifyInput) setTimeout(() => cyberJustifyInput.focus(), 0);
  } else {
    // Fallback path — no input UI in this window; submit as-is.
    submitWorkClaim('');
  }
});
if (cyberJustifySubmit) {
  cyberJustifySubmit.addEventListener('click', () => submitWorkClaim(cyberJustifyInput?.value || ''));
}
if (cyberJustifyCancel) {
  cyberJustifyCancel.addEventListener('click', () => resetCyberJustify());
}
if (cyberJustifyInput) {
  cyberJustifyInput.addEventListener('keydown', (e) => {
    if (e.key === 'Enter' && (e.ctrlKey || e.metaKey)) {
      e.preventDefault();
      submitWorkClaim(cyberJustifyInput.value || '');
    }
  });
}

document.getElementById('cyberBackBtn').addEventListener('click', () => {
  hideCyberOverlay();
  resetCyberJustify();
});

// Build warning ticker icons
const WARNING_ICON_SVG = `<svg viewBox="0 0 24 24" fill="none"><path d="M12 3 2 20h20L12 3Z" stroke="#FFB020" stroke-width="1.6" stroke-linejoin="round"/><path d="M12 9v5" stroke="#FFB020" stroke-width="1.8" stroke-linecap="round"/><circle cx="12" cy="17" r="1" fill="#FFB020"/></svg>`;
function buildWarningTicker(id, count) {
  const track = document.getElementById(id);
  let icons = '';
  for (let i = 0; i < count; i++) {
    icons += `<span class="cyber-warn-icon" style="--i:${i}">${WARNING_ICON_SVG}</span>`;
  }
  track.innerHTML = icons + icons;
}
buildWarningTicker('cyberTickerTop', 8);
buildWarningTicker('cyberTickerBottom', 8);

// ─── Tag input fields ────────────────────────────────────────────────

function createTagField({ boxId, listId, inputId, chipsId, suggestions, initialTags }) {
  let tags = (initialTags || []).slice();
  const box = document.getElementById(boxId);
  const list = document.getElementById(listId);
  const input = document.getElementById(inputId);
  const chipsRow = document.getElementById(chipsId);

  // Guard: if the markup for this field isn't present, return a no-op stub so
  // module-load doesn't throw and halt init().
  if (!box || !list || !input || !chipsRow) {
    return {
      get tags() { return tags.slice(); },
      updateSuggestions(newSugg) { suggestions = (newSugg || []).slice(); }
    };
  }

  function renderTags() {
    list.innerHTML = tags.map((t, i) => `
      <span class="tag-pill" data-i="${i}">${t}<button type="button" class="remove" aria-label="Remove ${t}">&times;</button></span>
    `).join('');
    list.querySelectorAll('.tag-pill .remove').forEach(btn => {
      btn.addEventListener('click', (e) => {
        e.stopPropagation();
        const i = parseInt(btn.closest('.tag-pill').dataset.i, 10);
        tags.splice(i, 1);
        renderTags();
        renderChips();
      });
    });
  }

  function renderChips() {
    const query = input.value.trim().toLowerCase();
    chipsRow.innerHTML = suggestions
      .filter(s => !tags.some(t => t.toLowerCase() === s.toLowerCase()))
      .filter(s => !query || s.toLowerCase().includes(query))
      .map(s => `<button type="button" class="chip" data-v="${s}">${s}<span class="plus">+</span></button>`)
      .join('');
    chipsRow.querySelectorAll('.chip').forEach(chip => {
      chip.addEventListener('click', () => addTag(chip.dataset.v));
    });
  }

  function addTag(raw) {
    const val = raw.trim();
    if (!val || tags.some(t => t.toLowerCase() === val.toLowerCase())) return;
    tags.push(val);
    renderTags();
    renderChips();
  }

  box.addEventListener('click', () => input.focus());
  input.addEventListener('keydown', (e) => {
    if (e.key === 'Enter' || e.key === ',') {
      e.preventDefault();
      addTag(input.value.replace(/,$/, ''));
      input.value = '';
    } else if (e.key === 'Backspace' && !input.value && tags.length) {
      tags.pop();
      renderTags();
      renderChips();
    }
  });
  input.addEventListener('input', renderChips);
  renderTags();
  renderChips();
  return {
    get tags() { return tags.slice(); },
    updateSuggestions(newSugg) {
      suggestions = (newSugg || []).slice();
      renderChips();
    }
  };
}

// Initialize tag fields
const startModalToolField = createTagField({
  boxId: 'toolTagBox', listId: 'toolTagList', inputId: 'toolTagInput', chipsId: 'toolChips',
  suggestions: ['ChatGPT', 'Google Docs', 'Overleaf', 'Canvas', 'Desmos'],
  initialTags: ['Overleaf', 'Desmos', 'Canvas']
});

// Settings tag fields are initialized from DB in setupSettings()

// ─── Focus time selector ─────────────────────────────────────────────

const timePills = document.querySelectorAll('.time-pill');
const timeCustomBox = document.getElementById('timeCustomBox');
const timeCustomInput = document.getElementById('timeCustomInput');

function selectTimeOption(el) {
  timePills.forEach(p => p.classList.remove('selected'));
  timeCustomBox.classList.remove('selected');
  el.classList.add('selected');
}

timePills.forEach(pill => { pill.addEventListener('click', () => selectTimeOption(pill)); });
timeCustomBox.addEventListener('click', () => { selectTimeOption(timeCustomBox); timeCustomInput.focus(); });
timeCustomInput.addEventListener('focus', () => selectTimeOption(timeCustomBox));

// ─── Roulette digits ─────────────────────────────────────────────────

function roulette(el, newText, prevText) {
  const chars = newText.split(''), prev = (prevText || '').padEnd(chars.length).split('');
  el.innerHTML = chars.map((c, i) => `<span class="rd${c !== prev[i] ? ' roll' : ''}">${c}</span>`).join('');
}

// ─── Session metrics ─────────────────────────────────────────────────

function setSessionOffState() {
  _lastActivityKey = '';
  _lastTimelineKey = '';
  document.getElementById('sMetOnTask').textContent = '—';
  document.getElementById('sMetElapsed').textContent = '—:——';
  document.getElementById('sMetDeep').textContent = '—:——';
  document.getElementById('sMetDrifts').textContent = '—';
  ['sMetOnTaskSub','sMetElapsedSub','sMetDeepSub','sMetDriftsSub'].forEach(id => {
    document.getElementById(id).textContent = '—';
  });
  const viz = document.getElementById('sessionTimelineViz');
  viz.innerHTML = ''; viz.removeAttribute('data-rendered');
}

function safeRoulette(el, textVal, defaultVal) {
  if (!el) return;
  if (el.dataset.lastVal === textVal) return;
  el.dataset.lastVal = textVal;
  roulette(el, textVal, defaultVal);
}

function applySessionState(s) {
  if (!s.active) return;
  elapsedSec = s.elapsed_sec;
  const pct = s.elapsed_sec > 0 ? Math.round((s.on_task_sec / s.elapsed_sec) * 100) : 100;
  safeRoulette(document.getElementById('sMetOnTask'), String(pct), '—');
  document.getElementById('sMetOnTaskSub').textContent = `${pct}% lifetime`;

  const m = Math.floor(s.elapsed_sec / 60), sec = s.elapsed_sec % 60;
  const t = `${String(m).padStart(2,'0')}:${String(sec).padStart(2,'0')}`;
  safeRoulette(document.getElementById('sMetElapsed'), t, '—:——');
  prevElapsedText = t;

  if (s.duration_target_min) {
    document.getElementById('sMetElapsedSub').textContent = `of ${s.duration_target_min}:00`;
  } else {
    document.getElementById('sMetElapsedSub').textContent = 'endless';
  }

  const dm = Math.floor(s.deep_focus_sec / 60), ds = s.deep_focus_sec % 60;
  safeRoulette(document.getElementById('sMetDeep'), `${String(dm).padStart(2,'0')}:${String(ds).padStart(2,'0')}`, '—:——');
  document.getElementById('sMetDeepSub').textContent = `streak ${String(Math.floor(s.current_streak_sec/60)).padStart(2,'0')}:${String(s.current_streak_sec%60).padStart(2,'0')}`;

  safeRoulette(document.getElementById('sMetDrifts'), String(s.drift_count), '—');
  document.getElementById('sMetDriftsSub').textContent = s.drift_count === 0 ? 'none yet' : `${s.drift_count} total`;

  // Update "now" bar
  const nowBar = document.getElementById('sessionNowBar');
  if (nowBar) {
    const glyph = nowBar.querySelector('.glyph');
    const path = nowBar.querySelector('.path');
    const verdict = nowBar.querySelector('.verdict');
    if (glyph && s.current_app) glyph.textContent = s.current_app.slice(0, 2);
    if (path) path.innerHTML = `${s.current_app} <span class="muted">— ${s.current_detail}</span>`;
    if (verdict) verdict.textContent = s.current_status === 'on_task' ? 'on task' : 'off task';
  }

  // Update Leaf Letout Card overlay
  const leafCard = document.getElementById('leafLetoutCard');
  if (leafCard) {
    leafCard.style.display = s.active ? 'flex' : 'none';
    
    // Remaining time calculation with roulette spinning animation
    const leafTime = document.getElementById('leafRemainingTime');
    if (leafTime) {
      let timeStr = '--:--';
      if (s.duration_target_min) {
        const totalTargetSec = s.duration_target_min * 60;
        const remSec = Math.max(0, totalTargetSec - s.elapsed_sec);
        const remM = Math.floor(remSec / 60), remS = remSec % 60;
        timeStr = `${String(remM).padStart(2,'0')}:${String(remS).padStart(2,'0')}`;
      } else {
        const m = Math.floor(s.elapsed_sec / 60), sec = s.elapsed_sec % 60;
        timeStr = `${String(m).padStart(2,'0')}:${String(sec).padStart(2,'0')}`;
      }
      roulette(leafTime, timeStr, prevLeafTimeText || '--:--');
      prevLeafTimeText = timeStr;
    }

    // Active App / Website
    const leafApp = document.getElementById('leafActiveApp');
    if (leafApp) {
      const appDisplay = s.current_app ? `${s.current_app}${s.current_detail ? ` · ${s.current_detail}` : ''}` : 'Desktop';
      leafApp.textContent = appDisplay;
      leafApp.title = appDisplay;
    }

    // Prof Xeno 1-liner comment — dynamic rotating pool
    const leafXeno = document.getElementById('leafXenoComment');
    if (leafXeno) {
      leafXeno.textContent = getXenoMessage(s);
    }
  }
}

// ─── Prof. Xeno dynamic message system ──────────────────────────────

const XENO_MSGS = {
  drift: [
    app => `"${app} again? The cosmos weeps."`,
    app => `"${app} is a black hole for your potential."`,
    app => `"Drift detected. Recalibrate, stargazer."`,
    app => `"${app}? That's not in your star chart today."`,
    app => `"The signal is fading… return to mission."`,
    app => `"You wandered off-orbit. Course-correct now."`,
    app => `"${app} won't get you tenure in this galaxy."`,
    app => `"Gravitational pull from ${app}. Resist it."`,
  ],
  streak_long: [
    `"15m+ unbroken focus. Galaxy brain unlocked."`,
    `"You're in the deep field now. Beautiful."`,
    `"Sustained orbit achieved. Keep burning steady."`,
    `"Your focus signature is off the charts."`,
    `"This is what peak cognition looks like."`,
    `"The universe bends to the disciplined mind."`,
  ],
  streak_mid: [
    `"Deep work momentum established. Stay locked."`,
    `"5 minutes of clean signal. Don't break it."`,
    `"You're building velocity. I can feel it."`,
    `"Focus is compounding. Keep the chain going."`,
    `"The noise is fading. You're in the zone."`,
  ],
  on_task: [
    goal => `"${goal} — the mission continues."`,
    goal => `"Steady hands, clear mind. ${goal} awaits."`,
    goal => `"Channel everything into ${goal}."`,
    goal => `"Lock in. The stars favor the focused."`,
    goal => `"One task, total commitment. That's the way."`,
    goal => `"${goal} won't finish itself. You've got this."`,
  ],
  idle: [
    `"Awaiting your signal, operator."`,
    `"The lab is quiet. Begin when ready."`,
    `"Systems nominal. Waiting for ignition."`,
    `"Prof. Xeno is watching. No pressure."`,
  ],
};
let xenoLastIdx = {}, xenoLastChange = 0;

function getXenoMessage(s) {
  const now = Date.now();
  if (now - xenoLastChange < 15000 && leafXenoCache) return leafXenoCache;
  xenoLastChange = now;

  let pool, key, arg;
  if (s.current_status === 'off_task') {
    key = 'drift'; pool = XENO_MSGS.drift; arg = s.current_app || 'Drift';
  } else if (s.current_streak_sec >= 900) {
    key = 'streak_long'; pool = XENO_MSGS.streak_long;
  } else if (s.current_streak_sec >= 300) {
    key = 'streak_mid'; pool = XENO_MSGS.streak_mid;
  } else if (s.goal) {
    key = 'on_task'; pool = XENO_MSGS.on_task; arg = s.goal;
  } else {
    key = 'idle'; pool = XENO_MSGS.idle;
  }

  let idx = (xenoLastIdx[key] || 0) + 1;
  if (idx >= pool.length) idx = 0;
  xenoLastIdx[key] = idx;

  const entry = pool[idx];
  leafXenoCache = typeof entry === 'function' ? entry(arg) : entry;
  return leafXenoCache;
}
let leafXenoCache = '';

function startLocalTimer(durationMin) {
  if (timerInterval) clearInterval(timerInterval);
  timerInterval = setInterval(() => {
    if (!sessionActive) { clearInterval(timerInterval); return; }
    elapsedSec++;
    const m = Math.floor(elapsedSec / 60), s = elapsedSec % 60;
    const t = `${String(m).padStart(2,'0')}:${String(s).padStart(2,'0')}`;
    roulette(document.getElementById('sMetElapsed'), t, prevElapsedText);
    prevElapsedText = t;
  }, 1000);
}

// ─── Session activity (live) ─────────────────────────────────────────

let _lastActivityKey = '';
let _lastTimelineKey = '';

function renderSessionActivity() {
  const list = document.getElementById('sessionActivityList');
  if (!list || !currentSessionId) return;
  invoke('get_session_intervals', { sessionId: currentSessionId }).then(intervals => {
    const recent = intervals.slice(-10);
    const key = recent.map(a => `${a.process_name}|${a.start_ts}|${a.end_ts}|${a.status}`).join(';');
    if (key === _lastActivityKey) return;
    _lastActivityKey = key;
    renderActivityList(list, recent);
  }).catch(() => {});
}

// ─── Activity lists ──────────────────────────────────────────────────

function getAppIconHtml(category, procName) {
  const cat = (category || procName || '').toLowerCase();
  if (cat.includes('invigil')) {
    return { html: `<svg viewBox="0 0 24 24" width="22" height="22" fill="none"><rect x="2" y="2" width="20" height="20" rx="5" fill="#5A7C4E"/><path d="M8 7v10M12 7v10M16 7v10" stroke="#F4EEDC" stroke-width="2" stroke-linecap="round"/></svg>`, hasIcon: true };
  }
  if (cat.includes('chrome')) {
    return { html: `<svg viewBox="0 0 24 24" width="22" height="22"><circle cx="12" cy="12" r="10" fill="#4285F4"/><circle cx="12" cy="12" r="4" fill="#FFF"/><circle cx="12" cy="12" r="3" fill="#4285F4"/><path d="M12 2a10 10 0 0 1 8.66 5H12" fill="#EA4335"/><path d="M20.66 7A10 10 0 0 1 12 22v-10" fill="#FBBC05"/><path d="M12 22a10 10 0 0 1-8.66-15H12" fill="#34A853"/></svg>`, hasIcon: true };
  }
  if (cat.includes('antigravity')) {
    return { html: `<svg viewBox="0 0 24 24" width="22" height="22" fill="none"><path d="M12 2L2 22h20L12 2z" fill="url(#agGrad)"/><defs><linearGradient id="agGrad" x1="0" y1="0" x2="1" y2="1"><stop offset="0%" stop-color="#FF4D5E"/><stop offset="50%" stop-color="#3FE8D8"/><stop offset="100%" stop-color="#7A9BB0"/></linearGradient></defs></svg>`, hasIcon: true };
  }
  if (cat.includes('discord')) {
    return { html: `<svg viewBox="0 0 24 24" width="22" height="22" fill="#5865F2"><path d="M20.317 4.37a19.791 19.791 0 0 0-4.885-1.515.074.074 0 0 0-.079.037c-.21.375-.444.864-.608 1.25a18.27 18.27 0 0 0-5.487 0 12.64 12.64 0 0 0-.617-1.25.077.077 0 0 0-.079-.037A19.736 19.736 0 0 0 3.677 4.37a.07.07 0 0 0-.032.027C.533 9.046-.32 13.58.099 18.057a.082.082 0 0 0 .031.057 19.9 19.9 0 0 0 5.993 3.03.078.078 0 0 0 .084-.028 14.09 14.09 0 0 0 1.226-1.994.076.076 0 0 0-.041-.106 13.107 13.107 0 0 1-1.872-.892.077.077 0 0 1-.008-.128 10.2 10.2 0 0 0 .372-.292.074.074 0 0 1 .077-.01c3.928 1.793 8.18 1.793 12.061 0a.074.074 0 0 1 .078.01c.12.098.246.198.373.292a.077.077 0 0 1-.006.127 12.299 12.299 0 0 1-1.873.892.077.077 0 0 0-.041.107c.36.698.772 1.362 1.225 1.993a.076.076 0 0 0 .084.028 19.839 19.839 0 0 0 6.002-3.03.077.077 0 0 0 .032-.054c.5-5.177-.838-9.674-3.549-13.66a.061.061 0 0 0-.031-.028z"/></svg>`, hasIcon: true };
  }
  if (cat.includes('youtube')) {
    return { html: `<svg viewBox="0 0 24 24" width="22" height="22" fill="#FF0000"><path d="M23.498 6.186a3.016 3.016 0 0 0-2.122-2.136C19.505 3.545 12 3.545 12 3.545s-7.505 0-9.377.505A3.017 3.017 0 0 0 .502 6.186C0 8.07 0 12 0 12s0 3.93.502 5.814a3.016 3.016 0 0 0 2.122 2.136c1.871.505 9.376.505 9.376.505s7.505 0 9.377-.505a3.015 3.015 0 0 0 2.122-2.136C24 15.93 24 12 24 12s0-3.93-.502-5.814zM9.545 15.568V8.432L15.818 12l-6.273 3.568z"/></svg>`, hasIcon: true };
  }
  // Match specifically on "vs code", "vscode", or the process file "code.exe" — the old
  // `cat.includes('code')` grabbed anything with the letters c-o-d-e (Claude Code windows,
  // "codepen", etc.) and, worse, the SVG under it was actually a checkmark path, not the
  // VS Code chevron. Real one below.
  if (/(?:^|\W)(?:vscode|vs code|code\.exe|code - insiders)/.test(cat)) {
    return { html: `<svg viewBox="0 0 100 100" width="22" height="22"><path d="M70.9 99.3L92.4 89c1.6-.8 2.6-2.4 2.6-4.2V15.2c0-1.8-1-3.4-2.6-4.2L70.9.7c-2.1-1-4.6-.6-6.3.9L23.8 38.4 6.1 25c-1.7-1.3-4-1.2-5.5.2-1.6 1.4-1.6 3.9 0 5.3l15.4 19.5L.6 69.5c-1.6 1.4-1.6 3.9 0 5.3 1.5 1.4 3.8 1.5 5.5.2l17.7-13.4 40.8 36.8c1.7 1.5 4.2 1.9 6.3.9zM75 27.2l-31 22.8 31 22.8V27.2z" fill="#007ACC"/></svg>`, hasIcon: true };
  }
  if (/(?:^|\W)(?:explorer\.exe|file explorer|windows explorer)|(?:^explorer$)/.test(cat)) {
    return { html: `<svg viewBox="0 0 24 24" width="22" height="22"><path d="M3 6a2 2 0 0 1 2-2h4l2 2h8a2 2 0 0 1 2 2v10a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2V6z" fill="#FFC842"/><path d="M3 8h18v3H3z" fill="#E6A11C"/></svg>`, hasIcon: true };
  }
  if (cat.includes('firefox')) {
    return { html: `<svg viewBox="0 0 24 24" width="22" height="22"><circle cx="12" cy="12" r="10" fill="#FF7139"/><path d="M12 4a8 8 0 1 0 0 16 8 8 0 0 0 0-16zm4 8c0 2.2-1.8 4-4 4s-4-1.8-4-4c0-.6.1-1.1.3-1.6.4 1 1.4 1.6 2.5 1.6.9 0 1.7-.4 2.2-1.1.5.7 1.3 1.1 2.2 1.1 1.1 0 2.1-.6 2.5-1.6.2.5.3 1 .3 1.6z" fill="#FFB84D"/></svg>`, hasIcon: true };
  }
  if (cat.includes('edge')) {
    return { html: `<svg viewBox="0 0 24 24" width="22" height="22"><circle cx="12" cy="12" r="10" fill="#0078D7"/><path d="M12 4a8 8 0 0 1 8 8c0 2-1 4-3 5-1 .5-2 .5-3 0-1.5-1-2-3-1-4.5.5-1 2-1.5 3-1H8c-2 0-3 1-3 3s1.5 4 4 4c2 0 3-1 4-2h5c-1 3-4 5-7 5-4 0-8-3-8-8s3-9 8-9z" fill="#33B4E5"/></svg>`, hasIcon: true };
  }
  // Fallback: soft initials tile — deliberately unopinionated so it's clearly a placeholder
  // and not a wrong-looking real logo.
  const init = (category || procName || '??').replace(/\.exe$/i, '').slice(0, 2).toUpperCase();
  return { html: `<span style="font-weight:700;font-size:12px;color:#fff;">${init}</span>`, hasIcon: false };
}

function renderSessionTimeline() {
  const viz = document.getElementById('sessionTimelineViz');
  if (!viz || !currentSessionId) return;

  invoke('get_session_intervals', { sessionId: currentSessionId }).then(intervals => {
    if (!intervals || intervals.length === 0) return;

    const tKey = intervals.map(a => `${a.process_name}|${a.start_ts}|${a.end_ts}|${a.status}`).join(';');
    if (tKey === _lastTimelineKey) return;
    _lastTimelineKey = tKey;

    const W = 600, H = 50;
    const n = Math.max(intervals.length, 2);
    const step = W / (n - 1 || 1);

    const points = intervals.map((inv, i) => {
      const val = inv.status === 'on_task' ? 100 : inv.status === 'off_task' ? 0 : 50;
      const x = i * step;
      const y = H - (val / 100) * (H - 12) - 6;
      return [x, y];
    });

    const pathD = points.reduce((acc, pt, i) => i === 0 ? `M ${pt[0]} ${pt[1]}` : `${acc} L ${pt[0]} ${pt[1]}`, '');
    const areaD = `${pathD} L ${points[points.length-1][0]} ${H} L 0 ${H} Z`;
    const lastPt = points[points.length - 1];

    viz.innerHTML = `
      <svg class="chart-svg" viewBox="0 0 ${W} ${H}" preserveAspectRatio="none" aria-hidden="true" style="width:100%;height:100%;">
        <defs>
          <linearGradient id="sessionTimelineFill" x1="0" y1="0" x2="0" y2="1">
            <stop offset="0%" stop-color="var(--moss)" stop-opacity="0.35"/>
            <stop offset="100%" stop-color="var(--moss)" stop-opacity="0"/>
          </linearGradient>
        </defs>
        <line x1="0" y1="6" x2="${W}" y2="6" stroke="var(--line)" stroke-dasharray="2 4"/>
        <line x1="0" y1="${H-6}" x2="${W}" y2="${H-6}" stroke="var(--line)" stroke-dasharray="2 4"/>
        <path d="${areaD}" fill="url(#sessionTimelineFill)"/>
        <path d="${pathD}" fill="none" stroke="var(--moss-deep)" stroke-width="2" stroke-linejoin="round" stroke-linecap="round"/>
      </svg>
      <div class="session-dot" style="position:absolute; left:${(lastPt[0]/W*100).toFixed(1)}%; top:${(lastPt[1]/H*100).toFixed(1)}%; width:8px; height:8px; border-radius:50%; background:var(--moss-deep); box-shadow: 0 0 0 4px color-mix(in oklab, var(--moss) 30%, transparent); transform:translate(-50%,-50%);"></div>
    `;
  }).catch(() => {});
}

function formatTimeShort(isoStr) {
  if (!isoStr) return '';
  const d = new Date(isoStr);
  if (isNaN(d.getTime())) return '';
  return d.toLocaleTimeString([], { hour: 'numeric', minute: '2-digit' });
}

function renderActivityList(container, items) {
  if (!items || items.length === 0) {
    container.innerHTML = '<div class="empty-state">No activity yet.</div>';
    return;
  }
  const colors = { 'on_task': '#408040', 'off_task': '#C5714A', 'ambiguous': '#7A9BB0' };
  // Reverse order so newest activity appears at the top of the list
  const orderedItems = items.slice().reverse();
  container.innerHTML = orderedItems.map(a => {
    const status = a.status || 'on_task';
    const color = colors[status] || '#666';
    const icon = getAppIconHtml(a.category, a.process_name);
    const startTime = formatTimeShort(a.start_ts);
    const dur = a.end_ts
      ? formatDuration(new Date(a.end_ts) - new Date(a.start_ts))
      : 'now';
    const timeDisplay = startTime ? `${startTime} · ${dur}` : dur;
    const bgStyle = icon.hasIcon ? 'background:transparent;' : `background:${color};`;
    const iconClass = icon.hasIcon ? 'a-icon' : 'a-icon no-icon';
    return `<div class="activity-row">
      <div class="${iconClass}" style="${bgStyle}display:flex;align-items:center;justify-content:center;">${icon.html}</div>
      <div><span class="a-name">${a.category || a.process_name}</span> <span class="a-detail">— ${a.window_title || ''}</span></div>
      <div class="a-dur">${timeDisplay}</div>
      <span class="a-badge ${status === 'on_task' ? 'on-task' : 'drift'}">${status === 'on_task' ? 'on task' : 'drift'}</span>
    </div>`;
  }).join('');
}

function formatDuration(ms) {
  const sec = Math.floor(ms / 1000);
  const m = Math.floor(sec / 60), s = sec % 60;
  return `${m}:${String(s).padStart(2, '0')}`;
}

function renderDashActivity() {
  const list = document.getElementById('dashActivityList');
  const title = document.getElementById('dashActivityTitle');
  const meta = document.getElementById('dashActivityMeta');
  meta.textContent = '';

  // If the user picked a date on the calendar, follow that date instead of always showing
  // today's activity. Range mode uses the whole range.
  if (state.mode === 'single' && state.start) {
    title.textContent = `Activity — ${fmtMonthDay(state.start)}`;
    loadActivityForRange(state.start, state.start, list, meta);
    return;
  }
  if (state.mode === 'range' && state.start && state.end) {
    title.textContent = `Activity — ${fmtMonthDay(state.start)} → ${fmtMonthDay(state.end)}`;
    loadActivityForRange(state.start, state.end, list, meta);
    return;
  }

  title.textContent = 'Activity today';
  if (liveData?.recent_sessions?.length > 0) {
    const latestId = liveData.recent_sessions[0].id;
    invoke('get_session_intervals', { sessionId: latestId }).then(intervals => {
      meta.textContent = `${intervals.length} entries`;
      renderActivityList(list, intervals.slice(-10));
    }).catch(() => {
      list.innerHTML = '<div class="empty-state">No activity recorded.</div>';
    });
  } else {
    list.innerHTML = '<div class="empty-state">Start a session to see activity.</div>';
  }
}

// Load activity intervals for the sessions inside a date range and paint them into the
// dashboard activity card. Called by renderDashActivity when the calendar has a selection.
function loadActivityForRange(startDate, endDate, list, meta) {
  invoke('get_sessions_in_range', { start: startDate, end: endDate })
    .then(sessions => {
      if (!sessions || sessions.length === 0) {
        list.innerHTML = '<div class="empty-state">No sessions on that date.</div>';
        return;
      }
      // Pull intervals from each session and merge.
      return Promise.all(
        sessions.map(s => invoke('get_session_intervals', { sessionId: s.id }).catch(() => []))
      ).then(all => {
        const flat = all.flat().sort((a, b) => (a.start_ts || '').localeCompare(b.start_ts || ''));
        meta.textContent = `${flat.length} entries`;
        if (flat.length === 0) {
          list.innerHTML = '<div class="empty-state">No activity in that window.</div>';
        } else {
          renderActivityList(list, flat.slice(-10));
        }
      });
    })
    .catch(() => {
      list.innerHTML = '<div class="empty-state">Could not load activity.</div>';
    });
}

// ─── Settings ────────────────────────────────────────────────────────

async function setupSettings() {
  try {
    const settings = await invoke('get_settings');
    const idleInput = document.getElementById('idleTimeoutInput');
    if (idleInput) {
      idleInput.value = settings.idle_timeout_sec;
      idleInput.addEventListener('change', () => {
        invoke('update_setting', { key: 'idle_timeout_sec', value: idleInput.value });
      });
    }
  } catch (e) {
    console.warn('Could not load settings:', e);
  }

  // Load profiles for deny list
  try {
    const profiles = await invoke('get_profiles');
    const defaultProfile = profiles.find(p => p.id === 'default') || profiles[0];
    if (defaultProfile) {
      const denyField = createTagField({
        boxId: 'denyTagBox', listId: 'denyTagList', inputId: 'denyTagInput', chipsId: 'denyChips',
        suggestions: ['YouTube', 'Discord — #general', 'Instagram', 'Reddit', 'TikTok', 'iMessage web'],
        initialTags: defaultProfile.deny_patterns || [],
      });

      let saveTimeout;
      const observer = new MutationObserver(() => {
        clearTimeout(saveTimeout);
        saveTimeout = setTimeout(() => {
          invoke('save_profile', {
            profile: {
              id: defaultProfile.id,
              name: defaultProfile.name,
              allow_patterns: defaultProfile.allow_patterns || [],
              deny_patterns: denyField.tags,
            }
          });
        }, 500);
      });
      const denyList = document.getElementById('denyTagList');
      if (denyList) observer.observe(denyList, { childList: true, subtree: true });
    }
  } catch (e) {
    console.warn('Could not load profiles:', e);
    createTagField({
      boxId: 'denyTagBox', listId: 'denyTagList', inputId: 'denyTagInput', chipsId: 'denyChips',
      suggestions: ['YouTube', 'Discord — #general', 'Instagram', 'Reddit', 'TikTok', 'iMessage web'],
      initialTags: ['YouTube', 'Instagram', 'Reddit'],
    });
  }
}

// ─── Backend event listeners ─────────────────────────────────────────

async function setupListeners() {
  // Session tick results from monitoring loop
  await listen('session-tick-result', (event) => {
    const result = event.payload;
    if (result?.state) {
      applySessionState(result.state);
      elapsedSec = result.state.elapsed_sec;
      currentGoal = result.state.goal || '';
      renderSessionTimeline();
      renderSessionActivity();
      updateAdvPanel(result.state);
    }
  });

  // Session timer expired
  await listen('session-expired', async () => {
    try {
      const summary = await invoke('end_session');
      showSummaryAnimated(summary);
    } catch (e) {
      console.error('Failed to end expired session:', e);
      setSessionUI(false);
    }
  });

  // Wire Leaf Overlay Stop Session Button
  const leafStopBtn = document.getElementById('leafStopBtn');
  if (leafStopBtn) {
    leafStopBtn.addEventListener('click', async () => {
      if (sessionActive) {
        try {
          const summary = await invoke('end_session');
          currentSessionId = null;
          showSummaryAnimated(summary);
        } catch (e) {
          console.error('Failed to end session from leaf card:', e);
          currentSessionId = null;
          setSessionUI(false);
        }
      }
    });
  }
}

// Listeners for the dedicated drift_overlay window. Kept separate from setupListeners()
// because the overlay window strips out .page-wrap on load, so applySessionState() would
// crash on the many DOM elements that no longer exist. Only what the overlay actually
// needs is registered here.
async function setupOverlayListeners() {
  // Track the current session goal so the overlay's sub-line can say "you said you'd
  // be doing <goal>". Cheap: just pulls the string, no DOM touching.
  await listen('session-tick-result', (event) => {
    const s = event.payload?.state;
    if (s) currentGoal = s.goal || '';
  });

  await listen('drift-detected', async (event) => {
    const { app, detail, elapsed_sec } = event.payload;
    showCyberOverlay(app, detail, elapsed_sec, currentGoal);
    try {
      if (window.__TAURI__?.window) {
        const win = window.__TAURI__.window.getCurrentWindow();
        await win.setAlwaysOnTop(true);
        await win.show();
        await win.setFocus();
      }
    } catch(e) {}
  });
}

// ─── Count-up animation ──────────────────────────────────────────────

function countUp(el, from, to, duration, prefix) {
  prefix = prefix || '';
  const start = performance.now();
  function tick(now) {
    const t = Math.min((now - start) / duration, 1);
    const ease = 1 - Math.pow(1 - t, 3);
    const val = Math.round(from + (to - from) * ease);
    el.textContent = prefix + val.toLocaleString();
    if (t < 1) requestAnimationFrame(tick);
  }
  requestAnimationFrame(tick);
}

// ─── Summary animation ──────────────────────────────────────────────

function showSummaryAnimated(summary) {
  sessionActive = false;
  if (timerInterval) { clearInterval(timerInterval); timerInterval = null; }

  const modal = document.getElementById('summaryModalBackdrop');
  const card = modal.querySelector('.summary-card');
  const metrics = modal.querySelectorAll('.summary-metric');
  const pointsBox = document.getElementById('summaryPointsBox');
  const breakdownItems = document.querySelectorAll('#summaryBreakdown .s-fade');
  const totalRow = document.getElementById('summaryTotalRow');
  const note = document.getElementById('summaryNote');
  const doneBtn = document.getElementById('summaryDoneBtn');
  const sv0 = document.getElementById('sbVal0');
  const sv1 = document.getElementById('sbVal1');
  const stv = document.getElementById('summaryTotalVal');

  // Update summary goal text
  const goalEl = modal.querySelector('.summary-goal');
  if (goalEl && summary) goalEl.textContent = summary.goal || '';

  // Reset
  metrics.forEach(m => { m.classList.add('pre'); m.classList.remove('slap'); m.querySelector('.cv').textContent = '0'; });
  pointsBox.classList.add('s-hide'); pointsBox.classList.remove('s-show');
  breakdownItems.forEach(s => s.classList.remove('in'));
  totalRow.classList.remove('slap'); totalRow.style.opacity = '0'; totalRow.style.transform = 'scale(0)';
  note.classList.add('s-hide'); note.classList.remove('s-show');
  doneBtn.classList.add('s-hide'); doneBtn.classList.remove('s-show');
  if (sv0) sv0.textContent = '0';
  if (sv1) sv1.textContent = '0';
  // Neutral zero — countUp will paint the correct sign when it runs. Setting "+0" here
  // and then "−800" from countUp caused a "+−800" glitch on some frames.
  if (stv) stv.textContent = '0';

  // Update breakdown labels
  if (summary?.point_breakdown) {
    const bd = summary.point_breakdown;
    const labels = document.querySelectorAll('#summaryBreakdown .s-fade:not(.v)');
    // Label matches the actual formula (on-task time × 50/min prorated), not elapsed
    // session length — otherwise "1 min × 50" shows a value of 0 and looks broken.
    const onSec = summary.on_task_sec ?? 0;
    const onMin = Math.floor(onSec / 60);
    const onRem = onSec % 60;
    const onLabel = onMin > 0 ? `${onMin}m ${onRem}s` : `${onRem}s`;
    if (labels[0]) labels[0].textContent = `${onLabel} × 50/min`;
    if (labels[1]) labels[1].textContent = `${summary.drift_count} distraction${summary.drift_count === 1 ? '' : 's'} × −100`;
    if (labels[2]) labels[2].textContent = `streak bonus`;
    const streakLabel = document.querySelectorAll('#summaryBreakdown .v.up.s-fade');
    if (streakLabel[0]) streakLabel[0].textContent = `×${bd.streak_multiplier}`;
  }

  modal.style.display = 'flex';

  function shake() { card.classList.remove('shaking'); void card.offsetWidth; card.classList.add('shaking'); }

  const targets = [
    Math.round(summary?.on_task_pct ?? 0),
    summary?.duration_min ?? 0,
    summary?.drift_count ?? 0,
  ];

  let d = 500;
  metrics.forEach((m, i) => {
    setTimeout(() => {
      m.classList.remove('pre'); m.classList.add('slap'); shake();
      const cv = m.querySelector('.cv');
      if (i === 1 && summary && summary.on_task_sec !== undefined) {
        const totalSec = summary.on_task_sec;
        const min = Math.floor(totalSec / 60);
        const sec = totalSec % 60;
        const valEl = m.querySelector('.value');
        if (valEl) {
          if (min > 0 && sec > 0) {
            valEl.innerHTML = `<span class="cv">${min}</span><span class="unit">min </span><span class="cv">${sec}</span><span class="unit">sec</span>`;
          } else if (sec > 0) {
            valEl.innerHTML = `<span class="cv">${sec}</span><span class="unit">sec</span>`;
          } else {
            valEl.innerHTML = `<span class="cv">${min}</span><span class="unit">min</span>`;
          }
        }
      } else if (cv) {
        countUp(cv, 0, targets[i], 350);
      }
    }, d + i * 350);
  });
  d += metrics.length * 350 + 400;

  setTimeout(() => { pointsBox.classList.remove('s-hide'); pointsBox.classList.add('s-show'); }, d);
  d += 250;

  const pairs = [[0,1],[2,3],[4,5]];
  const bp = summary?.point_breakdown;
  pairs.forEach(([a,b], ri) => {
    setTimeout(() => {
      if (breakdownItems[a]) breakdownItems[a].classList.add('in');
      if (breakdownItems[b]) breakdownItems[b].classList.add('in');
      if (ri === 0 && sv0 && bp) countUp(sv0, 0, bp.base_points, 450);
      if (ri === 1 && sv1 && bp) countUp(sv1, 0, Math.abs(bp.drift_penalty), 450, '−');
    }, d + ri * 300);
  });
  d += pairs.length * 300 + 500;

  setTimeout(() => {
    totalRow.style.opacity = ''; totalRow.style.transform = '';
    totalRow.classList.add('slap'); shake();
    // Sign prefix depends on the value now that totals can go negative: "+120", "−400", "0".
    if (stv && bp) {
      const prefix = bp.total > 0 ? '+' : (bp.total < 0 ? '−' : '');
      countUp(stv, 0, Math.abs(bp.total), 800, prefix);
    }
  }, d);
  d += 900;

  // Generate a Xeno note
  const noteText = generateXenoNote(summary);
  setTimeout(() => {
    note.innerHTML = `${noteText}<span class="sig">— Professor Xeno</span>`;
    note.classList.remove('s-hide'); note.classList.add('s-show');
    doneBtn.classList.remove('s-hide'); doneBtn.classList.add('s-show');
  }, d);
}

function generateXenoNote(summary) {
  if (!summary) return 'Session complete.';
  const pct = Math.round(summary.on_task_pct);
  const drifts = summary.drift_count;
  const min = summary.duration_min;

  if (pct >= 95 && drifts === 0) return `Clean sweep. ${min} minutes, zero drifts. That's the standard.`;
  if (pct >= 90) return `Strong lock-in. ${drifts > 0 ? `${drifts} drift${drifts > 1 ? 's' : ''} barely made a dent.` : 'Not a single drift.'} Keep building.`;
  if (pct >= 75) return `Solid work — ${pct}% on task across ${min} minutes. ${drifts > 2 ? 'Rough patches, but you came back.' : 'Room to tighten.'}`;
  if (pct >= 50) return `${min} minutes, ${pct}% on task. The drifts are eating real time — next session, try a 25-minute block to stay sharper.`;
  return `Tough one — ${pct}% on task. Don't sweat it, just start the next one cleaner.`;
}

function resetSummaryAnimation() {
  document.querySelectorAll('#summaryModalBackdrop .summary-metric').forEach(m => m.classList.remove('pre','slap'));
  ['summaryPointsBox','summaryNote'].forEach(id => document.getElementById(id).classList.remove('s-hide','s-show'));
  document.getElementById('summaryDoneBtn').classList.remove('s-hide','s-show');
  const tr = document.getElementById('summaryTotalRow');
  tr.classList.remove('slap'); tr.style.opacity = ''; tr.style.transform = '';
  document.querySelectorAll('#summaryBreakdown .s-fade').forEach(s => s.classList.remove('in'));
}

// ─── Insight cards (data-driven) ────────────────────────────────────

function renderInsights() {
  renderWhatIfInsight();
  renderMindInsight();
}

function renderWhatIfInsight() {
  const body = document.getElementById('insightWhatIfBody');
  if (!body || !liveData) return;

  const distractions = liveData.distractions || [];
  const totalDriftMin = distractions.reduce((s, d) => s + d.minutes, 0);

  if (totalDriftMin === 0) {
    body.innerHTML = '<div class="empty-state">No drift time this week — nothing lost!</div>';
    return;
  }

  const h = Math.floor(totalDriftMin / 60);
  const m = totalDriftMin % 60;
  const timeStr = h > 0 ? `${h}h ${m}m` : `${m}m`;
  const topApps = distractions.slice(0, 3).map(d => d.name).join(', ');

  body.innerHTML = `
    <h3 class="insight-headline">You lost <em>${timeStr}</em> to drifting this week${topApps ? ` — mostly ${topApps}` : ''}.</h3>
    <div class="insight-foot">Track it, don't stress it. Next week, aim 30% tighter.</div>
  `;
}

function renderMindInsight() {
  const body = document.getElementById('insightMindBody');
  if (!body || !liveData) return;

  const trend = liveData.trend_14d || [];
  if (trend.length < 3) {
    body.innerHTML = '<div class="empty-state">Need more data to spot patterns. Keep going!</div>';
    return;
  }

  // Compute avg focus
  const avgMin = Math.round(trend.reduce((s, t) => s + t.total_minutes, 0) / trend.length);
  const avgPct = Math.round(trend.reduce((s, t) => s + (t.on_task_pct || 0), 0) / trend.length);

  // Compare first half to second half for trend direction
  const half = Math.floor(trend.length / 2);
  const firstHalf = trend.slice(0, half);
  const secondHalf = trend.slice(half);
  const avgFirst = firstHalf.reduce((s, t) => s + t.total_minutes, 0) / firstHalf.length;
  const avgSecond = secondHalf.reduce((s, t) => s + t.total_minutes, 0) / secondHalf.length;
  const trendDir = avgSecond > avgFirst ? 'up' : avgSecond < avgFirst ? 'down' : 'steady';

  const items = [];
  items.push(`Your average daily focus is <b>${avgMin} min</b> at <b>${avgPct}%</b> on-task.`);

  if (trendDir === 'up') {
    items.push(`Your focus time is <b>trending up</b> — keep the momentum.`);
  } else if (trendDir === 'down') {
    items.push(`Focus has been <b>dipping recently</b>. Try shorter sessions to rebuild.`);
  } else {
    items.push(`Holding <b>steady</b> — consistency is its own win.`);
  }

  const streakDays = liveData.streak?.current || 0;
  if (streakDays >= 3) {
    items.push(`<b>${streakDays}-day streak</b> — that's discipline compounding.`);
  }

  const icoSvg = '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8"><circle cx="12" cy="12" r="4"/><path d="M12 2v3M12 19v3M4.2 4.2l2.1 2.1M17.7 17.7l2.1 2.1M2 12h3M19 12h3M4.2 19.8l2.1-2.1M17.7 6.3l2.1-2.1"/></svg>';

  body.innerHTML = `
    <h3 class="insight-headline">Here's what your patterns say.</h3>
    <div class="insight-list">
      ${items.map(i => `<div class="insight-item"><div class="ico">${icoSvg}</div><div>${i}</div></div>`).join('')}
    </div>
    <div class="insight-foot">Xeno reads your patterns, not your business. This stays on-device.</div>
  `;
}

// ─── Advanced Telemetry Panel ────────────────────────────────────────

let advPanelOpen = false;

function setupAdvPanel() {
  const toggle = document.getElementById('advToggle');
  const panel = document.getElementById('advPanel');
  const body = document.getElementById('advBody');
  if (!toggle || !panel) return;

  toggle.addEventListener('click', () => {
    advPanelOpen = !advPanelOpen;
    panel.classList.toggle('open', advPanelOpen);
    if (body) body.style.display = advPanelOpen ? '' : 'none';
  });
  if (body) body.style.display = 'none';
}

function updateAdvPanel(s) {
  if (!advPanelOpen || !sessionActive) return;

  const aiCalls = document.getElementById('advAiCalls');
  const aiBar = document.getElementById('advAiBar');
  const aiStatus = document.getElementById('advAiStatus');
  const cpuEl = document.getElementById('advCpu');
  const cpuBar = document.getElementById('advCpuBar');
  const memEl = document.getElementById('advMemory');

  const llmCalls = s.llm_calls || 0;
  const maxCalls = Math.max(llmCalls, 20);
  if (aiCalls) aiCalls.textContent = llmCalls;
  if (aiBar) aiBar.style.width = `${Math.min(100, (llmCalls / maxCalls) * 100)}%`;
  if (aiStatus) aiStatus.textContent = llmCalls > 0 ? 'Gemma 4B active' : 'Idle — rule engine only';

  const cpu = s.cpu_usage || 0;
  const mem = s.memory_mb || 0;
  if (cpuEl) cpuEl.textContent = `${cpu.toFixed(1)}%`;
  if (cpuBar) cpuBar.style.width = `${Math.min(100, cpu)}%`;
  if (memEl) memEl.textContent = `${mem.toFixed(0)} MB`;
}

// ─── Init on load ────────────────────────────────────────────────────

init();
