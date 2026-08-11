//! Terminal output, progress animation, and display-width helpers.

use std::{
    env,
    io::{self, IsTerminal, Write},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};
use unicode_width::UnicodeWidthStr;

const GREEN: &str = "\x1b[32m";
const RED: &str = "\x1b[31m";
const CYAN: &str = "\x1b[36m";
const DIM: &str = "\x1b[2m";
const RESET: &str = "\x1b[0m";

const ICON_OK: &str = "✓";
const ICON_ERR: &str = "✗";
const FRAMES: &[&str] = &["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

pub const ACTION_WIDTH: usize = 13;

const REVEAL_SPIN_FRAMES: usize = 3;
const REVEAL_SPIN_MS: u64 = 22;
const REVEAL_GAP_MS: u64 = 16;

/// Returns whether human-oriented terminal output is available.
pub fn interactive(json: bool) -> bool {
    !json && io::stderr().is_terminal()
}

fn stdout_tty() -> bool {
    io::stdout().is_terminal()
}

pub(crate) fn stdout_color() -> bool {
    stdout_tty() && colors_allowed()
}

fn stderr_color() -> bool {
    io::stderr().is_terminal() && colors_allowed()
}

fn colors_allowed() -> bool {
    env::var_os("NO_COLOR").is_none()
}

pub fn ok(action: &str, detail: impl AsRef<str>) {
    if stdout_color() {
        println!(
            "{GREEN}{ICON_OK}{RESET}  {:<ACTION_WIDTH$}  {}",
            action,
            detail.as_ref()
        );
    } else {
        println!("{ICON_OK}  {:<ACTION_WIDTH$}  {}", action, detail.as_ref());
    }
    let _ = io::stdout().flush();
}

pub fn err(action: &str, detail: impl AsRef<str>) {
    if stderr_color() {
        eprintln!(
            "{RED}{ICON_ERR}{RESET}  {:<ACTION_WIDTH$}  {}",
            action,
            detail.as_ref()
        );
    } else {
        eprintln!("{ICON_ERR}  {:<ACTION_WIDTH$}  {}", action, detail.as_ref());
    }
    let _ = io::stderr().flush();
}

pub fn note(message: impl AsRef<str>) {
    if stderr_color() {
        eprintln!("{DIM}·{RESET}  {:<ACTION_WIDTH$}  {}", "", message.as_ref());
    } else {
        eprintln!("·  {:<ACTION_WIDTH$}  {}", "", message.as_ref());
    }
}

pub fn sub(detail: impl AsRef<str>) {
    if stdout_color() {
        println!("{DIM}   {:<ACTION_WIDTH$}  {}{RESET}", "", detail.as_ref());
    } else {
        println!("   {:<ACTION_WIDTH$}  {}", "", detail.as_ref());
    }
    let _ = io::stdout().flush();
}

/// Animates one success row when stdout is a terminal.
pub async fn reveal_ok(action: &str, detail: impl AsRef<str>) {
    let detail = detail.as_ref();
    if !stdout_tty() {
        ok(action, detail);
        return;
    }

    let color = stdout_color();
    for frame in FRAMES.iter().cycle().take(REVEAL_SPIN_FRAMES) {
        if color {
            print!("\r{CYAN}{frame}{RESET}  {action:<ACTION_WIDTH$}  {detail}   ");
        } else {
            print!("\r{frame}  {action:<ACTION_WIDTH$}  {detail}   ");
        }
        let _ = io::stdout().flush();
        tokio::time::sleep(Duration::from_millis(REVEAL_SPIN_MS)).await;
    }
    print!("\r\x1b[2K");
    ok(action, detail);
    tokio::time::sleep(Duration::from_millis(REVEAL_GAP_MS)).await;
}

/// Animates a sequence of success rows.
pub async fn reveal_ok_rows(rows: impl IntoIterator<Item = (String, String)>) {
    for (action, detail) in rows {
        reveal_ok(&action, detail).await;
    }
}

pub async fn reveal_sub(detail: impl AsRef<str>) {
    sub(detail);
    if stdout_tty() {
        tokio::time::sleep(Duration::from_millis(REVEAL_GAP_MS / 2)).await;
    }
}

pub fn platform_row(id: &str, name: &str, note: &str) {
    let name_aligned = pad_display(name, 14);
    if stdout_color() {
        println!("{GREEN}{ICON_OK}{RESET}  {id:<10}  {name_aligned}  ·  {DIM}{note}{RESET}");
    } else {
        println!("{ICON_OK}  {id:<10}  {name_aligned}  ·  {note}");
    }
}

/// Returns the Unicode terminal width of `text`.
pub fn display_width(text: &str) -> usize {
    UnicodeWidthStr::width(text)
}

/// Right-pads `text` to `width` terminal columns.
pub fn pad_display(text: &str, width: usize) -> String {
    let w = display_width(text);
    if w >= width {
        text.to_owned()
    } else {
        format!("{text}{}", " ".repeat(width - w))
    }
}

/// Left-pads `text` to `width` terminal columns.
pub fn pad_display_left(text: &str, width: usize) -> String {
    let w = display_width(text);
    if w >= width {
        text.to_owned()
    } else {
        format!("{}{text}", " ".repeat(width - w))
    }
}

/// Background spinner for long-running commands.
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
        let color = stderr_color();
        let handle = tokio::spawn(async move {
            let mut frame = 0usize;
            while !flag.load(Ordering::Relaxed) {
                let glyph = FRAMES[frame % FRAMES.len()];
                {
                    let message = label.lock();
                    let message = message.as_deref().map(String::as_str).unwrap_or_default();
                    if color {
                        eprint!("\r{CYAN}{glyph}{RESET}  {message}   ");
                    } else {
                        eprint!("\r{glyph}  {message}   ");
                    }
                    let _ = io::stderr().flush();
                }
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

    /// Stops the spinner and prints success.
    pub async fn finish_ok(mut self, action: &str, detail: impl AsRef<str>) {
        self.stop_and_clear().await;
        reveal_ok(action, detail).await;
    }

    /// Stops the spinner without output.
    pub async fn finish_silent(mut self) {
        self.stop_and_clear().await;
    }

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

pub fn download_progress_label(percent: u8, downloaded: u64, total: u64) -> String {
    format!(
        "Downloading…  {percent:>3}%  ·  {} / {}",
        format_bytes(downloaded),
        format_bytes(total)
    )
}

pub fn one_line(text: &str, max_chars: usize) -> String {
    let mut words = text.split_whitespace();
    let mut collapsed = words.next().unwrap_or_default().to_owned();
    for word in words {
        collapsed.push(' ');
        collapsed.push_str(word);
    }
    if max_chars == 0 {
        return collapsed;
    }

    let mut chars = collapsed.char_indices();
    let Some((truncate_at, _)) = chars.nth(max_chars - 1) else {
        return collapsed;
    };
    if chars.next().is_none() {
        return collapsed;
    }
    collapsed.truncate(truncate_at);
    collapsed.push('…');
    collapsed
}

#[cfg(test)]
mod tests {
    use super::{display_width, one_line, pad_display};

    #[test]
    fn display_width_handles_wide_and_combining_characters() {
        assert_eq!(display_width("abc"), 3);
        assert_eq!(display_width("你好"), 4);
        assert_eq!(display_width("e\u{301}"), 1);
        assert_eq!(pad_display("好", 4), "好  ");
    }

    #[test]
    fn one_line_collapses_and_truncates_unicode_without_extra_spaces() {
        assert_eq!(one_line("  hello\n  world  ", 20), "hello world");
        assert_eq!(one_line("你好世界", 3), "你好…");
        assert_eq!(one_line("abc", 3), "abc");
        assert_eq!(one_line("abc", 1), "…");
        assert_eq!(one_line("abc", 0), "abc");
    }
}
