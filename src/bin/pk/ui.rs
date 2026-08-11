//! inline spinner while work is in progress, green `✓` / red `✗` when done.
//!
//! Status lines use a fixed action column so output stays aligned:
//!
//! ```text
//! ✓  Resolved       wechat · title…
//! ✓  Saved          ./downloads/wechat_xxx.mp4 · 8.1MB
//! ✓  Already saved  ./downloads/wechat_xxx.mp4 · 640.2MB
//! ```

use std::{
    io::{self, IsTerminal, Write},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

const GREEN: &str = "\x1b[32m";
const RED: &str = "\x1b[31m";
const CYAN: &str = "\x1b[36m";
const DIM: &str = "\x1b[2m";
const RESET: &str = "\x1b[0m";

const ICON_OK: &str = "✓";
const ICON_ERR: &str = "✗";
const FRAMES: &[&str] = &["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

/// Width of the action column (`Already saved` is the longest common verb).
const ACTION_WIDTH: usize = 13;

/// Human interactive mode: not JSON and stderr is a terminal.
pub fn interactive(json: bool) -> bool {
    !json && io::stderr().is_terminal()
}

/// `✓  Resolved       wechat · title…`
pub fn ok(action: &str, detail: impl AsRef<str>) {
    println!(
        "{GREEN}{ICON_OK}{RESET}  {:<ACTION_WIDTH$}  {}",
        action,
        detail.as_ref()
    );
}

/// `✗  Failed         …`
pub fn err(action: &str, detail: impl AsRef<str>) {
    eprintln!(
        "{RED}{ICON_ERR}{RESET}  {:<ACTION_WIDTH$}  {}",
        action,
        detail.as_ref()
    );
}

/// Dim note without a strong status icon column.
pub fn note(message: impl AsRef<str>) {
    eprintln!("{DIM}·{RESET}  {:<ACTION_WIDTH$}  {}", "", message.as_ref());
}

/// Aligned platform row: `✓  wechat      微信视频号    ·  needs cookie`
pub fn platform_row(id: &str, name: &str, note: &str) {
    let name_col: usize = 14;
    let pad = name_col.saturating_sub(display_width(name));
    let name_aligned = format!("{name}{}", " ".repeat(pad));
    println!("{GREEN}{ICON_OK}{RESET}  {id:<10}  {name_aligned}  ·  {DIM}{note}{RESET}");
}

/// Approximate terminal columns: CJK-ish code points count as 2.
fn display_width(text: &str) -> usize {
    text.chars()
        .map(|ch| {
            let u = ch as u32;
            if ch.is_ascii() {
                1
            } else if (0x1100..=0x115F).contains(&u)
                || (0x2E80..=0xA4CF).contains(&u)
                || (0xAC00..=0xD7A3).contains(&u)
                || (0xF900..=0xFAFF).contains(&u)
                || (0xFE10..=0xFE6F).contains(&u)
                || (0xFF00..=0xFF60).contains(&u)
                || (0xFFE0..=0xFFE6).contains(&u)
                || (0x20000..=0x2FFFD).contains(&u)
            {
                2
            } else {
                1
            }
        })
        .sum()
}

/// Inline braille spinner on stderr (Mole-style). No-op when not interactive.
pub struct Spinner {
    stop: Arc<AtomicBool>,
    message: Arc<Mutex<String>>,
    handle: Option<tokio::task::JoinHandle<()>>,
    active: bool,
}

impl Spinner {
    pub fn start(message: impl Into<String>) -> Self {
        let message = message.into();
        if !io::stderr().is_terminal() {
            return Self {
                stop: Arc::new(AtomicBool::new(true)),
                message: Arc::new(Mutex::new(message)),
                handle: None,
                active: false,
            };
        }

        let stop = Arc::new(AtomicBool::new(false));
        let text = Arc::new(Mutex::new(message));
        let flag = Arc::clone(&stop);
        let label = Arc::clone(&text);
        let handle = tokio::spawn(async move {
            let mut frame = 0usize;
            while !flag.load(Ordering::Relaxed) {
                let glyph = FRAMES[frame % FRAMES.len()];
                let message = label.lock().map(|guard| guard.clone()).unwrap_or_default();
                // Two spaces after glyph to match status-line gutter.
                eprint!("\r{CYAN}{glyph}{RESET}  {message}   ");
                let _ = io::stderr().flush();
                frame = frame.wrapping_add(1);
                tokio::time::sleep(Duration::from_millis(80)).await;
            }
        });

        Self {
            stop,
            message: text,
            handle: Some(handle),
            active: true,
        }
    }

    /// Cloneable handle for progress callbacks (must not outlive the spinner).
    pub fn label(&self) -> SpinnerLabel {
        SpinnerLabel {
            message: Arc::clone(&self.message),
        }
    }

    async fn stop_and_clear(&mut self) {
        if !self.active {
            return;
        }
        self.stop.store(true, Ordering::Relaxed);
        if let Some(handle) = self.handle.take() {
            let _ = handle.await;
        }
        eprint!("\r\x1b[2K");
        let _ = io::stderr().flush();
        self.active = false;
    }

    /// Clear spinner and print an aligned success line.
    pub async fn finish_ok(mut self, action: &str, detail: impl AsRef<str>) {
        self.stop_and_clear().await;
        ok(action, detail);
    }

    /// Clear spinner and print an aligned error line.
    pub async fn finish_err(mut self, action: &str, detail: impl AsRef<str>) {
        self.stop_and_clear().await;
        err(action, detail);
    }
}

impl Drop for Spinner {
    fn drop(&mut self) {
        if self.active {
            self.stop.store(true, Ordering::Relaxed);
            eprint!("\r\x1b[2K");
            let _ = io::stderr().flush();
            self.active = false;
        }
    }
}

/// Shared spinner text for progress callbacks.
#[derive(Clone)]
pub struct SpinnerLabel {
    message: Arc<Mutex<String>>,
}

impl SpinnerLabel {
    pub fn set(&self, message: impl Into<String>) {
        if let Ok(mut guard) = self.message.lock() {
            *guard = message.into();
        }
    }
}

pub fn format_bytes(bytes: u64) -> String {
    const KB: f64 = 1024.0;
    const MB: f64 = KB * 1024.0;
    const GB: f64 = MB * 1024.0;
    let n = bytes as f64;
    if n >= GB {
        format!("{:.2} GB", n / GB)
    } else if n >= MB {
        format!("{:.1} MB", n / MB)
    } else if n >= KB {
        format!("{:.0} KB", n / KB)
    } else {
        format!("{bytes} B")
    }
}

/// Spinner label: `Downloading…  42%  ·  12.3 MB / 29.1 MB`
pub fn download_progress_label(percent: u8, downloaded: u64, total: u64) -> String {
    format!(
        "Downloading…  {percent:>3}%  ·  {} / {}",
        format_bytes(downloaded),
        format_bytes(total)
    )
}

/// Collapse whitespace/newlines so status lines stay on one row.
pub fn one_line(text: &str, max_chars: usize) -> String {
    let collapsed: String = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if max_chars == 0 {
        return collapsed;
    }
    let count = collapsed.chars().count();
    if count <= max_chars {
        return collapsed;
    }
    let mut out: String = collapsed
        .chars()
        .take(max_chars.saturating_sub(1))
        .collect();
    out.push('…');
    out
}
