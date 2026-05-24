use std::collections::HashMap;
use std::fs;
use std::io::{Stdout, Write, stdout};
use std::net::UdpSocket;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use crossterm::event::{
    self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, KeyboardEnhancementFlags,
    PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
};
use crossterm::style::{Print, Stylize};
use crossterm::terminal::{
    self, Clear, ClearType, EnterAlternateScreen, LeaveAlternateScreen,
    supports_keyboard_enhancement,
};
use crossterm::{cursor, execute, queue};
use serde::{Deserialize, Serialize};

const DEFAULT_ROVER: &str = "192.168.1.102:4242";
const DEFAULT_PORT: u16 = 4242;
const TICK_HZ: u64 = 10;
const TICK_MS: u64 = 1000 / TICK_HZ;
const SPEED_DEFAULT: i32 = 700;
const SPEED_STEP: i32 = 100;
const SPEED_MIN: i32 = 100;
const SPEED_MAX: i32 = 1000;

// ============================================================================
// Action / key binding
// ============================================================================

#[derive(Copy, Clone, Eq, PartialEq, Hash, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum Action {
    Forward,
    Backward,
    StrafeLeft,
    StrafeRight,
    RotateCcw,
    RotateCw,
    Stop,
    SpeedUp,
    SpeedDown,
    Quit,
}

impl Action {
    const ALL: [Action; 10] = [
        Action::Forward,
        Action::Backward,
        Action::StrafeLeft,
        Action::StrafeRight,
        Action::RotateCcw,
        Action::RotateCw,
        Action::Stop,
        Action::SpeedUp,
        Action::SpeedDown,
        Action::Quit,
    ];

    fn label(self) -> &'static str {
        match self {
            Action::Forward => "forward",
            Action::Backward => "backward",
            Action::StrafeLeft => "strafe left",
            Action::StrafeRight => "strafe right",
            Action::RotateCcw => "rotate CCW",
            Action::RotateCw => "rotate CW",
            Action::Stop => "stop",
            Action::SpeedUp => "speed up",
            Action::SpeedDown => "speed down",
            Action::Quit => "quit",
        }
    }
}

#[derive(Default, Serialize, Deserialize)]
#[serde(default)]
struct ConfigFile {
    target: Option<String>,
    forward: Option<String>,
    backward: Option<String>,
    strafe_left: Option<String>,
    strafe_right: Option<String>,
    rotate_ccw: Option<String>,
    rotate_cw: Option<String>,
    stop: Option<String>,
    speed_up: Option<String>,
    speed_down: Option<String>,
    quit: Option<String>,
}

struct Keymap {
    bindings: HashMap<Action, KeyCode>,
}

impl Keymap {
    fn defaults() -> Self {
        let mut bindings = HashMap::with_capacity(Action::ALL.len());
        bindings.insert(Action::Forward, KeyCode::Char('w'));
        bindings.insert(Action::Backward, KeyCode::Char('s'));
        bindings.insert(Action::StrafeLeft, KeyCode::Char('a'));
        bindings.insert(Action::StrafeRight, KeyCode::Char('d'));
        bindings.insert(Action::RotateCcw, KeyCode::Char('q'));
        bindings.insert(Action::RotateCw, KeyCode::Char('e'));
        bindings.insert(Action::Stop, KeyCode::Char(' '));
        bindings.insert(Action::SpeedUp, KeyCode::Char('+'));
        bindings.insert(Action::SpeedDown, KeyCode::Char('-'));
        bindings.insert(Action::Quit, KeyCode::Esc);
        Self { bindings }
    }

    fn get(&self, action: Action) -> KeyCode {
        self.bindings
            .get(&action)
            .copied()
            .unwrap_or(KeyCode::Null)
    }

    fn lookup(&self, code: KeyCode) -> Option<Action> {
        let normalized = normalize(code);
        self.bindings
            .iter()
            .find(|&(_, &k)| normalize(k) == normalized)
            .map(|(a, _)| *a)
    }

    /// Bind `action` to `code`. Any other action previously bound to that
    /// same key is unbound — a key can only do one thing at a time.
    fn rebind(&mut self, action: Action, code: KeyCode) {
        let normalized = normalize(code);
        self.bindings
            .retain(|_, k| normalize(*k) != normalized);
        self.bindings.insert(action, code);
    }
}

