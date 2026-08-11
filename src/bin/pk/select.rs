//! Interactive source picker with radio / checkbox circles (○ ●).
//!
//! Keys:
//! - `↑`/`k`  move up
//! - `↓`/`j`  move down
//! - `Space`  toggle (multi only)
//! - `1`–`9`  jump to index
//! - `Enter`  confirm
//! - `q`/`Esc` cancel

use std::io::{self, IsTerminal, Read, Write};

use parse_kit::{Error, MediaSource, Result};

const GREEN: &str = "\x1b[32m";
const CYAN: &str = "\x1b[36m";
const DIM: &str = "\x1b[2m";
const RESET: &str = "\x1b[0m";
const ICON_ON: &str = "●";
const ICON_OFF: &str = "○";

/// Single-choice radio list. Returns `None` if cancelled.
pub fn pick_one(options: &[String], default: usize) -> Result<Option<usize>> {
    if options.is_empty() {
        return Err(Error::MediaUnavailable);
    }
    let default = default.min(options.len() - 1);
    if options.len() == 1 || !stdin_stdout_tty() {
        return Ok(Some(default));
    }
    match run_picker(options, Mode::Single { cursor: default })? {
        Outcome::Cancel => Ok(None),
        Outcome::Single(i) => Ok(Some(i)),
        Outcome::Multi(v) => Ok(v.into_iter().next()),
    }
}

/// Multi-select. Space toggles, Enter confirms. Returns `None` if cancelled.
pub fn pick_many(options: &[String], preselect_all: bool) -> Result<Option<Vec<usize>>> {
    if options.is_empty() {
        return Err(Error::MediaUnavailable);
    }
    if options.len() == 1 {
        return Ok(Some(vec![0]));
    }
    if !stdin_stdout_tty() {
        return Ok(Some((0..options.len()).collect()));
    }
    let selected = if preselect_all {
        vec![true; options.len()]
    } else {
        let mut v = vec![false; options.len()];
        v[0] = true;
        v
    };
    match run_picker(
        options,
        Mode::Multi {
            cursor: 0,
            selected,
        },
    )? {
        Outcome::Cancel => Ok(None),
        Outcome::Multi(indices) => Ok(Some(indices)),
        Outcome::Single(i) => Ok(Some(vec![i])),
    }
}

/// Build picker labels from media sources.
pub fn source_option_labels(sources: &[&MediaSource]) -> Vec<String> {
    sources
        .iter()
        .enumerate()
        .map(|(i, source)| {
            let mark = if i == 0 { "★" } else { " " };
            format!("{mark}  {}", source.quality_summary())
        })
        .collect()
}

fn stdin_stdout_tty() -> bool {
    io::stdin().is_terminal() && io::stdout().is_terminal()
}

enum Mode {
    Single { cursor: usize },
    Multi { cursor: usize, selected: Vec<bool> },
}

enum Outcome {
    Cancel,
    Single(usize),
    Multi(Vec<usize>),
}

fn run_picker(options: &[String], mut mode: Mode) -> Result<Outcome> {
    let _raw = RawMode::enter().map_err(|e| Error::Config(format!("无法进入终端原始模式: {e}")))?;
    let lines = options.len() + 2;
    draw(&mode, options)?;

    let mut stdin = io::stdin();
    let mut buf = [0_u8; 16];
    loop {
        let n = stdin
            .read(&mut buf)
            .map_err(|e| Error::Config(format!("读取键盘失败: {e}")))?;
        if n == 0 {
            continue;
        }
        match parse_key(&buf[..n]) {
            Key::Up => match &mut mode {
                Mode::Single { cursor } | Mode::Multi { cursor, .. } => {
                    *cursor = cursor.saturating_sub(1);
                }
            },
            Key::Down => match &mut mode {
                Mode::Single { cursor } | Mode::Multi { cursor, .. } => {
                    if *cursor + 1 < options.len() {
                        *cursor += 1;
                    }
                }
            },
            Key::Toggle => {
                if let Mode::Multi { cursor, selected } = &mut mode {
                    selected[*cursor] = !selected[*cursor];
                }
            }
            Key::Digit(d) => {
                let idx = (d as usize).saturating_sub(1);
                if idx < options.len() {
                    match &mut mode {
                        Mode::Single { cursor } => *cursor = idx,
                        Mode::Multi { cursor, selected } => {
                            *cursor = idx;
                            selected[idx] = !selected[idx];
                        }
                    }
                }
            }
            Key::Enter => {
                let outcome = match &mode {
                    Mode::Single { cursor } => Outcome::Single(*cursor),
                    Mode::Multi { selected, .. } => {
                        let indices: Vec<usize> = selected
                            .iter()
                            .enumerate()
                            .filter(|(_, on)| **on)
                            .map(|(i, _)| i)
                            .collect();
                        if indices.is_empty() {
                            // Keep UI; require at least one selection.
                            redraw(&mode, options, lines)?;
                            continue;
                        }
                        Outcome::Multi(indices)
                    }
                };
                clear_drawn(lines)?;
                return Ok(outcome);
            }
            Key::Cancel => {
                clear_drawn(lines)?;
                return Ok(Outcome::Cancel);
            }
            Key::Other => {}
        }
        redraw(&mode, options, lines)?;
    }
}

