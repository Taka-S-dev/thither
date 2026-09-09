use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;
use std::time::Duration;

use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use crossterm::terminal::{disable_raw_mode, enable_raw_mode};
use nucleo::pattern::{CaseMatching, Normalization};
use nucleo::{Config as MatchConfig, Matcher, Nucleo};
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, List, ListItem, Paragraph};
use ratatui::{Terminal, TerminalOptions, Viewport};

use crate::config::Config;
use crate::scan::{self, Entry};
use crate::{Mode, PickArgs};

type Error = Box<dyn std::error::Error>;

/// Nucleo tick budget per frame. Keeps redraws under 16 ms while a scan is running.
const TICK_MS: u64 = 10;
const POLL: Duration = Duration::from_millis(16);
/// The preview pane is dropped below this terminal width.
const MIN_WIDTH_FOR_PREVIEW: u16 = 80;
/// Share of the screen the inline picker takes, as fzf --height 40%.
const INLINE_HEIGHT_PERCENT: u32 = 40;
const INLINE_MIN_HEIGHT: u16 = 12;
/// Directory entries read for the preview. Enough to fill any pane; bounds the cost on huge folders.
const PREVIEW_LIMIT: usize = 500;

const MODE_ORDER: [Mode; 3] = [Mode::Dirs, Mode::Files, Mode::Recent];

/// One mode's candidates: its matcher and the scan that feeds it.
/// Sources are created the first time a mode is shown and kept, so switching
/// back with Tab is instant.
struct Source {
    matcher: Nucleo<Entry>,
    scan_done: Arc<AtomicBool>,
    cancel: Arc<AtomicBool>,
    scanner: Option<JoinHandle<()>>,
    selected: u32,
}

impl Source {
    fn start(mode: Mode, root: &Path, config: &Config) -> Self {
        let matcher = Nucleo::new(MatchConfig::DEFAULT.match_paths(), Arc::new(|| {}), None, 1);
        let scan_done = Arc::new(AtomicBool::new(false));
        let cancel = Arc::new(AtomicBool::new(false));
        let scanner = scan::spawn(
            root.to_path_buf(),
            mode,
            &config.exclude,
            matcher.injector(),
            scan_done.clone(),
            cancel.clone(),
        );
        Self {
            matcher,
            scan_done,
            cancel,
            scanner: Some(scanner),
            selected: 0,
        }
    }

    fn set_query(&mut self, query: &str, append: bool) {
        self.matcher
            .pattern
            .reparse(0, query, CaseMatching::Ignore, Normalization::Smart, append);
        self.selected = 0;
    }

    fn selected_entry(&self) -> Option<Entry> {
        let item = self.matcher.snapshot().get_matched_item(self.selected)?;
        Some(Entry {
            path: item.data.path.clone(),
            display: item.data.display.clone(),
        })
    }

    fn clamp_selection(&mut self) {
        let count = self.matcher.snapshot().matched_item_count();
        if count == 0 {
            self.selected = 0;
        } else if self.selected >= count {
            self.selected = count - 1;
        }
    }
}

impl Drop for Source {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
        if let Some(handle) = self.scanner.take() {
            let _ = handle.join();
        }
    }
}

struct Picker {
    sources: [Option<Source>; 3],
    mode: Mode,
    query: String,
    root: PathBuf,
    config: Config,
    /// Recomputes match positions for the visible rows only.
    highlighter: Matcher,
    /// Directory listing shown on the right, keyed by the path it was read from.
    preview: Option<(PathBuf, Vec<String>)>,
    /// Drives the spinner.
    frame_count: u32,
}

enum Action {
    Continue,
    Accept,
    Cancel,
}

