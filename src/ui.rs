//! The sidebar TUI of one pane.

use std::collections::HashMap;
use std::io::Write;
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers, MouseButton, MouseEventKind};

use crate::config::{
    status_def, t, Col, BG_CHOICES, FINISH_WAIT, HIDDEN_POLL, HIDDEN_REFRESH, ORDER_SEP, POLL,
    REFRESH, SEL_BG, SEP, SIDEBAR_WIDTH, SPINNER, TICK,
};
use crate::git;
use crate::live::{resolve_status, session_info, state};
use crate::model::{collect_agents, sort_agents, Agent};
use crate::panes::jump;
use crate::screen::{Color, Screen, Style};
use crate::tmux;
use crate::tmux::sidebar_panes;
use crate::transcript::{fmt_tokens, transcript_info};
use crate::worktree;

/// Lines of one agent: name, branch, worktree (+ separator).
const LINES_PER_AGENT: i64 = 3;

/// A key or event from the terminal (curses `getch` in the Python version).
#[derive(Clone, Copy, PartialEq, Debug)]
enum Key {
    F12,
    Resize,
    Char(char),
    Up,
    Down,
    Enter,
    Tab,
    Esc,
    Backspace,
    Click(i64),
    Other,
}

/// `threading.Event`: the worker waits on it, the UI sets it.
struct Wake {
    flag: Mutex<bool>,
    cond: Condvar,
}

impl Wake {
    fn set(&self) {
        *self.flag.lock().unwrap() = true;
        self.cond.notify_all();
    }

    /// Wait until set or until the timeout, then clear.
    fn wait_clear(&self, timeout: Duration) {
        let guard = self.flag.lock().unwrap();
        let (mut guard, _) = self.cond.wait_timeout_while(guard, timeout, |set| !*set).unwrap();
        *guard = false;
    }
}

/// State the worker thread reads, and the data it hands back.
#[derive(Default)]
struct Shared {
    home: Option<String>,
    only_home: bool,
    order: Vec<String>,
    generation: String,
    visible: bool,
    /// (generation at the start of the collection, agents)
    pending: Option<(String, Vec<Agent>)>,
}

/// Result of `poll_state`.
#[derive(PartialEq)]
enum Poll {
    Exit,
    Changed,
    Same,
}

pub struct Ui {
    me: String,
    sel: usize,
    scroll: i64,
    /// all agents from the worker
    all: Vec<Agent>,
    /// the agents shown: `all` after the state filter and the search
    agents: Vec<Agent>,
    /// state filter (w/d/b): only agents with this status
    state_filter: Option<&'static str>,
    /// search text (/): name, window, session, branch or path contains it
    query: String,
    /// the search line takes the keys
    searching: bool,
    /// pane -> status at the last check, to find changes that play a sound
    sound_prev: HashMap<String, String>,
    /// @agent_sidebar_sound (on unless "off"), and the files for waiting and done
    sound_on: bool,
    sound_waiting: String,
    sound_done: String,
    /// (y_start, y_end, index) for mouse clicks
    rows: Vec<(i64, i64, usize)>,
    /// session this pane is in
    own: Option<String>,
    /// home session: at the top of the list by default
    home: Option<String>,
    only_home: bool,
    /// session order set with J/K, shared by all sidebars
    order: Vec<String>,
    /// changes when J/K swaps windows: collect again
    generation: String,
    /// window of this sidebar
    window: String,
    shared_sel: String,
    visible: bool,
    /// agent data arrived at least once
    loaded: bool,
    last_poll: Instant,
    drawn_sel: Option<String>,
    /// pane -> last live state seen by refresh_live
    live_prev: HashMap<String, String>,
    /// pane -> deadline to find "finished" in the transcript
    finishing: HashMap<String, Instant>,
    last_draw: Instant,
    bg: Option<u8>,
    bg_shown_until: Instant,
    /// 256 colors (curses COLORS >= 256)
    rich: bool,
    /// more than 8 colors (curses COLORS > 8)
    many: bool,
    screen: Screen,
    /// a key read while draining F12s (curses `ungetch`)
    unget: Option<Key>,
    alive: bool,
    shared: Arc<Mutex<Shared>>,
    wake: Arc<Wake>,
}

fn now_secs() -> f64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs_f64()).unwrap_or(0.0)
}

fn len(s: &str) -> i64 {
    s.chars().count() as i64
}

fn worker(shared: Arc<Mutex<Shared>>, wake: Arc<Wake>) {
    loop {
        let visible = shared.lock().unwrap().visible;
        wake.wait_clear(if visible { REFRESH } else { HIDDEN_REFRESH });
        let (home, only, order, generation) = {
            let s = shared.lock().unwrap();
            (s.home.clone(), s.only_home, s.order.clone(), s.generation.clone())
        };
        let mut agents = collect_agents(home.as_deref(), &order);
        if only {
            agents.retain(|a| Some(&a.session) == home.as_ref());
        }
        shared.lock().unwrap().pending = Some((generation, agents));
    }
}

