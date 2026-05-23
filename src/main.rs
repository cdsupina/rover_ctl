use std::io::{Stdout, Write, stdout};
use std::net::UdpSocket;
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

const DEFAULT_ROVER: &str = "192.168.1.102:4242";
const TICK_HZ: u64 = 10;
const TICK_MS: u64 = 1000 / TICK_HZ;
const SPEED_DEFAULT: i32 = 700;
const SPEED_STEP: i32 = 100;
const SPEED_MIN: i32 = 100;
const SPEED_MAX: i32 = 1000;

#[derive(Default)]
struct State {
    vx: i32,
    vy: i32,
    omega: i32,
    speed: i32,
    sent: u64,
    quit: bool,
}

fn main() -> std::io::Result<()> {
    let rover_addr = std::env::args()
        .nth(1)
        .map(|a| if a.contains(':') { a } else { format!("{a}:4242") })
        .unwrap_or_else(|| DEFAULT_ROVER.to_string());

    let state = Arc::new(Mutex::new(State {
        speed: SPEED_DEFAULT,
        ..Default::default()
    }));

    // Background thread: streams the current state to the rover at TICK_HZ.
    // This is what keeps the rover's 500 ms safety watchdog fed.
    let sender_state = Arc::clone(&state);
    let sender_addr = rover_addr.clone();
    let sender = thread::spawn(move || {
        let sock = match UdpSocket::bind("0.0.0.0:0") {
            Ok(s) => s,
            Err(_) => return,
        };
        loop {
            let (vx, vy, omega, quit) = {
                let s = sender_state.lock().unwrap();
                (s.vx, s.vy, s.omega, s.quit)
            };
            if quit {
                let _ = sock.send_to(b"stop", &sender_addr);
                return;
            }
            let msg = format!("{vx} {vy} {omega}");
            let _ = sock.send_to(msg.as_bytes(), &sender_addr);
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

    // Modern terminals (kitty, ghostty, foot, alacritty ≥0.13, wezterm) report
    // key release events. When available, releasing a key zeroes that axis —
    // game-like feel. Otherwise the keys are sticky; press Space to stop.
    let release_events = supports_keyboard_enhancement().unwrap_or(false);
    if release_events {
        execute!(
            out,
            PushKeyboardEnhancementFlags(KeyboardEnhancementFlags::REPORT_EVENT_TYPES)
        )?;
    }

    let result = ui_loop(&mut out, &state, &rover_addr, release_events);

    if release_events {
        let _ = execute!(out, PopKeyboardEnhancementFlags);
    }
    let _ = execute!(out, cursor::Show, LeaveAlternateScreen);
    let _ = terminal::disable_raw_mode();

    {
        let mut s = state.lock().unwrap();
        s.quit = true;
        s.vx = 0;
        s.vy = 0;
        s.omega = 0;
    }
    let _ = sender.join();

    result
}

fn ui_loop(
    out: &mut Stdout,
    state: &Arc<Mutex<State>>,
    rover_addr: &str,
    release_events: bool,
) -> std::io::Result<()> {
    draw(out, state, rover_addr, release_events)?;
    loop {
        // Periodic redraw so the packet counter ticks even with no input.
        if event::poll(Duration::from_millis(100))?
            && let Event::Key(key) = event::read()?
            && !handle_key(key, state, release_events)
        {
            return Ok(());
        }
        draw(out, state, rover_addr, release_events)?;
    }
}

/// Rescale any currently-active axis to the new speed level (preserving sign),
/// so a +/- press while driving immediately changes how fast the rover is going
/// without needing to re-press the direction key.
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

    if matches!(key.kind, KeyEventKind::Press | KeyEventKind::Repeat)
        && (key.code == KeyCode::Esc
            || (ctrl && matches!(key.code, KeyCode::Char('c') | KeyCode::Char('C'))))
    {
        return false;
    }

    let mut s = state.lock().unwrap();
    let speed = s.speed;

    match key.kind {
        KeyEventKind::Press | KeyEventKind::Repeat => match key.code {
            KeyCode::Char('w') | KeyCode::Char('W') => s.vy = speed,
            KeyCode::Char('s') | KeyCode::Char('S') => s.vy = -speed,
            KeyCode::Char('a') | KeyCode::Char('A') => s.vx = -speed,
            KeyCode::Char('d') | KeyCode::Char('D') => s.vx = speed,
            KeyCode::Char('q') | KeyCode::Char('Q') => s.omega = speed,
            KeyCode::Char('e') | KeyCode::Char('E') => s.omega = -speed,
            KeyCode::Char(' ') | KeyCode::Char('x') | KeyCode::Char('X') => {
                s.vx = 0;
                s.vy = 0;
                s.omega = 0;
            }
            KeyCode::Char('+') | KeyCode::Char('=') => {
                s.speed = (s.speed + SPEED_STEP).min(SPEED_MAX);
                rescale_active(&mut s);
            }
            KeyCode::Char('-') | KeyCode::Char('_') => {
                s.speed = (s.speed - SPEED_STEP).max(SPEED_MIN);
                rescale_active(&mut s);
            }
            _ => {}
        },
        KeyEventKind::Release if release_events => match key.code {
            KeyCode::Char('w') | KeyCode::Char('W') if s.vy > 0 => s.vy = 0,
            KeyCode::Char('s') | KeyCode::Char('S') if s.vy < 0 => s.vy = 0,
            KeyCode::Char('a') | KeyCode::Char('A') if s.vx < 0 => s.vx = 0,
            KeyCode::Char('d') | KeyCode::Char('D') if s.vx > 0 => s.vx = 0,
            KeyCode::Char('q') | KeyCode::Char('Q') if s.omega > 0 => s.omega = 0,
            KeyCode::Char('e') | KeyCode::Char('E') if s.omega < 0 => s.omega = 0,
            _ => {}
        },
        _ => {}
    }
    true
}

fn draw(
    out: &mut Stdout,
    state: &Arc<Mutex<State>>,
    rover_addr: &str,
    release_events: bool,
) -> std::io::Result<()> {
    let (vx, vy, omega, speed, sent) = {
        let s = state.lock().unwrap();
        (s.vx, s.vy, s.omega, s.speed, s.sent)
    };

    queue!(out, Clear(ClearType::All), cursor::MoveTo(0, 0))?;
    queue!(
        out,
        Print(" ROVER CONTROL ".on_dark_cyan().black().bold()),
        Print("\r\n\r\n")
    )?;
    queue!(
        out,
        Print(format!(
            "  target:  {}\r\n",
            rover_addr.to_string().dark_grey()
        ))
    )?;
    queue!(
        out,
        Print(format!("  speed:   {}\r\n", speed.to_string().yellow()))
    )?;
    queue!(out, Print(format!("  packets: {sent}\r\n")))?;
    queue!(
        out,
        Print(format!(
            "  mode:    {}\r\n\r\n",
            if release_events {
                "hold-to-drive (key release supported)".green().to_string()
            } else {
                "sticky (press Space to stop — terminal does not report key release)"
                    .yellow()
                    .to_string()
            }
        ))
    )?;

    let axis = |name: &str, v: i32, neg: &str, pos: &str| {
        let arrow = match v.signum() {
            1 => pos.green().to_string(),
            -1 => neg.green().to_string(),
            _ => "·".dark_grey().to_string(),
        };
        format!("    {name:<6} {v:>+5}   {arrow}")
    };
    queue!(out, Print("  velocity:\r\n".dark_grey()))?;
    queue!(out, Print(axis("vx", vx, "←", "→")), Print("\r\n"))?;
    queue!(out, Print(axis("vy", vy, "↓", "↑")), Print("\r\n"))?;
    queue!(out, Print(axis("ω", omega, "↻", "↺")), Print("\r\n\r\n"))?;

    queue!(out, Print("  keys:\r\n".dark_grey()))?;
    queue!(
        out,
        Print(format!(
            "    {}  forward     {}  backward     {}  strafe left     {}  strafe right\r\n",
            "W".bold(),
            "S".bold(),
            "A".bold(),
            "D".bold()
        ))
    )?;
    queue!(
        out,
        Print(format!(
            "    {}  rotate CCW  {}  rotate CW\r\n",
            "Q".bold(),
            "E".bold()
        ))
    )?;
    queue!(
        out,
        Print(format!(
            "    {}  stop        {} / {}  speed       {} / {}  quit\r\n",
            "Space".bold(),
            "+".bold(),
            "-".bold(),
            "Esc".bold(),
            "Ctrl-C".bold()
        ))
    )?;

    out.flush()
}
