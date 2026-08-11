//! Interactive single- and multi-source picker.

use std::io::{self, IsTerminal, Read, Write};

use parse_kit::{Error, MediaSource, Result};

use crate::{
    sources::source_kind_label,
    ui::{self, pad_display, pad_display_left},
};

const GREEN: &str = "\x1b[32m";
const CYAN: &str = "\x1b[36m";
const DIM: &str = "\x1b[2m";
const RESET: &str = "\x1b[0m";
const ICON_ON: &str = "●";
const ICON_OFF: &str = "○";
const COL_SEP: &str = "  ·  ";

/// Column-aligned source options.
#[derive(Debug, Clone)]
pub struct SourcePickerTable {
    /// Header aligned with [`Self::rows`].
    pub header: String,
    /// One aligned row per source.
    pub rows: Vec<String>,
}

/// Picks one option or returns `None` on cancellation.
pub fn pick_one(
    options: &[String],
    default: usize,
    column_header: Option<&str>,
) -> Result<Option<usize>> {
    if options.is_empty() {
        return Err(Error::MediaUnavailable);
    }
    let default = default.min(options.len() - 1);
    if options.len() == 1 || !stdin_stdout_tty() {
        return Ok(Some(default));
    }
    match run_picker(options, column_header, Mode::Single { cursor: default })? {
        Outcome::Cancel => Ok(None),
        Outcome::Single(i) => Ok(Some(i)),
        Outcome::Multi(v) => Ok(v.into_iter().next()),
    }
}