impl Ui {
    pub fn new() -> Self {
        let term = std::env::var("TERM").unwrap_or_default();
        let rich = term.contains("256color") || term.contains("direct");
        let mut ui = Ui {
            me: std::env::var("TMUX_PANE").unwrap_or_default(),
            sel: 0,
            scroll: 0,
            all: Vec::new(),
            agents: Vec::new(),
            state_filter: None,
            query: String::new(),
            searching: false,
            sound_prev: HashMap::new(),
            sound_on: true,
            sound_waiting: String::new(),
            sound_done: String::new(),
            rows: Vec::new(),
            own: None,
            home: None,
            only_home: false,
            order: Vec::new(),
            generation: String::new(),
            window: String::new(),
            shared_sel: String::new(),
            visible: true,
            loaded: false,
            last_poll: Instant::now(),
            drawn_sel: None,
            live_prev: HashMap::new(),
            finishing: HashMap::new(),
            last_draw: Instant::now(),
            bg: None,
            bg_shown_until: Instant::now(),
            rich,
            many: rich || term.contains("16color"),
            screen: Screen::new(),
            unget: None,
            alive: true,
            shared: Arc::new(Mutex::new(Shared { visible: true, ..Default::default() })),
            wake: Arc::new(Wake { flag: Mutex::new(false), cond: Condvar::new() }),
        };
        ui.set_bg(SEL_BG);
        ui.alive = ui.poll_state() != Poll::Exit;
        ui.wake.set();
        let (shared, wake) = (ui.shared.clone(), ui.wake.clone());
        std::thread::spawn(move || worker(shared, wake));
        ui
    }

    fn set_bg(&mut self, bg: u8) {
        if Some(bg) != self.bg {
            self.bg = Some(bg);
        }
    }

    /// The color of a palette entry (curses color number).
    fn palette(&self, col: Col) -> Color {
        match col {
            Col::Text => Color::Default,
            Col::Red => Color::Idx(1),
            Col::Green => Color::Idx(2),
            Col::Yellow => Color::Idx(3),
            Col::Blue => Color::Idx(4),
            Col::Magenta => Color::Idx(5),
            Col::Cyan => Color::Idx(6),
            Col::Gray => Color::Idx(if self.many { 8 } else { 7 }),
        }
    }

    /// Style of a color, normal or on the selection background.
    fn style(&self, col: Col, selected: bool) -> Style {
        if !selected {
            return Style { fg: self.palette(col), bg: Color::Default, bold: false };
        }
        // dark gray text disappears on a colored selection background
        let fg = if col == Col::Gray && self.rich { Color::Idx(252) } else { self.palette(col) };
        let bg = if self.rich { Color::Idx(self.bg.unwrap_or(SEL_BG)) } else { Color::Idx(0) };
        Style { fg, bg, bold: false }
    }

    fn sync_shared(&self) {
        let mut s = self.shared.lock().unwrap();
        s.home = self.home.clone();
        s.only_home = self.only_home;
        s.order = self.order.clone();
        s.generation = self.generation.clone();
        s.visible = self.visible;
    }

    // --- shared state in tmux options

    /// Read visibility and shared state from tmux.
    fn poll_state(&mut self) -> Poll {
        let result = self.poll_state_inner();
        self.sync_shared();
        result
    }