/// Letter chars are case-insensitive at lookup time.
fn normalize(code: KeyCode) -> KeyCode {
    match code {
        KeyCode::Char(c) if c.is_ascii_alphabetic() => KeyCode::Char(c.to_ascii_lowercase()),
        other => other,
    }
}

fn key_to_string(code: KeyCode) -> String {
    match code {
        KeyCode::Char(' ') => "space".into(),
        KeyCode::Char(c) => c.to_string(),
        KeyCode::Esc => "esc".into(),
        KeyCode::Enter => "enter".into(),
        KeyCode::Tab => "tab".into(),
        KeyCode::Backspace => "backspace".into(),
        KeyCode::Delete => "delete".into(),
        KeyCode::Up => "↑".into(),
        KeyCode::Down => "↓".into(),
        KeyCode::Left => "←".into(),
        KeyCode::Right => "→".into(),
        KeyCode::Home => "home".into(),
        KeyCode::End => "end".into(),
        KeyCode::PageUp => "pageup".into(),
        KeyCode::PageDown => "pagedown".into(),
        KeyCode::Insert => "insert".into(),
        KeyCode::F(n) => format!("f{n}"),
        KeyCode::Null => "(unbound)".into(),
        _ => format!("{code:?}"),
    }
}

fn string_to_key(s: &str) -> Option<KeyCode> {
    if s.is_empty() {
        return None;
    }
    let lower = s.to_lowercase();
    let named = match lower.as_str() {
        "space" => Some(KeyCode::Char(' ')),
        "esc" | "escape" => Some(KeyCode::Esc),
        "enter" | "return" => Some(KeyCode::Enter),
        "tab" => Some(KeyCode::Tab),
        "backspace" => Some(KeyCode::Backspace),
        "delete" | "del" => Some(KeyCode::Delete),
        "up" | "↑" => Some(KeyCode::Up),
        "down" | "↓" => Some(KeyCode::Down),
        "left" | "←" => Some(KeyCode::Left),
        "right" | "→" => Some(KeyCode::Right),
        "home" => Some(KeyCode::Home),
        "end" => Some(KeyCode::End),
        "pageup" => Some(KeyCode::PageUp),
        "pagedown" => Some(KeyCode::PageDown),
        "insert" => Some(KeyCode::Insert),
        _ => None,
    };
    if named.is_some() {
        return named;
    }
    if let Some(rest) = lower.strip_prefix('f')
        && let Ok(n) = rest.parse::<u8>()
        && (1..=12).contains(&n)
    {
        return Some(KeyCode::F(n));
    }
    let chars: Vec<char> = s.chars().collect();
    if chars.len() == 1 {
        Some(KeyCode::Char(chars[0]))
    } else {
        None
    }
}

fn keymap_from_config(cfg: &ConfigFile) -> Keymap {
    let mut km = Keymap::defaults();
    let pairs: [(Action, Option<&str>); 10] = [
        (Action::Forward, cfg.forward.as_deref()),
        (Action::Backward, cfg.backward.as_deref()),
        (Action::StrafeLeft, cfg.strafe_left.as_deref()),
        (Action::StrafeRight, cfg.strafe_right.as_deref()),
        (Action::RotateCcw, cfg.rotate_ccw.as_deref()),
        (Action::RotateCw, cfg.rotate_cw.as_deref()),
        (Action::Stop, cfg.stop.as_deref()),
        (Action::SpeedUp, cfg.speed_up.as_deref()),
        (Action::SpeedDown, cfg.speed_down.as_deref()),
        (Action::Quit, cfg.quit.as_deref()),
    ];
    for (action, raw) in pairs {
        if let Some(s) = raw
            && let Some(code) = string_to_key(s)
        {
            km.rebind(action, code);
        }
    }
    km
}

fn config_from_state(s: &State) -> ConfigFile {
    ConfigFile {
        target: Some(s.address.clone()),
        forward: Some(key_to_string(s.keymap.get(Action::Forward))),
        backward: Some(key_to_string(s.keymap.get(Action::Backward))),
        strafe_left: Some(key_to_string(s.keymap.get(Action::StrafeLeft))),
        strafe_right: Some(key_to_string(s.keymap.get(Action::StrafeRight))),
        rotate_ccw: Some(key_to_string(s.keymap.get(Action::RotateCcw))),
        rotate_cw: Some(key_to_string(s.keymap.get(Action::RotateCw))),
        stop: Some(key_to_string(s.keymap.get(Action::Stop))),
        speed_up: Some(key_to_string(s.keymap.get(Action::SpeedUp))),
        speed_down: Some(key_to_string(s.keymap.get(Action::SpeedDown))),
        quit: Some(key_to_string(s.keymap.get(Action::Quit))),
    }
}