fn draw(mode: &Mode, options: &[String]) -> Result<()> {
    let mut out = io::stdout();
    let (title, hint) = match mode {
        Mode::Single { .. } => (
            "选择下载画质",
            "↑↓ 移动  ·  1-9 跳转  ·  Enter 确认  ·  q 取消",
        ),
        Mode::Multi { .. } => (
            "选择要下载的图片",
            "↑↓ 移动  ·  Space 勾选  ·  Enter 确认  ·  q 取消",
        ),
    };
    writeln!(out, "{CYAN}{title}{RESET}").map_err(io_err)?;
    writeln!(out, "{DIM}{hint}{RESET}").map_err(io_err)?;

    for (i, label) in options.iter().enumerate() {
        let (cursor_here, on) = match mode {
            Mode::Single { cursor } => (*cursor == i, *cursor == i),
            Mode::Multi { cursor, selected } => (*cursor == i, selected[i]),
        };
        let circle = if on {
            format!("{GREEN}{ICON_ON}{RESET}")
        } else {
            format!("{DIM}{ICON_OFF}{RESET}")
        };
        let pointer = if cursor_here {
            format!("{CYAN}❯{RESET}")
        } else {
            " ".into()
        };
        writeln!(out, " {pointer} {circle}  [{i}]  {label}").map_err(io_err)?;
    }
    out.flush().map_err(io_err)?;
    Ok(())
}

fn redraw(mode: &Mode, options: &[String], lines: usize) -> Result<()> {
    clear_drawn(lines)?;
    draw(mode, options)
}

fn clear_drawn(lines: usize) -> Result<()> {
    let mut out = io::stdout();
    for _ in 0..lines {
        write!(out, "\x1b[1A\x1b[2K").map_err(io_err)?;
    }
    out.flush().map_err(io_err)?;
    Ok(())
}

fn io_err(error: io::Error) -> Error {
    Error::Config(format!("终端输出失败: {error}"))
}

enum Key {
    Up,
    Down,
    Toggle,
    Enter,
    Cancel,
    Digit(u8),
    Other,
}

fn parse_key(buf: &[u8]) -> Key {
    match buf {
        [b'\n' | b'\r', ..] => Key::Enter,
        [b'q' | b'Q'] => Key::Cancel,
        [0x1b, b'[', b'A', ..] => Key::Up,
        [0x1b, b'[', b'B', ..] => Key::Down,
        [b'k' | b'K', ..] => Key::Up,
        [b'j' | b'J', ..] => Key::Down,
        [b' ', ..] => Key::Toggle,
        [d @ b'1'..=b'9', ..] => Key::Digit(d - b'0'),
        [0x1b] => Key::Cancel,
        _ => Key::Other,
    }
}

struct RawMode {
    original: libc::termios,
}

impl RawMode {
    fn enter() -> io::Result<Self> {
        let mut original = std::mem::MaybeUninit::<libc::termios>::uninit();
        let fd = libc::STDIN_FILENO;
        if unsafe { libc::tcgetattr(fd, original.as_mut_ptr()) } != 0 {
            return Err(io::Error::last_os_error());
        }
        let original = unsafe { original.assume_init() };
        let mut raw = original;
        raw.c_lflag &= !(libc::ICANON | libc::ECHO);
        raw.c_cc[libc::VMIN] = 1;
        raw.c_cc[libc::VTIME] = 0;
        if unsafe { libc::tcsetattr(fd, libc::TCSANOW, &raw) } != 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(Self { original })
    }
}

impl Drop for RawMode {
    fn drop(&mut self) {
        let fd = libc::STDIN_FILENO;
        unsafe {
            let _ = libc::tcsetattr(fd, libc::TCSANOW, &self.original);
        }
    }
}