    fn poll_state_inner(&mut self) -> Poll {
        self.last_poll = Instant::now();
        let fields = [
            "#{@agent_sidebar_on}", "#{@agent_sidebar_sel}", "#{@agent_sidebar_only}",
            "#{@agent_sidebar_home}", "#{session_name}", "#{window_panes}",
            // visible = a client shows this window. Not session_attached: tmux
            // updates that one lazily, so it is stale right after switch-client.
            "#{?window_active_clients,1,0}", "#{pane_width}",
            "#{@agent_sidebar_bg}", "#{P:#{?pane_active,#{pane_id},}}", "#{window_id}",
            "#{@agent_sidebar_focus}", "#{@agent_sidebar_order}", "#{@agent_sidebar_gen}",
            "#{@agent_sidebar_sound}", "#{@agent_sidebar_sound_waiting}",
            "#{@agent_sidebar_sound_done}",
        ];
        let out = tmux!["display-message", "-p", "-t", self.me, fields.join(SEP)];
        let out = out.trim_end_matches('\n');
        let parts: Vec<&str> = out.split(SEP).collect();
        let [on, sel, only, home, own, panes, visible, width, bg, active, window, focus, order,
             generation, sound, sound_waiting, sound_done] = parts[..]
        else {
            return Poll::Exit;
        };
        if on != "1" {
            return Poll::Exit;
        }
        self.sound_on = sound != "off";
        let or = |v: &str, default: &str| if v.is_empty() { default.to_string() } else { v.to_string() };
        self.sound_waiting = or(sound_waiting, crate::sound::DEFAULT_WAITING);
        self.sound_done = or(sound_done, crate::sound::DEFAULT_DONE);
        self.window = window.to_string();
        if panes == "1" {
            return Poll::Exit; // alone in the window (the agent exited)
        }
        if visible == "1" && width != SIDEBAR_WIDTH.to_string() {
            // tmux scales panes proportionally when the window is resized
            tmux!["resize-pane", "-t", self.me, "-x", SIDEBAR_WIDTH.to_string()];
        }
        let mut changed = false;
        let order: Vec<String> =
            order.split(ORDER_SEP).filter(|s| !s.is_empty()).map(str::to_string).collect();
        if order != self.order {
            self.order = order;
            sort_agents(&mut self.all, self.home.as_deref(), &self.order);
            self.apply_filter();
            self.wake.set();
            changed = true;
        }
        if generation != self.generation {
            self.generation = generation.to_string();
            self.wake.set(); // windows were swapped
        }
        if !bg.is_empty() && bg.bytes().all(|b| b.is_ascii_digit())
            && let Ok(n) = bg.parse::<u8>()
                && Some(n) != self.bg {
                    self.set_bg(n);
                    self.bg_shown_until = Instant::now() + Duration::from_secs(3);
                    changed = true;
                }
        let home = if home.is_empty() { own } else { home };
        if (only == "1") != self.only_home || Some(home) != self.home.as_deref() {
            self.wake.set(); // the agent list itself changes
            changed = true;
        }
        changed |= sel != self.shared_sel
            || Some(own) != self.own.as_deref()
            || (visible == "1") != self.visible;
        self.shared_sel = sel.to_string();
        self.only_home = only == "1";
        self.home = Some(home.to_string());
        self.own = Some(own.to_string());
        self.visible = visible == "1";
        // The selection follows you when you move with tmux keys or scripts.
        // This compares states, not events: a window+pane pair that differs from
        // the last one handled is always handled, however fast you switch.
        let key = format!("{window}:{active}");
        if self.visible && self.loaded && key != focus {
            self.follow(window, active, &key);
            changed = true;
        }
        if changed { Poll::Changed } else { Poll::Same }
    }

    /// Focus in an agent pane -> select that agent. Otherwise (focus in the
    /// sidebar) select an agent of this window, unless one is selected already.
    fn follow(&mut self, window: &str, active: &str, key: &str) {
        let mine: Vec<&str> = self
            .all
            .iter()
            .filter(|a| a.window_id == window)
            .map(|a| a.pane.as_str())
            .collect();
        let mut target = self.shared_sel.clone();
        if mine.contains(&active) {
            target = active.to_string();
        } else if !mine.is_empty() && !mine.contains(&self.shared_sel.as_str()) {
            target = mine[0].to_string();
        }
        self.shared_sel = target.clone();
        tmux!["set-option", "-g", "@agent_sidebar_sel", target, ";",
              "set-option", "-g", "@agent_sidebar_focus", key];
    }

    fn select(&mut self, i: i64) {
        if 0 <= i && (i as usize) < self.agents.len() {
            self.sel = i as usize;
            self.shared_sel = self.agents[self.sel].pane.clone();
            // store it and "poke" (F12) the other sidebars right away, so after a
            // window switch they never show the old selection for a moment
            let cmd = vec!["set-option".into(), "-g".into(), "@agent_sidebar_sel".into(),
                           self.shared_sel.clone()];
            self.poke_others(cmd);
        }
    }

    /// J/K: swap the selected agent with the next (+1) or previous (-1)
    /// agent of its session. The tmux windows swap, so the window numbers
    /// change too. At the edge of a session, move the whole session.
    fn move_agent(&mut self, delta: i64) {
        if self.agents.is_empty() {
            return;
        }
        let j = self.sel as i64 + delta;
        let a = self.agents[self.sel].clone();
        let b = (0 <= j && (j as usize) < self.agents.len()).then(|| self.agents[j as usize].clone());
        let Some(b) = b.filter(|b| b.session == a.session) else {
            self.move_session(delta);
            return;
        };
        let mut cmd: Vec<String>;
        if a.window_id == b.window_id {
            cmd = vec!["swap-pane".into(), "-d".into(), "-s".into(), a.pane.clone(), "-t".into(),
                       b.pane.clone()];
            for x in &mut self.all {
                if x.pane == a.pane {
                    x.order = b.order;
                } else if x.pane == b.pane {
                    x.order = a.order;
                }
            }
        } else {
            // -d and select-window: you keep looking at the same window
            cmd = vec!["swap-window".into(), "-d".into(), "-s".into(), a.window_id.clone(),
                       "-t".into(), b.window_id.clone(), ";".into(), "select-window".into(),
                       "-t".into(), self.window.clone()];
            let (wa, wb) = (a.order.0, b.order.0);
            for x in &mut self.all {
                if x.window_id == a.window_id {
                    x.order = (wb, x.order.1);
                } else if x.window_id == b.window_id {
                    x.order = (wa, x.order.1);
                }
            }
        }
        sort_agents(&mut self.all, self.home.as_deref(), &self.order);
        self.apply_filter();
        self.sync_sel(); // the selection stays on the same agent
        // the other sidebars collect the new order at once (@agent_sidebar_gen)
        let ns = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
        self.generation = ns.to_string();
        self.sync_shared();
        cmd.extend([";".into(), "set-option".into(), "-g".into(), "@agent_sidebar_gen".into(),
                    self.generation.clone()]);
        self.poke_others(cmd);
    }