// ============================================================================
// Config file
// ============================================================================

fn config_path() -> Option<PathBuf> {
    let base = std::env::var("XDG_CONFIG_HOME")
        .ok()
        .map(PathBuf::from)
        .or_else(|| std::env::var("HOME").ok().map(|h| PathBuf::from(h).join(".config")))?;
    Some(base.join("rover_ctl").join("config.toml"))
}

fn load_config() -> ConfigFile {
    config_path()
        .and_then(|p| fs::read_to_string(&p).ok())
        .and_then(|s| toml::from_str(&s).ok())
        .unwrap_or_default()
}

fn save_state(s: &State) {
    let Some(path) = config_path() else { return };
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    if let Ok(toml) = toml::to_string_pretty(&config_from_state(s)) {
        let _ = fs::write(&path, toml);
    }
}

// ============================================================================
// Application state
// ============================================================================

#[derive(Clone, PartialEq, Eq)]
enum Mode {
    Drive,
    Rebind { cursor: usize },
    AwaitKey { action: Action, cursor: usize },
    EditAddress { buffer: String },
}

struct State {
    vx: i32,
    vy: i32,
    omega: i32,
    speed: i32,
    sent: u64,
    quit: bool,
    keymap: Keymap,
    mode: Mode,
    address: String,
    status: Option<String>,
}

impl State {
    fn new(keymap: Keymap, address: String) -> Self {
        Self {
            vx: 0,
            vy: 0,
            omega: 0,
            speed: SPEED_DEFAULT,
            sent: 0,
            quit: false,
            keymap,
            mode: Mode::Drive,
            address,
            status: None,
        }
    }

    fn zero_velocity(&mut self) {
        self.vx = 0;
        self.vy = 0;
        self.omega = 0;
    }
}

// ============================================================================
// Address parsing
// ============================================================================

/// Accept "host", "host:port", or even raw IPs. Bare host gets DEFAULT_PORT
/// appended so the sender always has something to bind to.
fn normalize_address(input: &str) -> Option<String> {
    let s = input.trim();
    if s.is_empty() {
        return None;
    }
    if s.contains(':') {
        // Validate port half is numeric. Otherwise refuse.
        let (_, port) = s.rsplit_once(':')?;
        if port.parse::<u16>().is_ok() {
            Some(s.to_string())
        } else {
            None
        }
    } else {
        Some(format!("{s}:{DEFAULT_PORT}"))
    }
}

// ============================================================================
// Main
// ============================================================================

fn main() -> std::io::Result<()> {
    let cfg = load_config();
    let keymap = keymap_from_config(&cfg);
    let address = std::env::args()
        .nth(1)
        .and_then(|a| normalize_address(&a))
        .or_else(|| cfg.target.clone().and_then(|t| normalize_address(&t)))
        .unwrap_or_else(|| DEFAULT_ROVER.to_string());

    let state = Arc::new(Mutex::new(State::new(keymap, address)));

    // Background thread: streams the current state to whatever address
    // state.address currently holds. Re-reads each tick so address changes
    // take effect immediately.
    let sender_state = Arc::clone(&state);
    let sender = thread::spawn(move || {
        let sock = match UdpSocket::bind("0.0.0.0:0") {
            Ok(s) => s,
            Err(_) => return,
        };
        loop {
            let (vx, vy, omega, quit, addr) = {
                let s = sender_state.lock().unwrap();
                (s.vx, s.vy, s.omega, s.quit, s.address.clone())
            };
            if quit {
                let _ = sock.send_to(b"stop", &addr);
                return;
            }
            let msg = format!("{vx} {vy} {omega}");
            let _ = sock.send_to(msg.as_bytes(), &addr);
            {
                let mut s = sender_state.lock().unwrap();
                s.sent = s.sent.saturating_add(1);
            }
            thread::sleep(Duration::from_millis(TICK_MS));
        }
    });

    let mut out = stdout();
    terminal::enable_raw_mode()?;
    execute!(out, EnterAlternateScreen, cursor::Hide)?;

    let release_events = supports_keyboard_enhancement().unwrap_or(false);
    if release_events {
        execute!(
            out,
            PushKeyboardEnhancementFlags(KeyboardEnhancementFlags::REPORT_EVENT_TYPES)
        )?;
    }

    let result = ui_loop(&mut out, &state, release_events);

    if release_events {
        let _ = execute!(out, PopKeyboardEnhancementFlags);
    }
    let _ = execute!(out, cursor::Show, LeaveAlternateScreen);
    let _ = terminal::disable_raw_mode();

    {
        let mut s = state.lock().unwrap();
        s.quit = true;
        s.zero_velocity();
    }
    let _ = sender.join();

    result
}