pub fn run(args: PickArgs, root: PathBuf, config: Config) -> Result<Option<PathBuf>, Error> {
    if args.mode == Mode::Recent {
        // Fail with a message now rather than showing an empty picker.
        scan::recent()?;
    }
    let mut picker = Picker {
        sources: [None, None, None],
        mode: args.mode,
        query: String::new(),
        root,
        config,
        highlighter: Matcher::new(MatchConfig::DEFAULT.match_paths()),
        preview: None,
        frame_count: 0,
    };
    picker.set_query(&args.query);

    if args.select_1 {
        let source = picker.source();
        if let Some(handle) = source.scanner.take() {
            handle.join().map_err(|_| "scan thread panicked")?;
        }
        while source.matcher.tick(TICK_MS).running {}
        if source.matcher.snapshot().matched_item_count() == 1
            && let Some(entry) = source.selected_entry()
        {
            return Ok(Some(output_path(&entry, args.mode)));
        }
    }

    let result = picker.run_tui();
    let mode = picker.mode;
    drop(picker);
    result.map(|entry| entry.map(|e| output_path(&e, mode)))
}

/// Files mode outputs the parent directory: the point is to cd there.
fn output_path(entry: &Entry, mode: Mode) -> PathBuf {
    match mode {
        Mode::Files => entry
            .path
            .parent()
            .map(PathBuf::from)
            .unwrap_or_else(|| entry.path.clone()),
        _ => entry.path.clone(),
    }
}

fn mode_index(mode: Mode) -> usize {
    MODE_ORDER
        .iter()
        .position(|&m| m == mode)
        .expect("known mode")
}

impl Picker {
    /// The current mode's source, started on first use.
    fn source(&mut self) -> &mut Source {
        let idx = mode_index(self.mode);
        if self.sources[idx].is_none() {
            let mut source = Source::start(self.mode, &self.root, &self.config);
            source.set_query(&self.query, false);
            self.sources[idx] = Some(source);
        }
        self.sources[idx].as_mut().expect("just created")
    }

    fn set_query(&mut self, query: &str) {
        let append = query.starts_with(&self.query);
        self.query = query.to_string();
        let q = self.query.clone();
        self.source().set_query(&q, append);
    }

    fn switch_mode(&mut self, step: isize) {
        let idx = mode_index(self.mode) as isize + step;
        let len = MODE_ORDER.len() as isize;
        self.mode = MODE_ORDER[idx.rem_euclid(len) as usize];
        self.source().selected = 0;
    }

    fn run_tui(&mut self) -> Result<Option<Entry>, Error> {
        let mut terminal = TerminalGuard::enter()?;
        loop {
            let source = self.source();
            source.matcher.tick(TICK_MS);
            source.clamp_selection();
            terminal.draw(|frame| self.render(frame.area(), frame))?;

            if !event::poll(POLL)? {
                continue;
            }
            let Event::Key(key) = event::read()? else {
                continue;
            };
            if key.kind == KeyEventKind::Release {
                continue;
            }
            match self.handle_key(key) {
                Action::Continue => {}
                Action::Cancel => return Ok(None),
                Action::Accept => {
                    if let Some(entry) = self.source().selected_entry() {
                        return Ok(Some(entry));
                    }
                }
            }
        }
    }

    fn handle_key(&mut self, key: KeyEvent) -> Action {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        match (key.code, ctrl) {
            (KeyCode::Esc, _) | (KeyCode::Char('c'), true) => Action::Cancel,
            (KeyCode::Enter, _) => Action::Accept,
            (KeyCode::Tab, _) => {
                self.switch_mode(1);
                Action::Continue
            }
            (KeyCode::BackTab, _) => {
                self.switch_mode(-1);
                Action::Continue
            }
            (KeyCode::Up, _) | (KeyCode::Char('k'), true) => {
                let s = self.source();
                s.selected = s.selected.saturating_sub(1);
                Action::Continue
            }
            (KeyCode::Down, _) | (KeyCode::Char('j'), true) => {
                let s = self.source();
                s.selected = s.selected.saturating_add(1);
                Action::Continue
            }
            (KeyCode::Backspace, _) => {
                let mut q = self.query.clone();
                q.pop();
                self.set_query(&q);
                Action::Continue
            }
            (KeyCode::Char('u'), true) => {
                self.set_query("");
                Action::Continue
            }
            (KeyCode::Char(c), false) => {
                let mut q = self.query.clone();
                q.push(c);
                self.set_query(&q);
                Action::Continue
            }
            _ => Action::Continue,
        }
    }