    /// Run a tmux command and send F12 to the other sidebars, so they
    /// check the shared state now.
    fn poke_others(&self, mut cmd: Vec<String>) {
        for pane in sidebar_panes() {
            if pane != self.me {
                cmd.extend([";".into(), "send-keys".into(), "-t".into(), pane, "F12".into()]);
            }
        }
        tmux::tmux(&cmd);
    }

    /// Move the session of the selected agent down (+1) or up (-1).
    fn move_session(&mut self, delta: i64) {
        if self.agents.is_empty() || self.only_home {
            return;
        }
        let mut sessions: Vec<String> = Vec::new();
        for a in &self.agents {
            if !sessions.contains(&a.session) {
                sessions.push(a.session.clone());
            }
        }
        let Some(i) = sessions.iter().position(|s| *s == self.agents[self.sel].session) else {
            return;
        };
        let j = i as i64 + delta;
        if !(0 <= j && (j as usize) < sessions.len()) {
            return;
        }
        sessions.swap(i, j as usize);
        // keep the sessions the filter hides now in the order too
        for a in &self.all {
            if !sessions.contains(&a.session) {
                sessions.push(a.session.clone());
            }
        }
        self.order = sessions.clone();
        sort_agents(&mut self.all, self.home.as_deref(), &sessions);
        self.apply_filter();
        self.sync_sel(); // the selection stays on the same agent
        self.sync_shared();
        self.poke_others(vec!["set-option".into(), "-g".into(), "@agent_sidebar_order".into(),
                              sessions.join(ORDER_SEP)]);
    }

    fn sync_sel(&mut self) {
        self.sel = match self.agents.iter().position(|a| a.pane == self.shared_sel) {
            Some(i) => i,
            None => self.sel.min(self.agents.len().saturating_sub(1)),
        };
    }

    /// Apply Claude Code's live state at once (every tick). It only stats
    /// a few small files, so it is cheap. True when a status changed.
    fn refresh_live(&mut self) -> bool {
        let mut changed = false;
        let now = Instant::now();
        for i in 0..self.all.len() {
            let Some(info) = self.all[i].pid.and_then(session_info) else {
                continue;
            };
            let live = state(&info).to_string();
            let pane = self.all[i].pane.clone();
            let prev = self.live_prev.insert(pane.clone(), live.clone());
            if live != "idle" {
                self.finishing.remove(&pane);
            } else if prev.as_deref() == Some("busy") && self.visible {
                // The turn ended: finished, or you stopped it (Esc, Ctrl+C).
                // The transcript tells which. It can lag a moment, so look
                // again for up to FINISH_WAIT.
                self.finishing.insert(pane.clone(), now + FINISH_WAIT);
            }
            if let Some(deadline) = self.finishing.get(&pane).copied() {
                if transcript_info(self.all[i].transcript.as_deref()).finished {
                    self.finishing.remove(&pane);
                    self.mark_finished(i);
                } else if now > deadline {
                    self.finishing.remove(&pane); // stopped by you: stays idle
                }
            }
            let new = resolve_status(&self.all[i].hook, &live);
            if new != self.all[i].status {
                self.all[i].status = new.to_string();
                changed = true;
            }
        }
        if changed {
            self.apply_filter(); // a status change can move an agent in or out of the filter
        }
        changed
    }

    /// Play a sound when an agent turns waiting (red) or done (green). Only the
    /// visible sidebar plays, and not for the agent you look at.
    fn sound_changes(&mut self) {
        let mut play: Option<String> = None;
        for a in &self.all {
            let prev = self.sound_prev.insert(a.pane.clone(), a.status.clone());
            let Some(prev) = prev else { continue }; // first sight: no sound
            if prev == a.status || !self.visible || !self.sound_on {
                continue;
            }
            let file = match a.status.as_str() {
                "waiting" => &self.sound_waiting,
                "done" => &self.sound_done,
                _ => continue,
            };
            let seen = tmux!["display-message", "-p", "-t", a.pane,
                             "#{&&:#{pane_active},#{window_active_clients}}"].trim() == "1";
            if !seen && play.is_none() {
                play = Some(file.clone());
            }
        }
        if let Some(file) = play {
            crate::sound::play(&file);
        }
    }

    /// Green if you did not look at the agent when it finished. The Stop
    /// hook does the same, but no hook runs for a background session.
    fn mark_finished(&mut self, i: usize) {
        let pane = self.all[i].pane.clone();
        let seen = tmux!["display-message", "-p", "-t", pane,
                         "#{&&:#{pane_active},#{window_active_clients}}"].trim() == "1";
        let hook = if seen { "idle" } else { "done" };
        self.all[i].hook = hook.to_string();
        let ts = (now_secs() as i64).to_string();
        tmux!["set-option", "-p", "-t", pane, "@agent_status", hook, ";",
              "set-option", "-p", "-t", pane, "@agent_ts", ts];
    }