fn ui_loop(
    out: &mut Stdout,
    state: &Arc<Mutex<State>>,
    release_events: bool,
) -> std::io::Result<()> {
    draw(out, state, release_events)?;
    loop {
        if event::poll(Duration::from_millis(100))?
            && let Event::Key(key) = event::read()?
            && !handle_key(key, state, release_events)
        {
            return Ok(());
        }
        draw(out, state, release_events)?;
    }
}

fn rescale_active(s: &mut State) {
    if s.vx != 0 {
        s.vx = s.vx.signum() * s.speed;
    }
    if s.vy != 0 {
        s.vy = s.vy.signum() * s.speed;
    }
    if s.omega != 0 {
        s.omega = s.omega.signum() * s.speed;
    }
}

/// Returns false when the user requests quit.
fn handle_key(key: KeyEvent, state: &Arc<Mutex<State>>, release_events: bool) -> bool {
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    let is_press = matches!(key.kind, KeyEventKind::Press | KeyEventKind::Repeat);

    // Ctrl-C always quits (hardcoded escape hatch).
    if is_press && ctrl && matches!(key.code, KeyCode::Char('c') | KeyCode::Char('C')) {
        return false;
    }

    let mut s = state.lock().unwrap();

    // Mode-specific overlays — these capture all input while active.
    match s.mode.clone() {
        Mode::Rebind { cursor } if is_press => {
            return handle_rebind_select(&mut s, cursor, key);
        }
        Mode::AwaitKey { action, cursor } if is_press => {
            return handle_rebind_press(&mut s, action, cursor, key);
        }
        Mode::EditAddress { buffer } if is_press => {
            return handle_address_edit(&mut s, buffer, key);
        }
        _ => {}
    }

    // Mode-entry chords from the drive screen. Ctrl-R and Ctrl-T are
    // hardcoded — never remappable — so a broken binding can't lock you out.
    if is_press && ctrl && matches!(key.code, KeyCode::Char('r') | KeyCode::Char('R')) {
        s.mode = Mode::Rebind { cursor: 0 };
        s.zero_velocity();
        s.status = None;
        return true;
    }
    if is_press && ctrl && matches!(key.code, KeyCode::Char('t') | KeyCode::Char('T')) {
        let buf = s.address.clone();
        s.mode = Mode::EditAddress { buffer: buf };
        s.zero_velocity();
        s.status = None;
        return true;
    }

    // Driving: look up the key in the keymap.
    let action = s.keymap.lookup(key.code);
    let speed = s.speed;

    match (key.kind, action) {
        (KeyEventKind::Press | KeyEventKind::Repeat, Some(a)) => match a {
            Action::Forward => s.vy = speed,
            Action::Backward => s.vy = -speed,
            Action::StrafeLeft => s.vx = -speed,
            Action::StrafeRight => s.vx = speed,
            Action::RotateCcw => s.omega = speed,
            Action::RotateCw => s.omega = -speed,
            Action::Stop => s.zero_velocity(),
            Action::SpeedUp => {
                s.speed = (s.speed + SPEED_STEP).min(SPEED_MAX);
                rescale_active(&mut s);
            }
            Action::SpeedDown => {
                s.speed = (s.speed - SPEED_STEP).max(SPEED_MIN);
                rescale_active(&mut s);
            }
            Action::Quit => return false,
        },
        (KeyEventKind::Release, Some(a)) if release_events => match a {
            Action::Forward if s.vy > 0 => s.vy = 0,
            Action::Backward if s.vy < 0 => s.vy = 0,
            Action::StrafeLeft if s.vx < 0 => s.vx = 0,
            Action::StrafeRight if s.vx > 0 => s.vx = 0,
            Action::RotateCcw if s.omega > 0 => s.omega = 0,
            Action::RotateCw if s.omega < 0 => s.omega = 0,
            _ => {}
        },
        _ => {}
    }
    true
}