    fn render(&mut self, area: Rect, frame: &mut ratatui::Frame) {
        let mode = self.mode;
        let show_preview = area.width >= MIN_WIDTH_FOR_PREVIEW;
        let [list_area, preview_area] = if show_preview {
            Layout::horizontal([Constraint::Percentage(60), Constraint::Percentage(40)]).areas(area)
        } else {
            [area, Rect::default()]
        };

        let selected = self.source().selected_entry();
        if show_preview {
            let dir = selected
                .as_ref()
                .map(|e| output_path(e, mode))
                .filter(|p| p.is_dir());
            self.render_preview(preview_area, frame, dir.as_deref());
        }

        // Header: mode tabs, then where the candidates come from.
        let mut header: Vec<Span> = vec![Span::styled("[", theme::HEADER)];
        for (i, &m) in MODE_ORDER.iter().enumerate() {
            if i > 0 {
                header.push(Span::styled("|", theme::HEADER));
            }
            let style = if m == mode {
                theme::HEADER.add_modifier(Modifier::BOLD | Modifier::UNDERLINED)
            } else {
                theme::HEADER
            };
            header.push(Span::styled(m.label(), style));
        }
        header.push(Span::styled("] ", theme::HEADER));
        let location = match mode {
            Mode::Recent => "zoxide".to_string(),
            _ => self.root.display().to_string(),
        };
        header.push(Span::styled(location, theme::HEADER));

        let source = self.sources[mode_index(mode)]
            .as_mut()
            .expect("current source");
        let snapshot = source.matcher.snapshot();
        let count = snapshot.matched_item_count();
        let total = snapshot.item_count();
        let scanning = !source.scan_done.load(Ordering::Acquire);

        let block = Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(theme::BORDER)
            .title_bottom(
                Line::from(Span::styled(
                    " Tab: mode  Enter: cd  Esc: cancel ",
                    theme::BORDER,
                ))
                .right_aligned(),
            );
        let inner = block.inner(list_area);
        frame.render_widget(block, list_area);

        // Same vertical order as fzf --layout=reverse: prompt, info, header, list.
        let [prompt_area, info_area, header_area, rows_area] = Layout::vertical([
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Min(0),
        ])
        .areas(inner);

        let prompt = Paragraph::new(Line::from(vec![
            Span::styled("> ", theme::PROMPT),
            Span::raw(&self.query),
        ]));
        frame.render_widget(prompt, prompt_area);
        frame.set_cursor_position((
            prompt_area.x + 2 + self.query.chars().count() as u16,
            prompt_area.y,
        ));

        let spinner = if scanning {
            self.frame_count = self.frame_count.wrapping_add(1);
            SPINNER[(self.frame_count / 4) as usize % SPINNER.len()]
        } else {
            ' '
        };
        let counts = format!("{spinner} {count}/{total} ");
        let rule_width = (info_area.width as usize).saturating_sub(counts.chars().count());
        let info = Paragraph::new(Line::from(vec![
            Span::styled(counts, theme::INFO),
            Span::styled("─".repeat(rule_width), theme::BORDER),
        ]));
        frame.render_widget(info, info_area);
        frame.render_widget(Paragraph::new(Line::from(header)), header_area);

        let height = rows_area.height as u32;
        if height == 0 || count == 0 {
            return;
        }
        let selected = source.selected;
        let first = selected
            .saturating_sub(height - 1)
            .min(count.saturating_sub(height));
        let last = (first + height).min(count);
        let pattern = snapshot.pattern().column_pattern(0);
        let mut indices = Vec::new();
        let items: Vec<ListItem> = snapshot
            .matched_items(first..last)
            .enumerate()
            .map(|(i, item)| {
                let idx = first + i as u32;
                indices.clear();
                pattern.indices(
                    item.matcher_columns[0].slice(..),
                    &mut self.highlighter,
                    &mut indices,
                );
                indices.sort_unstable();
                indices.dedup();
                let current = idx == selected;
                let mut line = highlight_line(current, &item.data.display, &indices);
                if current {
                    line = line.style(theme::CURRENT);
                }
                ListItem::new(line)
            })
            .collect();
        frame.render_widget(List::new(items), rows_area);
    }