    /// Swap in new agent data from the worker. True if there was some.
    fn take_data(&mut self) -> bool {
        let Some((generation, mut agents)) = self.shared.lock().unwrap().pending.take() else {
            return false;
        };
        if generation != self.generation {
            return false; // collected before J/K swapped windows: wait for new data
        }
        // the worker may have sorted with an older order (J/K just now)
        sort_agents(&mut agents, self.home.as_deref(), &self.order);
        self.all = agents;
        self.apply_filter();
        self.loaded = true;
        true
    }

    // --- filter and search

    fn matches(&self, a: &Agent) -> bool {
        if self.state_filter.is_some_and(|st| a.status != st) {
            return false;
        }
        if self.query.is_empty() {
            return true;
        }
        let q = self.query.to_lowercase();
        let git = a.git.as_ref();
        [a.label(), &a.window, &a.session, &a.cwd,
         git.map_or("", |g| g.branch.as_str()), git.map_or("", |g| g.top.as_str())]
            .iter()
            .any(|field| field.to_lowercase().contains(&q))
    }

    /// Rebuild the shown list from all agents.
    fn apply_filter(&mut self) {
        self.agents = self.all.iter().filter(|a| self.matches(a)).cloned().collect();
    }

    fn set_state_filter(&mut self, st: &'static str) {
        self.state_filter = if self.state_filter == Some(st) { None } else { Some(st) };
        self.scroll = 0;
        self.apply_filter();
    }

    fn clear_filter(&mut self) {
        self.state_filter = None;
        self.query.clear();
        self.searching = false;
        self.apply_filter();
    }

    /// Keys while the search line is open. True when the key was used.
    fn search_key(&mut self, k: Key) -> bool {
        match k {
            Key::Char(c) => self.query.push(c),
            Key::Backspace => {
                self.query.pop();
            }
            Key::Enter => self.searching = false, // keep the filter
            Key::Esc => {
                self.query.clear();
                self.searching = false;
            }
            _ => return false, // arrows, Tab, clicks: normal handling
        }
        self.scroll = 0;
        self.apply_filter();
        true
    }

    // --- drawing

    fn put(&mut self, y: i64, x: i64, text: &str, style: Style) {
        self.screen.put(y, x, text, style);
    }

    fn counts_label(agents: &[&Agent]) -> Vec<(String, Col)> {
        ["waiting", "done", "working"]
            .iter()
            .filter_map(|st| {
                let n = agents.iter().filter(|a| a.status == *st).count();
                let def = status_def(st)?;
                (n > 0).then(|| (format!("{}{n}", def.symbol), def.color))
            })
            .collect()
    }

    /// Virtual rows: (y, header session) or (y, agent index).
    fn layout(&self) -> Vec<(i64, Result<usize, String>)> {
        let mut items = Vec::new();
        let mut y = 0;
        let mut prev: Option<&str> = None;
        for (i, a) in self.agents.iter().enumerate() {
            if prev != Some(a.session.as_str()) {
                if prev.is_some() {
                    y += 1;
                }
                items.push((y, Err(a.session.clone())));
                y += 1;
                prev = Some(&a.session);
            }
            items.push((y, Ok(i)));
            y += LINES_PER_AGENT + 1;
        }
        items
    }