fn handle_rebind_select(s: &mut State, cursor: usize, key: KeyEvent) -> bool {
    let n = Action::ALL.len();
    match key.code {
        KeyCode::Up | KeyCode::Char('k') => {
            let next = if cursor == 0 { n - 1 } else { cursor - 1 };
            s.mode = Mode::Rebind { cursor: next };
        }
        KeyCode::Down | KeyCode::Char('j') => {
            s.mode = Mode::Rebind {
                cursor: (cursor + 1) % n,
            };
        }
        KeyCode::Home => s.mode = Mode::Rebind { cursor: 0 },
        KeyCode::End => s.mode = Mode::Rebind { cursor: n - 1 },
        KeyCode::Enter => {
            s.mode = Mode::AwaitKey {
                action: Action::ALL[cursor],
                cursor,
            };
        }
        KeyCode::Esc => {
            s.mode = Mode::Drive;
            s.status = None;
        }
        _ => {}
    }
    true
}

fn handle_rebind_press(
    s: &mut State,
    action: Action,
    cursor: usize,
    key: KeyEvent,
) -> bool {
    if key.code == KeyCode::Esc {
        s.mode = Mode::Rebind { cursor };
        return true;
    }
    // Skip pure modifier presses (they'd never fire alone as a binding).
    if matches!(
        key.code,
        KeyCode::Modifier(_) | KeyCode::CapsLock | KeyCode::NumLock | KeyCode::ScrollLock | KeyCode::Null
    ) {
        return true;
    }
    s.keymap.rebind(action, key.code);
    s.status = Some(format!(
        "bound {} → {}",
        action.label(),
        key_to_string(key.code)
    ));
    s.mode = Mode::Rebind { cursor };
    save_state(s);
    true
}

fn handle_address_edit(s: &mut State, mut buffer: String, key: KeyEvent) -> bool {
    match key.code {
        KeyCode::Enter => match normalize_address(&buffer) {
            Some(addr) => {
                s.address = addr.clone();
                s.status = Some(format!("target → {addr}"));
                s.mode = Mode::Drive;
                save_state(s);
            }
            None => {
                s.status = Some("invalid address (use IP or IP:PORT)".into());
                s.mode = Mode::EditAddress { buffer };
            }
        },
        KeyCode::Esc => {
            s.mode = Mode::Drive;
            s.status = None;
        }
        KeyCode::Backspace => {
            buffer.pop();
            s.mode = Mode::EditAddress { buffer };
        }
        KeyCode::Char(c) if !c.is_control() => {
            buffer.push(c);
            s.mode = Mode::EditAddress { buffer };
        }
        _ => {}
    }
    true
}

// ============================================================================
// UI
// ============================================================================

