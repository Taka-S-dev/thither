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

use crate::browse::{self, Browser};
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

const MODE_ORDER: [Mode; 4] = [Mode::Dirs, Mode::Files, Mode::Recent, Mode::Browse];

/// One mode's candidates: its matcher and the scan that feeds it.
/// Sources are created the first time a mode is shown and kept, so switching
/// back with Tab is instant.
struct Source {
    mode: Mode,
    matcher: Nucleo<Entry>,
    scan_done: Arc<AtomicBool>,
    cancel: Arc<AtomicBool>,
    scanner: Option<JoinHandle<()>>,
    selected: u32,
    query_empty: bool,
    /// Item indices in browse order (shallow paths first, then by name), built
    /// once the scan has finished. With no query nucleo lists items in the order
    /// the parallel walk found them, which is noise to a reader.
    browse_order: Option<Vec<u32>>,
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
            mode,
            matcher,
            scan_done,
            cancel,
            scanner: Some(scanner),
            selected: 0,
            query_empty: true,
            browse_order: None,
        }
    }

    /// Browse order applies to scanned modes with an empty query. zoxide's own
    /// order (by score) is the point of recent mode, so it is left alone.
    fn browsing(&self) -> bool {
        self.query_empty && self.mode != Mode::Recent
    }

    /// Builds the browse order once every scanned item is in the snapshot.
    fn refresh_browse_order(&mut self) {
        if self.browse_order.is_some() || !self.scan_done.load(Ordering::Acquire) {
            return;
        }
        let snapshot = self.matcher.snapshot();
        if snapshot.item_count() != self.matcher.injector().injected_items() {
            return;
        }
        let mut order: Vec<u32> = (0..snapshot.item_count()).collect();
        order.sort_by_cached_key(|&i| {
            let display = snapshot
                .get_item(i)
                .map(|item| item.data.display.as_str())
                .unwrap_or_default();
            let depth = display.matches(std::path::MAIN_SEPARATOR).count();
            (depth, display.to_lowercase())
        });
        self.browse_order = Some(order);
    }

    /// The nth row as shown: browse order when browsing, nucleo's ranking otherwise.
    fn visible(&self, n: u32) -> Option<nucleo::Item<'_, Entry>> {
        let snapshot = self.matcher.snapshot();
        match (&self.browse_order, self.browsing()) {
            (Some(order), true) => snapshot.get_item(*order.get(n as usize)?),
            _ => snapshot.get_matched_item(n),
        }
    }

    fn set_query(&mut self, query: &str, append: bool) {
        self.matcher
            .pattern
            .reparse(0, query, CaseMatching::Ignore, Normalization::Smart, append);
        self.query_empty = query.is_empty();
        self.selected = 0;
    }

    fn selected_entry(&self) -> Option<Entry> {
        let item = self.visible(self.selected)?;
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
    /// One per scanned mode (dirs, files, recent). The browse slot stays empty.
    sources: [Option<Source>; 4],
    /// State of browse mode, created when the mode is first shown.
    browser: Option<Browser>,
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
        sources: [None, None, None, None],
        browser: (args.mode == Mode::Browse).then(|| Browser::new(root.clone())),
        mode: args.mode,
        query: String::new(),
        root,
        config,
        highlighter: Matcher::new(MatchConfig::DEFAULT.match_paths()),
        preview: None,
        frame_count: 0,
    };
    picker.set_query(&args.query);

    if args.select_1 && args.mode != Mode::Browse {
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
        if self.mode == Mode::Browse {
            self.browser().set_filter(query);
            return;
        }
        let append = query.starts_with(&self.query);
        self.query = query.to_string();
        let q = self.query.clone();
        self.source().set_query(&q, append);
    }

    fn browser(&mut self) -> &mut Browser {
        let root = self.root.clone();
        self.browser.get_or_insert_with(|| Browser::new(root))
    }

    /// Moves the scan root. Every scanned mode restarts from the new place.
    fn set_root(&mut self, root: PathBuf) {
        if root == self.root {
            return;
        }
        self.root = root;
        self.sources = [None, None, None, None];
        self.preview = None;
    }

    /// Tab order is dirs, files, recent, browse. Browse mode starts from the
    /// directory selected in the previous mode, and the scanned modes pick up
    /// wherever browsing ended, so the two ways of looking share one place.
    fn switch_mode(&mut self, step: isize) {
        let leaving = self.mode;
        let idx = mode_index(leaving) as isize + step;
        let len = MODE_ORDER.len() as isize;
        let entering = MODE_ORDER[idx.rem_euclid(len) as usize];

        if leaving == Mode::Browse {
            let cwd = self.browser().cwd.clone();
            self.set_root(cwd);
        }
        self.mode = entering;
        if entering == Mode::Browse {
            let start = self.sources[mode_index(leaving)]
                .as_ref()
                .and_then(Source::selected_entry)
                .map(|e| output_path(&e, leaving))
                .filter(|p| p.is_dir())
                .unwrap_or_else(|| self.root.clone());
            self.browser = Some(Browser::new(start));
        } else {
            self.source().selected = 0;
        }
    }

    fn run_tui(&mut self) -> Result<Option<Entry>, Error> {
        let mut terminal = TerminalGuard::enter()?;
        loop {
            if self.mode != Mode::Browse {
                let source = self.source();
                source.matcher.tick(TICK_MS);
                source.refresh_browse_order();
                source.clamp_selection();
            }
            terminal.draw(|frame| self.render(frame.area(), frame))?;

            if !event::poll(POLL)? {
                continue;
            }
            let key = match event::read()? {
                Event::Key(key) => key,
                Event::Resize(_, rows) => {
                    terminal.reopen(rows)?;
                    continue;
                }
                _ => continue,
            };
            if key.kind == KeyEventKind::Release {
                continue;
            }
            match self.handle_key(key) {
                Action::Continue => {}
                Action::Cancel => return Ok(None),
                Action::Accept => {
                    if self.mode == Mode::Browse {
                        let path = self.browser().target();
                        let display = path.display().to_string();
                        return Ok(Some(Entry { path, display }));
                    }
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
            (KeyCode::Esc, _) | (KeyCode::Char('c'), true) => return Action::Cancel,
            (KeyCode::Enter, _) => return Action::Accept,
            (KeyCode::Tab, _) => {
                self.switch_mode(1);
                return Action::Continue;
            }
            (KeyCode::BackTab, _) => {
                self.switch_mode(-1);
                return Action::Continue;
            }
            _ => {}
        }
        if self.mode == Mode::Browse {
            return self.handle_browse_key(key);
        }
        match (key.code, ctrl) {
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

    fn handle_browse_key(&mut self, key: KeyEvent) -> Action {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let b = self.browser();
        match (key.code, ctrl) {
            (KeyCode::Left, _) | (KeyCode::Char('h'), true) => b.up(),
            (KeyCode::Right, _) | (KeyCode::Char('l'), true) => b.enter(),
            (KeyCode::Up, _) | (KeyCode::Char('k'), true) => b.move_selection(-1),
            (KeyCode::Down, _) | (KeyCode::Char('j'), true) => b.move_selection(1),
            (KeyCode::PageUp, _) => b.move_selection(-10),
            (KeyCode::PageDown, _) => b.move_selection(10),
            // Backspace on an empty filter climbs, as in yazi.
            (KeyCode::Backspace, _) => {
                if b.filter.is_empty() {
                    b.up();
                } else {
                    let mut f = b.filter.clone();
                    f.pop();
                    b.set_filter(&f);
                }
            }
            (KeyCode::Char('u'), true) => b.set_filter(""),
            (KeyCode::Char(c), false) => {
                let mut f = b.filter.clone();
                f.push(c);
                b.set_filter(&f);
            }
            _ => {}
        }
        Action::Continue
    }

    fn render(&mut self, area: Rect, frame: &mut ratatui::Frame) {
        if self.mode == Mode::Browse {
            self.render_browse(area, frame);
            return;
        }
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

        let location = match mode {
            Mode::Recent => "zoxide".to_string(),
            _ => self.root.display().to_string(),
        };
        let header = header_line(mode, location);

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
        let items: Vec<ListItem> = (first..last)
            .filter_map(|idx| source.visible(idx).map(|item| (idx, item)))
            .map(|(idx, item)| {
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

    /// Three columns like yazi: parent, current directory, selected entry's contents.
    fn render_browse(&mut self, area: Rect, frame: &mut ratatui::Frame) {
        let block = Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(theme::BORDER)
            .title_bottom(
                Line::from(Span::styled(
                    " Left: up  Right: enter  Tab: mode  Enter: cd  Esc: cancel ",
                    theme::BORDER,
                ))
                .right_aligned(),
            );
        let inner = block.inner(area);
        frame.render_widget(block, area);

        let [prompt_area, info_area, header_area, columns_area] = Layout::vertical([
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Min(0),
        ])
        .areas(inner);

        let root = self.root.clone();
        let browser = self.browser.get_or_insert_with(|| Browser::new(root));
        let filter = browser.filter.clone();
        let prompt = Paragraph::new(Line::from(vec![
            Span::styled("> ", theme::PROMPT),
            Span::raw(filter.as_str()),
        ]));
        frame.render_widget(prompt, prompt_area);
        frame.set_cursor_position((
            prompt_area.x + 2 + filter.chars().count() as u16,
            prompt_area.y,
        ));

        let shown = browser.rows().len();
        let total = browser.items().len();
        let counts = format!("  {shown}/{total} ");
        let rule_width = (info_area.width as usize).saturating_sub(counts.chars().count());
        let info = Paragraph::new(Line::from(vec![
            Span::styled(counts, theme::INFO),
            Span::styled("─".repeat(rule_width), theme::BORDER),
        ]));
        frame.render_widget(info, info_area);
        let header = header_line(Mode::Browse, browser.cwd.display().to_string());
        frame.render_widget(Paragraph::new(Line::from(header)), header_area);

        let [parent_area, sep1, current_area, sep2, preview_area] = Layout::horizontal([
            Constraint::Percentage(25),
            Constraint::Length(1),
            Constraint::Percentage(40),
            Constraint::Length(1),
            Constraint::Min(0),
        ])
        .areas(columns_area);
        for sep in [sep1, sep2] {
            let bar: Vec<Line> = (0..sep.height)
                .map(|_| Line::from(Span::styled("│", theme::BORDER)))
                .collect();
            frame.render_widget(Paragraph::new(bar), sep);
        }

        // Parent column: the directory we are in is marked.
        if let Some((items, here)) = browser.parent_listing() {
            let height = parent_area.height as usize;
            let first = here
                .unwrap_or(0)
                .saturating_sub(height.saturating_sub(1))
                .min(items.len().saturating_sub(height));
            let rows: Vec<ListItem> = items
                .iter()
                .enumerate()
                .skip(first)
                .take(height)
                .map(|(i, item)| {
                    let current = Some(i) == here;
                    let mut line = highlight_line(current, &item.label(), &[]);
                    if current {
                        line = line.style(theme::CURRENT);
                    } else if item.is_dir {
                        line = line.style(theme::DIR);
                    }
                    ListItem::new(line)
                })
                .collect();
            frame.render_widget(List::new(rows), parent_area);
        }

        // Current column: filtered rows with match highlights.
        let height = current_area.height as usize;
        let selected = browser.selected;
        let first = selected
            .saturating_sub(height.saturating_sub(1))
            .min(shown.saturating_sub(height));
        let rows: Vec<ListItem> = browser
            .rows()
            .iter()
            .enumerate()
            .skip(first)
            .take(height)
            .map(|(i, row)| {
                let item = &browser.items()[row.index];
                let current = i == selected;
                let label = item.label();
                let mut line = highlight_line(current, &label, &row.hits);
                if current {
                    line = line.style(theme::CURRENT);
                } else if item.is_dir {
                    line = line.style(theme::DIR);
                }
                ListItem::new(line)
            })
            .collect();
        frame.render_widget(List::new(rows), current_area);

        // Preview column: contents of the selected directory.
        let dir = browser.selected_path().filter(|p| p.is_dir());
        if let Some(dir) = dir {
            let cached = self.preview.as_ref().is_some_and(|(p, _)| p == &dir);
            if !cached {
                self.preview = Some((dir.clone(), list_dir(&dir)));
            }
            if let Some((_, names)) = &self.preview {
                let rows: Vec<ListItem> = names
                    .iter()
                    .take(preview_area.height as usize)
                    .map(|n| {
                        let style = if n.ends_with(std::path::MAIN_SEPARATOR) {
                            theme::DIR
                        } else {
                            Style::default()
                        };
                        ListItem::new(Line::from(Span::styled(format!(" {n}"), style)))
                    })
                    .collect();
                frame.render_widget(List::new(rows), preview_area);
            }
        } else {
            self.preview = None;
        }
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

/// `[dirs|files|recent|browse] <location>` with the active mode underlined.
fn header_line(mode: Mode, location: String) -> Vec<Span<'static>> {
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
    header.push(Span::styled(location, theme::HEADER));
    header
}

/// Directory names first (with a trailing separator), then files, both sorted case-insensitively.
fn list_dir(dir: &Path) -> Vec<String> {
    if std::fs::read_dir(dir).is_err() {
        return vec!["(unreadable)".to_string()];
    }
    browse::read_dir(dir)
        .iter()
        .take(PREVIEW_LIMIT)
        .map(browse::Item::label)
        .collect()
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
    pub const DIR: Style = Style::new().fg(Color::Blue);
    pub const CURRENT: Style = Style::new()
        .fg(Color::Indexed(255))
        .bg(Color::Indexed(236))
        .add_modifier(Modifier::BOLD);
}

/// Shown next to the counts while a scan is still feeding the list.
const SPINNER: [char; 4] = ['|', '/', '-', '\\'];

/// Builds one row, colouring the characters at `indices` (sorted, char positions).
/// The current row gets fzf's pointer bar in the gutter.
fn highlight_line(current: bool, text: &str, indices: &[u32]) -> Line<'static> {
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
            spans.push(Span::styled(text[run_start..byte].to_string(), style));
            run_start = byte;
        }
        run_hit = hit;
    }
    if run_start < text.len() {
        let style = if run_hit { matched } else { Style::default() };
        spans.push(Span::styled(text[run_start..].to_string(), style));
    }
    Line::from(spans)
}

/// Everything from the cursor row to the bottom of the screen, so a fresh
/// window is filled instead of leaving the lower part empty. Near the bottom
/// the picker still takes at least 40% (and the minimum), scrolling the
/// history up as fzf --height does.
fn inline_height(rows: u16, cursor_row: u16) -> u16 {
    let below_cursor = rows.saturating_sub(cursor_row);
    let floor = (u32::from(rows) * INLINE_HEIGHT_PERCENT / 100) as u16;
    below_cursor
        .max(floor)
        .max(INLINE_MIN_HEIGHT)
        .min(rows.max(1))
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
        let (_, cursor_row) = crossterm::cursor::position().unwrap_or((0, rows));
        enable_raw_mode()?;
        match Self::open(rows, cursor_row) {
            Ok(terminal) => Ok(Self { terminal }),
            Err(err) => {
                let _ = disable_raw_mode();
                Err(err)
            }
        }
    }

    /// Creates an inline viewport starting at `top`, sized for a `rows`-tall screen.
    fn open(rows: u16, top: u16) -> Result<Terminal<CrosstermBackend<io::Stderr>>, Error> {
        let height = inline_height(rows, top);
        let terminal = Terminal::with_options(
            CrosstermBackend::new(io::stderr()),
            TerminalOptions {
                viewport: Viewport::Inline(height),
            },
        )?;
        Ok(terminal)
    }

    /// Rebuilds the viewport after the window changed size. An inline viewport's
    /// height is fixed when it is created, so growing the window would otherwise
    /// leave the extra rows unused, and shrinking it would draw off screen.
    fn reopen(&mut self, rows: u16) -> Result<(), Error> {
        let top = self.terminal.get_frame().area().y;
        let top = top.min(rows.saturating_sub(1));
        self.terminal.clear()?;
        crossterm::execute!(io::stderr(), crossterm::cursor::MoveTo(0, top))?;
        self.terminal = Self::open(rows, top)?;
        Ok(())
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
    fn inline_height_fills_the_space_below_the_cursor() {
        assert_eq!(inline_height(50, 0), 50);
        assert_eq!(inline_height(50, 10), 40);
        assert_eq!(inline_height(50, 45), 20);
        assert_eq!(inline_height(20, 19), INLINE_MIN_HEIGHT);
        assert_eq!(inline_height(10, 9), 10);
        assert_eq!(inline_height(0, 0), 1);
    }

    #[test]
    fn mode_order_wraps_both_ways() {
        assert_eq!(mode_index(Mode::Dirs), 0);
        assert_eq!(mode_index(Mode::Browse), 3);
        let next = |m: Mode, step: isize| {
            let i = mode_index(m) as isize + step;
            MODE_ORDER[i.rem_euclid(MODE_ORDER.len() as isize) as usize]
        };
        assert_eq!(next(Mode::Recent, 1), Mode::Browse);
        assert_eq!(next(Mode::Browse, 1), Mode::Dirs);
        assert_eq!(next(Mode::Dirs, -1), Mode::Browse);
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