    fn draw(&mut self) {
        self.last_draw = Instant::now();
        let (cols, lines) = crossterm::terminal::size().unwrap_or((80, 24));
        self.screen.erase(cols as usize, lines as usize);
        let (h, w) = self.screen.size();
        let tx = t();
        self.put(0, 1, tx.title, Style::DEFAULT.bold());
        let mut x = len(tx.title) + 2;
        // the counts cover all agents, also the ones a filter hides
        let all: Vec<&Agent> = self.all.iter().collect();
        for (label, col) in Self::counts_label(&all) {
            let st = self.style(col, false).bold();
            self.put(0, x, &label, st);
            x += len(&label) + 1;
        }
        if Instant::now() < self.bg_shown_until {
            let label = format!("bg {}", self.bg.unwrap_or(SEL_BG));
            let st = self.style(Col::Text, true).bold();
            self.put(0, w - len(&label) - 2, &label, st);
        } else {
            // active filters: [home  state  /search]
            let mut parts: Vec<String> = Vec::new();
            if let (true, Some(home)) = (self.only_home, self.home.clone().filter(|h| !h.is_empty())) {
                parts.push(home);
            }
            if let Some(st) = self.state_filter {
                parts.push(tx.filter_name(st).to_string());
            }
            if !self.query.is_empty() && !self.searching {
                parts.push(format!("/{}", self.query));
            }
            if !parts.is_empty() {
                let label = format!("[{}]", parts.join(" "));
                let st = self.style(Col::Cyan, false);
                self.put(0, (x + 1).max(w - len(&label) - 2), &label, st);
            }
        }
        let gray = self.style(Col::Gray, false);
        self.put(1, 0, &"─".repeat((w - 1).max(0) as usize), gray);

        self.rows.clear();
        let (top, avail) = (2, (h - 3).max(1));
        if self.agents.is_empty() {
            let text = if self.all.is_empty() { tx.no_agents } else { tx.no_match };
            self.put(top, 1, text, gray);
        }
        let items = self.layout();
        // scrolling: keep the selected agent (and its session header) in view
        let per = LINES_PER_AGENT + 1;
        for (idx, (y, item)) in items.iter().enumerate() {
            if *item == Ok(self.sel) {
                let y0 = match idx.checked_sub(1).map(|p| &items[p]) {
                    Some((hy, Err(_))) => *hy,
                    _ => *y,
                };
                if y0 < self.scroll {
                    self.scroll = y0;
                } else if y + per > self.scroll + avail {
                    self.scroll = y + per - avail;
                }
            }
        }
        for (y, item) in &items {
            let sy = y - self.scroll + top;
            if sy < top || sy >= h - 1 {
                continue;
            }
            match item {
                Err(session) => self.draw_header(sy, session, w),
                Ok(i) => {
                    self.draw_agent(sy, *i, w);
                    self.rows.push((sy, sy + per - 1, *i));
                }
            }
        }
        if self.searching {
            let line = format!("/{}▏", self.query);
            self.put(h - 1, 1, &line, Style::DEFAULT.bold());
        } else {
            self.put(h - 1, 1, tx.help, gray);
        }
        let _ = self.screen.flush(&mut std::io::stdout().lock());
        let sel = self.agents.get(self.sel).map(|a| a.pane.clone()).unwrap_or_default();
        if Some(&sel) != self.drawn_sel.as_ref() {
            // jump() waits for this before it shows the window of this sidebar
            tmux!["set-option", "-p", "-t", self.me, "@agent_sidebar_drawn", sel];
            self.drawn_sel = Some(sel);
        }
    }

    fn draw_header(&mut self, y: i64, session: &str, w: i64) {
        let cyan = self.style(Col::Cyan, false);
        let head = format!("━━ {session} ");
        self.put(y, 0, &head, cyan.bold());
        let mut x = len(&head);
        let mine: Vec<&Agent> = self.agents.iter().filter(|a| a.session == session).collect();
        for (label, col) in Self::counts_label(&mine) {
            let st = self.style(col, false).bold();
            self.put(y, x, &format!("{label} "), st);
            x += len(&label) + 1;
        }
        self.put(y, x, &"━".repeat((w - x - 1).max(0) as usize), cyan);
    }

    fn draw_agent(&mut self, y: i64, i: usize, w: i64) {
        let a = self.agents[i].clone();
        let selected = i == self.sel;
        let c = |ui: &Ui, col: Col| ui.style(col, selected);
        if selected {
            for dy in 0..LINES_PER_AGENT {
                let st = c(self, Col::Text);
                self.put(y + dy, 0, &" ".repeat((w - 1).max(0) as usize), st);
            }
        }
        let def = status_def(&a.status).unwrap_or_else(|| status_def("unknown").unwrap());
        let mut sym = def.symbol;
        if a.status == "working" {
            sym = SPINNER[(now_secs() * 2.0) as usize % SPINNER.len()];
        }
        let st = c(self, def.color).bold();
        self.put(y, 1, sym, st);
        let st = c(self, Col::Text).bold();
        self.put(y, 2, &format!(" {} {}", i + 1, a.label()), st);
        let tokens = fmt_tokens(a.tokens);
        if !tokens.is_empty() {
            let st = c(self, Col::Gray);
            self.put(y, w - len(&tokens) - 2, &tokens, st);
        }
        match &a.git {
            Some(g) => {
                let st = c(self, Col::Magenta);
                let branch = format!("⎇ {}", g.branch);
                self.put(y + 1, 3, &branch, st);
                // ahead / behind the upstream, changed files
                let mut x = 3 + len(&branch) + 1;
                let parts = [("↑", g.status.ahead, Col::Green), ("↓", g.status.behind, Col::Red),
                             ("●", g.status.changes, Col::Yellow)];
                for (sym, n, col) in parts {
                    if n > 0 {
                        let label = format!("{sym}{n}");
                        let st = c(self, col);
                        self.put(y + 1, x, &label, st);
                        x += len(&label) + 1;
                    }
                }
                let tag = if g.linked_worktree { " [wt]" } else { "" };
                let path = git::short_path(&g.top, Some(w - 7 - len(tag)));
                let icon = if g.linked_worktree { "⊕ " } else { "⌂ " };
                let st = c(self, Col::Blue);
                self.put(y + 2, 3, &format!("{icon}{path}{tag}"), st);
            }
            None => {
                let st = c(self, Col::Gray);
                self.put(y + 1, 3, t().no_git, st);
                let st = c(self, Col::Blue);
                self.put(y + 2, 3, &format!("⌂ {}", git::short_path(&a.cwd, Some(w - 7))), st);
            }
        }
        let gray = self.style(Col::Gray, false);
        self.put(y + 3, 1, &"┄".repeat((w - 3).max(0) as usize), gray);
    }

    // --- actions