fn draw(
    out: &mut Stdout,
    state: &Arc<Mutex<State>>,
    release_events: bool,
) -> std::io::Result<()> {
    let s = state.lock().unwrap();

    queue!(out, Clear(ClearType::All), cursor::MoveTo(0, 0))?;
    draw_header(out)?;
    draw_status_block(out, &s, release_events)?;
    draw_axes(out, &s)?;

    match &s.mode {
        Mode::Drive => {
            draw_keymap(out, &s, None)?;
            draw_status_line(out, &s)?;
            draw_footer(out, &[("Ctrl-R", "rebind"), ("Ctrl-T", "target"), ("Ctrl-C", "quit")])?;
        }
        Mode::Rebind { cursor } => {
            draw_keymap(out, &s, Some(*cursor))?;
            queue!(
                out,
                Print("\r\n"),
                Print("  rebind: ".on_yellow().black().bold()),
                Print(" ↑/↓ select, Enter to bind, Esc to exit\r\n".dark_grey()),
            )?;
            draw_footer(out, &[("↑/↓", "move"), ("Enter", "bind"), ("Esc", "cancel"), ("Ctrl-C", "quit")])?;
        }
        Mode::AwaitKey { action, cursor } => {
            draw_keymap(out, &s, Some(*cursor))?;
            queue!(
                out,
                Print("\r\n"),
                Print("  rebind: ".on_yellow().black().bold()),
                Print(format!(
                    " press the new key for {} (Esc to cancel)\r\n",
                    action.label().bold().yellow()
                )),
            )?;
            draw_footer(out, &[("any key", "bind"), ("Esc", "cancel"), ("Ctrl-C", "quit")])?;
        }
        Mode::EditAddress { buffer } => {
            draw_keymap(out, &s, None)?;
            queue!(
                out,
                Print("\r\n"),
                Print("  target: ".on_cyan().black().bold()),
                Print(" enter IP or IP:PORT, Enter to save, Esc to cancel\r\n".dark_grey()),
                Print(format!("    > {}\r\n", format!("{buffer}_").bold())),
            )?;
            if let Some(msg) = &s.status {
                queue!(out, Print(format!("    {}\r\n", msg.clone().red())))?;
            }
            draw_footer(out, &[("Enter", "save"), ("Esc", "cancel"), ("Ctrl-C", "quit")])?;
        }
    }

    drop(s);
    out.flush()
}

fn draw_header(out: &mut Stdout) -> std::io::Result<()> {
    queue!(
        out,
        Print(" ROVER CONTROL ".on_dark_cyan().black().bold()),
        Print("\r\n\r\n"),
    )
}

fn draw_status_block(out: &mut Stdout, s: &State, release_events: bool) -> std::io::Result<()> {
    let mode_str = if release_events {
        "hold-to-drive".green().to_string()
    } else {
        "sticky (terminal does not report key release)".yellow().to_string()
    };
    queue!(
        out,
        Print(format!("  target   {}\r\n", s.address.clone().cyan())),
        Print(format!("  speed    {}\r\n", s.speed.to_string().yellow())),
        Print(format!("  packets  {}\r\n", s.sent.to_string().dark_grey())),
        Print(format!("  input    {mode_str}\r\n\r\n")),
    )
}

fn draw_axes(out: &mut Stdout, s: &State) -> std::io::Result<()> {
    let axis = |name: &str, v: i32, neg: &str, pos: &str| {
        let arrow = match v.signum() {
            1 => pos.green().to_string(),
            -1 => neg.green().to_string(),
            _ => "·".dark_grey().to_string(),
        };
        format!("    {name:<6} {v:>+5}   {arrow}")
    };
    queue!(
        out,
        Print("  velocity\r\n".dark_grey()),
        Print(axis("vx", s.vx, "←", "→")),
        Print("\r\n"),
        Print(axis("vy", s.vy, "↓", "↑")),
        Print("\r\n"),
        Print(axis("ω", s.omega, "↻", "↺")),
        Print("\r\n\r\n"),
    )
}

fn draw_keymap(out: &mut Stdout, s: &State, cursor: Option<usize>) -> std::io::Result<()> {
    queue!(out, Print("  keybindings\r\n".dark_grey()))?;
    for (i, action) in Action::ALL.iter().enumerate() {
        let key = key_to_string(s.keymap.get(*action));
        let row = format!("  {:<14} {}", action.label(), key);
        if cursor == Some(i) {
            queue!(out, Print(format!("  ▶ {}\r\n", row.trim_start().on_dark_grey().white().bold())))?;
        } else {
            queue!(out, Print(format!("    {row}\r\n")))?;
        }
    }
    Ok(())
}

fn draw_status_line(out: &mut Stdout, s: &State) -> std::io::Result<()> {
    queue!(out, Print("\r\n"))?;
    if let Some(msg) = &s.status {
        queue!(out, Print(format!("  → {}\r\n", msg.clone().green())))?;
    }
    Ok(())
}

fn draw_footer(out: &mut Stdout, hints: &[(&str, &str)]) -> std::io::Result<()> {
    queue!(out, Print("\r\n  "))?;
    for (i, (key, label)) in hints.iter().enumerate() {
        if i > 0 {
            queue!(out, Print("  ".dark_grey()))?;
        }
        queue!(
            out,
            Print((*key).bold()),
            Print(" "),
            Print((*label).dark_grey()),
        )?;
    }
    queue!(out, Print("\r\n"))
}