    /// Lists the directory the current selection would cd into.
    fn render_preview(&mut self, area: Rect, frame: &mut ratatui::Frame, dir: Option<&Path>) {
        let title = match dir {
            Some(d) => {
                let shown = d.strip_prefix(&self.root).unwrap_or(d);
                let shown = shown.display().to_string();
                format!(" {} ", if shown.is_empty() { "." } else { &shown })
            }
            None => String::new(),
        };
        let block = Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(theme::BORDER)
            .title(Span::styled(title, theme::HEADER));
        let inner = block.inner(area);
        frame.render_widget(block, area);

        let Some(dir) = dir else {
            self.preview = None;
            return;
        };
        let cached = self.preview.as_ref().is_some_and(|(p, _)| p == dir);
        if !cached {
            self.preview = Some((dir.to_path_buf(), list_dir(dir)));
        }
        let Some((_, names)) = &self.preview else {
            return;
        };
        let items: Vec<ListItem> = names
            .iter()
            .take(inner.height as usize)
            .map(|n| {
                let style = if n.ends_with(std::path::MAIN_SEPARATOR) {
                    Style::default()
                        .fg(Color::Blue)
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default()
                };
                ListItem::new(Span::styled(n.as_str(), style))
            })
            .collect();
        frame.render_widget(List::new(items), inner);
    }
}

/// Directory names first (with a trailing separator), then files, both sorted case-insensitively.
fn list_dir(dir: &Path) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return vec!["(unreadable)".to_string()];
    };
    let mut dirs = Vec::new();
    let mut files = Vec::new();
    for entry in entries.flatten().take(PREVIEW_LIMIT) {
        let name = entry.file_name().to_string_lossy().into_owned();
        if entry.file_type().is_ok_and(|t| t.is_dir()) {
            dirs.push(format!("{name}{}", std::path::MAIN_SEPARATOR));
        } else {
            files.push(name);
        }
    }
    let key = |s: &String| s.to_lowercase();
    dirs.sort_by_key(key);
    files.sort_by_key(key);
    dirs.extend(files);
    dirs
}

/// Colours from fzf's default dark theme, by 256-colour index, so the picker
/// looks like the fzf-based commands it replaces.
mod theme {
    use ratatui::style::{Color, Modifier, Style};

    pub const BORDER: Style = Style::new().fg(Color::Indexed(240));
    pub const PROMPT: Style = Style::new().fg(Color::Indexed(110));
    pub const INFO: Style = Style::new().fg(Color::Indexed(144));
    pub const HEADER: Style = Style::new().fg(Color::Indexed(109));
    pub const POINTER: Style = Style::new().fg(Color::Indexed(161));
    pub const MATCH: Style = Style::new().fg(Color::Indexed(108));
    pub const CURRENT: Style = Style::new()
        .fg(Color::Indexed(255))
        .bg(Color::Indexed(236))
        .add_modifier(Modifier::BOLD);
}

/// Shown next to the counts while a scan is still feeding the list.
const SPINNER: [char; 4] = ['|', '/', '-', '\\'];

/// Builds one row, colouring the characters at `indices` (sorted, char positions).
/// The current row gets fzf's pointer bar in the gutter.
fn highlight_line<'a>(current: bool, text: &'a str, indices: &[u32]) -> Line<'a> {
    let matched = theme::MATCH;
    let pointer = if current {
        Span::styled("▌ ", theme::POINTER)
    } else {
        Span::raw("  ")
    };
    let mut spans = vec![pointer];
    let mut next = indices.iter().peekable();
    let mut run_start = 0;
    let mut run_hit = false;
    for (pos, (byte, _)) in text.char_indices().enumerate() {
        let hit = next.peek().is_some_and(|&&i| i as usize == pos);
        if hit {
            next.next();
        }
        if hit != run_hit && byte > run_start {
            let style = if run_hit { matched } else { Style::default() };
            spans.push(Span::styled(&text[run_start..byte], style));
            run_start = byte;
        }
        run_hit = hit;
    }
    if run_start < text.len() {
        let style = if run_hit { matched } else { Style::default() };
        spans.push(Span::styled(&text[run_start..], style));
    }
    Line::from(spans)
}