    fn go(&mut self, i: i64, focus: bool) {
        if 0 <= i && (i as usize) < self.agents.len() {
            self.select(i);
            jump(&mut self.agents[i as usize], focus);
            // jump marks a "done" agent as seen
            let a = &self.agents[i as usize];
            if let Some(x) = self.all.iter_mut().find(|x| x.pane == a.pane) {
                x.status = a.status.clone();
                x.hook = a.hook.clone();
            }
        }
    }

    fn next_waiting(&mut self) {
        let n = self.agents.len();
        for off in 1..=n {
            let i = (self.sel + off) % n;
            if self.agents[i].status == "waiting" || self.agents[i].status == "done" {
                self.select(i as i64);
                return;
            }
        }
    }

    fn rename(&self) {
        let Some(a) = self.agents.get(self.sel) else { return };
        tmux::tmux_bg(&["command-prompt".into(), "-I".into(), a.label().to_string(), "-p".into(),
                        t().rename.to_string(),
                        format!("set-option -p -t {} @agent_name '%%'", a.pane)]);
    }

    /// The client that shows this sidebar's window (prompts and menus go there).
    fn client(&self) -> String {
        tmux!["list-clients", "-F", "#{client_name} #{window_id}"]
            .lines()
            .filter_map(|l| l.split_once(' '))
            .find(|(_, w)| *w == self.window)
            .map(|(c, _)| c.to_string())
            .unwrap_or_default()
    }

    /// W: ask for a branch, add a worktree for it, start claude there.
    fn worktree_new(&self) {
        let (Some(a), client) = (self.agents.get(self.sel), self.client()) else { return };
        if a.git.is_none() {
            return;
        }
        let run = worktree::self_cmd(&["worktree-new", &client, &a.pane]);
        let template = format!("set-option -g {} \"%%%\" ; run-shell -b {}",
                               worktree::INPUT_OPT, worktree::quote(&run));
        tmux::tmux_bg(&["command-prompt", "-t", &client, "-p", t().wt_branch, &template]);
    }

    /// O: menu of the repo's worktrees. A worktree with an agent shows it,
    /// one without gets a new window with claude.
    fn worktree_menu(&self) {
        let (Some(a), client) = (self.agents.get(self.sel), self.client()) else { return };
        let items = worktree::list(&a.cwd);
        if items.is_empty() {
            return;
        }
        let mut args: Vec<String> = vec!["display-menu".into(), "-c".into(), client.clone(),
                                         "-T".into(), t().wt_menu.into(), "-x".into(), "P".into(),
                                         "-y".into(), "P".into()];
        for (i, (path, branch)) in items.iter().enumerate() {
            let key = if i < 9 { (i + 1).to_string() } else { String::new() };
            let label = format!("{branch}  {}", git::short_path(path, None)).replace('#', "##");
            let run = worktree::self_cmd(&["worktree-open", &client, &a.pane, path]);
            args.extend([label, key, format!("run-shell -b {}", worktree::quote(&run))]);
        }
        tmux::tmux_bg(&args);
    }

    /// D: remove the linked worktree of the selected agent (asks first).
    fn worktree_remove(&self) {
        let (Some(a), client) = (self.agents.get(self.sel), self.client()) else { return };
        let Some(g) = a.git.as_ref().filter(|g| g.linked_worktree) else {
            tmux!["display-message", "-c", client, t().wt_not_linked];
            return;
        };
        let run = worktree::self_cmd(&["worktree-remove", &client, &a.pane, &g.top]);
        let prompt = format!("{} {} (y/n)", t().wt_remove, git::short_path(&g.top, None));
        tmux::tmux_bg(&["confirm-before".into(), "-t".into(), client, "-p".into(), prompt,
                        format!("run-shell -b {}", worktree::quote(&run))]);
    }

    fn handle_key(&mut self, k: Key) {
        if self.searching && self.search_key(k) {
            return;
        }
        let n = self.agents.len() as i64;
        match k {
            Key::Char('j') | Key::Down => self.select((self.sel as i64 + 1).min(n - 1)),
            Key::Char('k') | Key::Up => self.select((self.sel as i64 - 1).max(0)),
            Key::Char('g') => self.select(0),
            Key::Char('G') => self.select(n - 1),
            Key::Char('J') => self.move_agent(1),
            Key::Char('K') => self.move_agent(-1),
            Key::Enter | Key::Char('o') => self.go(self.sel as i64, false),
            Key::Char('i') => self.go(self.sel as i64, true),
            Key::Char(c @ '1'..='9') => self.go(c as i64 - '1' as i64, false),
            Key::Tab => self.next_waiting(),
            Key::Char('s') => {
                tmux!["set-option", "-g", "@agent_sidebar_only", if self.only_home { "" } else { "1" }];
                self.scroll = 0;
            }
            Key::Char('n') => self.rename(),
            Key::Char('c') => {
                let i = match self.bg.and_then(|bg| BG_CHOICES.iter().position(|c| *c == bg)) {
                    Some(i) => i + 1,
                    None => 0,
                };
                let bg = BG_CHOICES[i % BG_CHOICES.len()];
                tmux!["set-option", "-g", "@agent_sidebar_bg", bg.to_string()];
                // the other sidebars read the new color on their next poll
                self.set_bg(bg);
                self.bg_shown_until = Instant::now() + Duration::from_secs(3);
            }
            Key::Char('r') => {
                git::clear_cache();
                self.wake.set();
            }
            Key::Char('/') => {
                self.searching = true;
                self.query.clear();
                self.apply_filter();
            }
            Key::Char('w') => self.set_state_filter("waiting"),
            Key::Char('d') => self.set_state_filter("done"),
            Key::Char('b') => self.set_state_filter("working"),
            Key::Char('a') | Key::Esc => self.clear_filter(),
            Key::Char('W') => self.worktree_new(),
            Key::Char('O') => self.worktree_menu(),
            Key::Char('D') => self.worktree_remove(),
            Key::Click(my) => {
                let rows = self.rows.clone();
                for (y0, y1, i) in rows {
                    if y0 <= my && my <= y1 {
                        self.go(i as i64, false);
                    }
                }
            }
            _ => {}
        }
    }