/// Picks multiple options or returns `None` on cancellation.
pub fn pick_many(
    options: &[String],
    preselect_all: bool,
    column_header: Option<&str>,
) -> Result<Option<Vec<usize>>> {
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
        column_header,
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

/// Builds the aligned source table.
pub fn source_option_table(sources: &[&MediaSource]) -> SourcePickerTable {
    let rows: Vec<SourceCols> = sources.iter().map(|source| source_cols(source)).collect();
    let (header, rows) = align_source_table(&rows);
    SourcePickerTable { header, rows }
}

struct SourceCols {
    label: String,
    dims: String,
    rate: String,
    size: String,
    kind: String,
}

fn source_cols(source: &MediaSource) -> SourceCols {
    let dims = match (source.width, source.height) {
        (Some(w), Some(h)) => format!("{w}×{h}"),
        _ => String::new(),
    };
    let rate = source
        .bitrate_bps
        .filter(|v| *v > 0)
        .map(format_bitrate)
        .unwrap_or_default();
    let size = source
        .size_hint
        .filter(|v| *v > 0)
        .map(ui::format_bytes)
        .unwrap_or_default();
    let kind = source_kind_label(source.provenance).to_owned();
    SourceCols {
        label: source.quality_label(),
        dims,
        rate,
        size,
        kind,
    }
}

/// Aligns headers and data to shared column widths.
fn header_cols(show_dims: bool, show_rate: bool, show_size: bool, show_kind: bool) -> SourceCols {
    SourceCols {
        label: "画质".into(),
        dims: if show_dims {
            "分辨率".into()
        } else {
            String::new()
        },
        rate: if show_rate {
            "码率".into()
        } else {
            String::new()
        },
        size: if show_size {
            "大小".into()
        } else {
            String::new()
        },
        kind: if show_kind {
            "类型".into()
        } else {
            String::new()
        },
    }
}

fn align_source_table(rows: &[SourceCols]) -> (String, Vec<String>) {
    if rows.is_empty() {
        return (String::new(), Vec::new());
    }
    let show_dims = rows.iter().any(|r| !r.dims.is_empty());
    let show_rate = rows.iter().any(|r| !r.rate.is_empty());
    let show_size = rows.iter().any(|r| !r.size.is_empty());
    let show_kind = rows.iter().any(|r| !r.kind.is_empty());
    let header = header_cols(show_dims, show_rate, show_size, show_kind);

    let label_w = rows
        .iter()
        .map(|r| ui::display_width(&r.label))
        .chain(std::iter::once(ui::display_width(&header.label)))
        .max()
        .unwrap_or(0);
    let dims_w = if show_dims {
        rows.iter()
            .map(|r| ui::display_width(&r.dims))
            .chain(std::iter::once(ui::display_width(&header.dims)))
            .max()
            .unwrap_or(0)
    } else {
        0
    };
    let rate_w = if show_rate {
        rows.iter()
            .map(|r| ui::display_width(&r.rate))
            .chain(std::iter::once(ui::display_width(&header.rate)))
            .max()
            .unwrap_or(0)
    } else {
        0
    };
    let size_w = if show_size {
        rows.iter()
            .map(|r| ui::display_width(&r.size))
            .chain(std::iter::once(ui::display_width(&header.size)))
            .max()
            .unwrap_or(0)
    } else {
        0
    };
    let kind_w = if show_kind {
        rows.iter()
            .map(|r| ui::display_width(&r.kind))
            .chain(std::iter::once(ui::display_width(&header.kind)))
            .max()
            .unwrap_or(0)
    } else {
        0
    };

    let format_row = |row: &SourceCols, rates_right: bool| {
        let mut parts = vec![pad_display(&row.label, label_w)];
        if dims_w > 0 {
            parts.push(pad_display(&row.dims, dims_w));
        }
        if rate_w > 0 {
            if rates_right {
                parts.push(pad_display_left(&row.rate, rate_w));
            } else {
                parts.push(pad_display(&row.rate, rate_w));
            }
        }
        if size_w > 0 {
            if rates_right {
                parts.push(pad_display_left(&row.size, size_w));
            } else {
                parts.push(pad_display(&row.size, size_w));
            }
        }
        if kind_w > 0 {
            parts.push(pad_display(&row.kind, kind_w));
        }
        parts.join(COL_SEP)
    };

    // Headers are left-aligned; numeric values are right-aligned.
    let header_line = format_row(&header, false);
    let body = rows.iter().map(|row| format_row(row, true)).collect();
    (header_line, body)
}

fn format_bitrate(bps: u64) -> String {
    if bps >= 1_000_000 {
        format!("{:.1} Mbps", bps as f64 / 1_000_000.0)
    } else if bps >= 1_000 {
        format!("{:.0} kbps", bps as f64 / 1_000.0)
    } else {
        format!("{bps} bps")
    }
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

fn run_picker(options: &[String], column_header: Option<&str>, mut mode: Mode) -> Result<Outcome> {
    let _raw = RawMode::enter().map_err(|e| Error::Config(format!("无法进入终端原始模式: {e}")))?;
    let lines = options.len() + 2 + usize::from(column_header.is_some());
    draw(&mode, options, column_header)?;

    let mut stdin = io::stdin();
    let mut buf = [0_u8; 16];
    loop {
        let n = match stdin.read(&mut buf) {
            Ok(n) => n,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(Error::Config(format!("读取键盘失败: {error}"))),
        };
        if n == 0 {
            clear_drawn(lines)?;
            return Ok(Outcome::Cancel);
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
                            redraw(&mode, options, column_header, lines)?;
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
        redraw(&mode, options, column_header, lines)?;
    }
}

fn draw(mode: &Mode, options: &[String], column_header: Option<&str>) -> Result<()> {
    let mut out = io::stdout();
    let color = ui::stdout_color();
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
    if color {
        writeln!(out, "{CYAN}{title}{RESET}").map_err(io_err)?;
        writeln!(out, "{DIM}{hint}{RESET}").map_err(io_err)?;
    } else {
        writeln!(out, "{title}").map_err(io_err)?;
        writeln!(out, "{hint}").map_err(io_err)?;
    }

    let index_width = options.len().to_string().len().max(1);
    if let Some(header) = column_header {
        let gutter = format!(
            " {} {}  {}  ",
            " ",
            " ",
            " ".repeat(index_width + 2) // width of "[N]"
        );
        if color {
            writeln!(out, "{DIM}{gutter}{header}{RESET}").map_err(io_err)?;
        } else {
            writeln!(out, "{gutter}{header}").map_err(io_err)?;
        }
    }
    for (i, label) in options.iter().enumerate() {
        let (cursor_here, on) = match mode {
            Mode::Single { cursor } => (*cursor == i, *cursor == i),
            Mode::Multi { cursor, selected } => (*cursor == i, selected[i]),
        };
        let circle = if on && color {
            format!("{GREEN}{ICON_ON}{RESET}")
        } else if on {
            ICON_ON.to_owned()
        } else if color {
            format!("{DIM}{ICON_OFF}{RESET}")
        } else {
            ICON_OFF.to_owned()
        };
        let pointer = if cursor_here && color {
            format!("{CYAN}❯{RESET}")
        } else if cursor_here {
            "❯".into()
        } else {
            " ".into()
        };
        let display_n = i + 1;
        let index = format!("[{display_n:>index_width$}]");
        writeln!(out, " {pointer} {circle}  {index}  {label}").map_err(io_err)?;
    }
    out.flush().map_err(io_err)?;
    Ok(())
}

fn redraw(
    mode: &Mode,
    options: &[String],
    column_header: Option<&str>,
    lines: usize,
) -> Result<()> {
    clear_drawn(lines)?;
    draw(mode, options, column_header)
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
        [b'q' | b'Q', ..] | [0x03 | 0x04, ..] => Key::Cancel,
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
        // Read Ctrl-C as input so Drop can restore terminal state.
        raw.c_lflag &= !(libc::ICANON | libc::ECHO | libc::ISIG);
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

#[cfg(test)]
mod tests {
    use parse_kit::{MediaSource, MediaSourceKind, VideoCodec};
    use url::Url;

    use super::{Key, align_source_table, parse_key, source_cols};

    fn sample(
        label: &str,
        w: Option<u32>,
        h: Option<u32>,
        bps: Option<u64>,
        size: Option<u64>,
        kind: MediaSourceKind,
    ) -> MediaSource {
        MediaSource {
            url: Url::parse("https://upos-sz-mirrorcos.bilivideo.com/v.m4s").unwrap(),
            codec: VideoCodec::Unknown,
            provenance: kind,
            width: w,
            height: h,
            size_hint: size,
            decode_key: None,
            label: Some(label.into()),
            bitrate_bps: bps,
        }
    }

    #[test]
    fn source_labels_align_columns() {
        let sources = [
            sample(
                "1080P/AVC",
                Some(1920),
                Some(1080),
                Some(2_600_000),
                Some(66_500_000),
                MediaSourceKind::Derived,
            ),
            sample(
                "720P/HEVC",
                Some(1280),
                Some(720),
                Some(359_000),
                None,
                MediaSourceKind::Derived,
            ),
            sample(
                "720P/AVC",
                None,
                None,
                None,
                Some(49_600_000),
                MediaSourceKind::Direct,
            ),
        ];
        let rows: Vec<_> = sources.iter().map(source_cols).collect();
        let (header, labels) = align_source_table(&rows);
        assert_eq!(labels.len(), 3);
        assert!(header.contains("画质"));
        assert!(header.contains("分辨率"));
        assert!(header.contains("码率"));
        assert!(header.contains("大小"));
        assert!(header.contains("类型"));
        let widths: Vec<_> = labels.iter().map(|l| crate::ui::display_width(l)).collect();
        assert!(widths.iter().all(|w| *w == widths[0]));
        assert_eq!(crate::ui::display_width(&header), widths[0]);
        assert!(labels[0].contains("1080P/AVC"));
        assert!(labels[0].contains("1920×1080"));
        assert!(labels[2].contains("origin"));
        assert!(!labels[0].contains('★'));
    }

    #[test]
    fn cancel_keys_allow_buffered_input_and_control_bytes() {
        assert!(matches!(parse_key(b"quit"), Key::Cancel));
        assert!(matches!(parse_key(&[0x03]), Key::Cancel));
        assert!(matches!(parse_key(&[0x04]), Key::Cancel));
    }
}