/// The picker draws on stderr so stdout stays clean for the selected path.
struct TerminalGuard {
    terminal: Terminal<CrosstermBackend<io::Stderr>>,
}

impl TerminalGuard {
    /// Opens an inline viewport at the bottom of the screen, like fzf --height,
    /// so the command history above stays visible.
    fn enter() -> Result<Self, Error> {
        let (_, rows) = crossterm::terminal::size()?;
        let height = (u32::from(rows) * INLINE_HEIGHT_PERCENT / 100) as u16;
        let height = height.clamp(INLINE_MIN_HEIGHT, rows.max(1));
        enable_raw_mode()?;
        let terminal = Terminal::with_options(
            CrosstermBackend::new(io::stderr()),
            TerminalOptions {
                viewport: Viewport::Inline(height),
            },
        );
        match terminal {
            Ok(terminal) => Ok(Self { terminal }),
            Err(err) => {
                let _ = disable_raw_mode();
                Err(err.into())
            }
        }
    }
}

impl std::ops::Deref for TerminalGuard {
    type Target = Terminal<CrosstermBackend<io::Stderr>>;
    fn deref(&self) -> &Self::Target {
        &self.terminal
    }
}

impl std::ops::DerefMut for TerminalGuard {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.terminal
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        // Wipe the viewport so the prompt comes back where the picker opened.
        let _ = self.terminal.clear();
        let _ = disable_raw_mode();
        let _ = io::stderr().flush();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pieces(line: &Line) -> Vec<(String, bool)> {
        line.spans
            .iter()
            .map(|s| {
                (
                    s.content.to_string(),
                    s.style.fg == Some(Color::Indexed(108)),
                )
            })
            .collect()
    }

    #[test]
    fn highlights_runs_of_matched_chars() {
        let line = highlight_line(true, "openssl", &[0, 1, 2]);
        assert_eq!(
            pieces(&line),
            vec![
                ("▌ ".into(), false),
                ("ope".into(), true),
                ("nssl".into(), false)
            ]
        );
    }

    #[test]
    fn highlights_scattered_and_multibyte_chars() {
        let line = highlight_line(false, r"ドキュメント\src", &[1, 7]);
        assert_eq!(
            pieces(&line),
            vec![
                ("  ".into(), false),
                ("ド".into(), false),
                ("キ".into(), true),
                ("ュメント\\".into(), false),
                ("s".into(), true),
                ("rc".into(), false)
            ]
        );
    }

    #[test]
    fn no_indices_means_plain_text() {
        let line = highlight_line(false, "docs", &[]);
        assert_eq!(
            pieces(&line),
            vec![("  ".into(), false), ("docs".into(), false)]
        );
    }

    #[test]
    fn mode_order_wraps_both_ways() {
        assert_eq!(mode_index(Mode::Dirs), 0);
        assert_eq!(mode_index(Mode::Recent), 2);
        let next = |m: Mode, step: isize| {
            let i = mode_index(m) as isize + step;
            MODE_ORDER[i.rem_euclid(MODE_ORDER.len() as isize) as usize]
        };
        assert_eq!(next(Mode::Recent, 1), Mode::Dirs);
        assert_eq!(next(Mode::Dirs, -1), Mode::Recent);
    }

    #[test]
    fn list_dir_puts_directories_first() {
        let tmp = std::env::temp_dir().join(format!("navkit-preview-{}", std::process::id()));
        std::fs::create_dir_all(tmp.join("zeta")).unwrap();
        std::fs::write(tmp.join("Alpha.txt"), "").unwrap();
        std::fs::write(tmp.join("beta.txt"), "").unwrap();
        let names = list_dir(&tmp);
        std::fs::remove_dir_all(&tmp).unwrap();
        let sep = std::path::MAIN_SEPARATOR;
        assert_eq!(
            names,
            vec![
                format!("zeta{sep}"),
                "Alpha.txt".to_string(),
                "beta.txt".to_string()
            ]
        );
    }
}