    /// One terminal event, like curses `getch` with a timeout: None after
    /// the timeout or for events that are not keys.
    fn getch(&mut self, timeout: Duration) -> Option<Key> {
        if let Some(k) = self.unget.take() {
            return Some(k);
        }
        if !event::poll(timeout).ok()? {
            return None;
        }
        match event::read().ok()? {
            Event::Key(k) if k.kind != KeyEventKind::Release => {
                let plain = !k.modifiers.intersects(KeyModifiers::CONTROL | KeyModifiers::ALT);
                Some(match k.code {
                    KeyCode::F(12) => Key::F12,
                    KeyCode::Up => Key::Up,
                    KeyCode::Down => Key::Down,
                    KeyCode::Enter => Key::Enter,
                    // Ctrl+J is a newline (10): curses treats it like Enter
                    KeyCode::Char('j') if k.modifiers == KeyModifiers::CONTROL => Key::Enter,
                    KeyCode::Tab => Key::Tab,
                    KeyCode::Esc => Key::Esc,
                    KeyCode::Backspace => Key::Backspace,
                    KeyCode::Char(c) if plain => Key::Char(c),
                    _ => Key::Other,
                })
            }
            Event::Mouse(m) => match m.kind {
                MouseEventKind::Down(MouseButton::Left) => Some(Key::Click(m.row as i64)),
                _ => None,
            },
            Event::Resize(_, _) => Some(Key::Resize),
            _ => None,
        }
    }

    pub fn run(&mut self) {
        while self.alive {
            let mut k = self.getch(TICK);
            let mut dirty = false;
            if k == Some(Key::F12) {
                // a tmux hook (window switch) or another sidebar: check state now;
                // drain queued F12s so a burst of switches costs one check
                loop {
                    match self.getch(Duration::ZERO) {
                        Some(Key::F12) => continue,
                        Some(other) => {
                            self.unget = Some(other);
                            break;
                        }
                        None => break,
                    }
                }
                k = Some(Key::F12);
            } else if let Some(key) = k.filter(|k| *k != Key::Resize) {
                self.handle_key(key);
                dirty = true;
            } else if k == Some(Key::Resize) {
                self.screen.invalidate();
                dirty = true;
            }
            let poll_every = if self.visible { POLL } else { HIDDEN_POLL };
            if k == Some(Key::F12) || dirty || self.last_poll.elapsed() >= poll_every {
                match self.poll_state() {
                    Poll::Exit => return,
                    Poll::Changed => dirty = true,
                    Poll::Same => {}
                }
            }
            if self.take_data() {
                // new data may make a pending focus-follow possible
                if self.poll_state() == Poll::Exit {
                    return;
                }
                dirty = true;
            }
            dirty |= self.refresh_live();
            self.sound_changes();
            // periodic redraw keeps the "working" spinner moving
            if dirty || (self.visible && self.last_draw.elapsed() >= Duration::from_millis(500)) {
                self.sync_sel();
                self.draw();
            }
        }
    }
}

/// Raw mode, alternate screen, no cursor, mouse clicks. Restored on drop.
pub struct Terminal;

impl Terminal {
    pub fn enter() -> std::io::Result<Terminal> {
        crossterm::terminal::enable_raw_mode()?;
        let mut out = std::io::stdout();
        // 1049: alternate screen, 25: hide cursor, 1000 + 1006: mouse buttons (SGR)
        out.write_all(b"\x1b[?1049h\x1b[?25l\x1b[?1000h\x1b[?1006h")?;
        out.flush()?;
        Ok(Terminal)
    }
}

impl Drop for Terminal {
    fn drop(&mut self) {
        let mut out = std::io::stdout();
        let _ = out.write_all(b"\x1b[?1006l\x1b[?1000l\x1b[0m\x1b[?25h\x1b[?1049l");
        let _ = out.flush();
        let _ = crossterm::terminal::disable_raw_mode();
    }
}
