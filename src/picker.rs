use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;
use std::time::Duration;

use crossterm::event::{
    self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEvent, KeyEventKind,
    KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use crossterm::terminal::{disable_raw_mode, enable_raw_mode};
use nucleo::pattern::{CaseMatching, Normalization};
use nucleo::{Config as MatchConfig, Matcher, Nucleo};
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, List, ListItem, Paragraph, Wrap};
use ratatui::{Terminal, TerminalOptions, Viewport};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use crate::browse::{self, Browser};
use crate::config::Config;
use crate::icons;
use crate::open;
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

const MODE_ORDER: [Mode; 5] = [
    Mode::Dirs,
    Mode::Files,
    Mode::Recent,
    Mode::Favorites,
    Mode::Browse,
];

/// One mode's candidates: its matcher and the scan that feeds it.
/// Sources are created the first time a mode is shown and kept, so switching
/// back with Tab is instant.
struct Source {
    mode: Mode,
    matcher: Nucleo<Entry>,
    scan_done: Arc<AtomicBool>,
    cancel: Arc<AtomicBool>,
    scanner: Option<JoinHandle<Result<(), String>>>,
    scan_error: Option<String>,
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
            scan_error: None,
            selected: 0,
            query_empty: true,
            browse_order: None,
        }
    }

    /// Browse order applies to scanned modes with an empty query. zoxide's own
    /// order (by score) is the point of recent mode, so it is left alone.
    fn browsing(&self) -> bool {
        self.query_empty && !matches!(self.mode, Mode::Recent | Mode::Favorites)
    }

    /// Builds the browse order once every scanned item is in the snapshot.
    fn refresh_browse_order(&mut self) {
        if matches!(self.mode, Mode::Recent | Mode::Favorites)
            || self.browse_order.is_some()
            || !self.scan_done.load(Ordering::Acquire)
        {
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

    /// Collect only completed workers, keeping slow history queries off the UI thread.
    fn collect_scan_result(&mut self) {
        if self
            .scanner
            .as_ref()
            .is_some_and(|handle| handle.is_finished())
        {
            self.finish_scan();
        }
    }

    fn finish_scan(&mut self) {
        if let Some(handle) = self.scanner.take() {
            self.scan_error = handle
                .join()
                .unwrap_or_else(|_| Err("scan thread panicked".into()))
                .err();
            self.scan_done.store(true, Ordering::Release);
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
    /// One per candidate source; the browse slot stays empty.
    sources: [Option<Source>; MODE_ORDER.len()],
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
    notice: Option<String>,
    pinned: crate::favorites::Index,
    menu: Option<crate::action_menu::Menu>,
    mouse_rows: (Rect, usize, usize),
    mouse_header: Rect,
    /// Clickable areas of the navigation buttons, empty outside browse mode.
    mouse_nav: Vec<(Rect, Nav)>,
    /// Column where the mode tabs start, which the buttons push to the right.
    mouse_modes_x: u16,
    mouse_paths: Vec<(Rect, PathBuf)>,
    last_click: Option<(PathBuf, u16, u16, std::time::Instant)>,
}

/// The buttons drawn at the head of the browse header, mirroring Alt-Left,
/// Alt-Right and Left so the same moves are reachable without the keyboard.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Nav {
    Back,
    Forward,
    Up,
}

impl Nav {
    /// Glyph and the width it is drawn in, padding included.
    const BUTTONS: [(Nav, &'static str); 3] =
        [(Nav::Back, " ◀ "), (Nav::Forward, " ▶ "), (Nav::Up, " ▲ ")];
    const WIDTH: u16 = 3;
}

enum Action {
    Continue,
    Accept,
    Cancel,
    Execute(Box<(crate::actions::Action, PathBuf)>),
}

pub fn run(args: PickArgs, root: PathBuf, config: Config) -> Result<Option<PathBuf>, Error> {
    let (pinned, notice) = match crate::favorites::Index::load() {
        Ok(index) => (index, None),
        Err(error) => (
            crate::favorites::Index::default(),
            Some(format!("Cannot load favorite markers: {error}")),
        ),
    };
    let mut picker = Picker {
        sources: std::array::from_fn(|_| None),
        browser: (args.mode == Mode::Browse).then(|| Browser::new(root.clone())),
        mode: args.mode,
        query: String::new(),
        root,
        config,
        highlighter: Matcher::new(MatchConfig::DEFAULT.match_paths()),
        preview: None,
        frame_count: 0,
        notice,
        pinned,
        menu: None,
        mouse_rows: (Rect::default(), 0, 0),
        mouse_header: Rect::default(),
        mouse_nav: Vec::new(),
        mouse_modes_x: 0,
        mouse_paths: Vec::new(),
        last_click: None,
    };
    picker.set_query(&args.query);

    if args.select_1 && args.mode != Mode::Browse {
        let source = picker.source();
        source.finish_scan();
        if let Some(error) = &source.scan_error {
            return Err(error.clone().into());
        }
        while source.matcher.tick(TICK_MS).running {}
        if source.matcher.snapshot().matched_item_count() == 0 {
            return Ok(None);
        }
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
        self.sources = std::array::from_fn(|_| None);
        self.preview = None;
    }

    /// First entry into browse uses the search selection; subsequent mode
    /// switches restore the existing browser, including its filter and selection.
    fn switch_mode(&mut self, step: isize) {
        let leaving = self.mode;
        let idx = mode_index(leaving) as isize + step;
        let len = MODE_ORDER.len() as isize;
        let entering = MODE_ORDER[idx.rem_euclid(len) as usize];
        if entering == leaving {
            return;
        }

        if leaving == Mode::Browse {
            let cwd = self.browser().cwd.clone();
            self.set_root(cwd);
        }
        self.mode = entering;
        if entering == Mode::Favorites {
            self.reload_pinned();
            self.sources[mode_index(Mode::Favorites)] = None;
        }
        if entering == Mode::Browse && self.browser.is_none() {
            let start = self.sources[mode_index(leaving)]
                .as_ref()
                .and_then(Source::selected_entry)
                .map(|e| output_path(&e, leaving))
                .filter(|p| p.is_dir())
                .unwrap_or_else(|| self.root.clone());
            self.browser = Some(Browser::new(start));
        } else if entering != Mode::Browse {
            self.source();
        }
    }

    fn run_tui(&mut self) -> Result<Option<Entry>, Error> {
        let mut terminal = TerminalGuard::enter(self.config.mouse)?;
        loop {
            if self.mode != Mode::Browse {
                let source = self.source();
                source.collect_scan_result();
                source.matcher.tick(TICK_MS);
                source.refresh_browse_order();
                source.clamp_selection();
            }
            // Draw only once the input queue is empty. A burst of scroll
            // events then costs one redraw at the end instead of one per
            // notch, which is what made the picker stop answering during a
            // fast scroll through a large directory.
            if !event::poll(Duration::ZERO)? {
                self.refresh_preview();
                terminal.draw(|frame| self.render(frame.area(), frame))?;
            }

            if !event::poll(POLL)? {
                continue;
            }
            let key = match event::read()? {
                Event::Mouse(mouse) => {
                    if self.config.mouse
                        && let Some(menu) = &mut self.menu
                    {
                        if !menu.handle_mouse(mouse) {
                            continue;
                        }
                        KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)
                    } else {
                        self.handle_mouse(mouse);
                        continue;
                    }
                }
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
                Action::Execute(request) => {
                    let (action, target) = *request;
                    let name = action.name().to_string();
                    if action.run_mode() == crate::actions::RunMode::Terminal {
                        let resume_top = terminal.get_frame().area().y;
                        drop(terminal);
                        let action_screen = ActionScreen::enter()?;
                        eprintln!("\n{name}\nTarget: {}", target.display());
                        let result = action.execute_terminal(&target);
                        let message = match result {
                            Ok(status) => format!("{name}: {status}"),
                            Err(error) => format!("{name}: {error}"),
                        };
                        eprintln!("\n{message}\nPress Enter or Esc to return to thither.");
                        let pause = wait_for_return();
                        drop(action_screen);
                        terminal = TerminalGuard::enter_at(Some(resume_top), self.config.mouse)?;
                        self.notice = Some(message);
                        pause?;
                        self.preview = None;
                        if let Some(browser) = &mut self.browser {
                            browser.refresh();
                        }
                    } else {
                        self.notice = Some(match action.execute_detached(&target) {
                            Ok(Some(path)) => format!("Temporary copy: {}", path.display()),
                            Ok(None) => format!(
                                "{}: {}",
                                if matches!(action, crate::actions::Action::Copy) {
                                    "Copied"
                                } else {
                                    "Started"
                                },
                                name
                            ),
                            Err(error) => format!("{name}: {error}"),
                        });
                    }
                }
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

    /// The file or directory under the cursor, as is (files mode gives the
    /// file, not its parent). Browse mode falls back to the directory shown.
    fn selected_path(&mut self) -> Option<PathBuf> {
        if self.mode == Mode::Browse {
            let b = self.browser();
            return Some(b.selected_path().unwrap_or_else(|| b.cwd.clone()));
        }
        self.source().selected_entry().map(|e| e.path)
    }

    fn handle_mouse(&mut self, mouse: MouseEvent) {
        if !self.config.mouse || self.menu.is_some() {
            return;
        }
        let position = (mouse.column, mouse.row).into();
        if !matches!(
            mouse.kind,
            MouseEventKind::Down(MouseButton::Left) | MouseEventKind::Up(MouseButton::Left)
        ) {
            self.last_click = None;
        }
        if matches!(
            mouse.kind,
            MouseEventKind::Down(MouseButton::Left | MouseButton::Right)
        ) && let Some((_, path)) = self
            .mouse_paths
            .iter()
            .find(|(area, _)| area.contains(position))
        {
            self.last_click = None;
            let path = path.clone();
            if mouse.kind == MouseEventKind::Down(MouseButton::Right) {
                self.right_click(path, mouse.modifiers);
                return;
            }
            if self.mode != Mode::Browse {
                self.mode = Mode::Browse;
                self.browser().navigate_to(&path);
                self.preview = None;
                return;
            }
            if path == self.browser().cwd {
                self.browser().up();
            } else {
                self.browser().navigate_to(&path);
            }
            self.preview = None;
            return;
        }
        if mouse.kind == MouseEventKind::Down(MouseButton::Left)
            && self.mouse_header.contains(position)
        {
            if let Some((_, nav)) = self
                .mouse_nav
                .iter()
                .find(|(area, _)| area.contains(position))
            {
                self.navigate(*nav);
                return;
            }
            let mut x = self.mouse_modes_x;
            for (index, mode) in MODE_ORDER.iter().enumerate() {
                let end = x + mode.label().len() as u16;
                if mouse.column >= x && mouse.column < end {
                    self.switch_mode(index as isize - mode_index(self.mode) as isize);
                    return;
                }
                x = end + 1;
            }
        }
        let (area, first, count) = self.mouse_rows;
        if !area.contains(position) || count == 0 {
            return;
        }
        let selected = if self.mode == Mode::Browse {
            self.browser().selected
        } else {
            self.source().selected as usize
        };
        let next = match mouse.kind {
            MouseEventKind::Down(MouseButton::Left | MouseButton::Right) => {
                let row = first + (mouse.row - area.y) as usize;
                if row >= count {
                    return;
                }
                row
            }
            MouseEventKind::ScrollUp => selected.saturating_sub(3),
            MouseEventKind::ScrollDown => selected.saturating_add(3).min(count - 1),
            _ => return,
        };
        if self.mode == Mode::Browse {
            self.browser().selected = next;
            if mouse.kind == MouseEventKind::Down(MouseButton::Left)
                && let Some(path) = self.browser().selected_path()
            {
                let now = std::time::Instant::now();
                let double = self
                    .last_click
                    .take()
                    .is_some_and(|(previous, x, y, time)| {
                        previous == path
                            && x == mouse.column
                            && y == mouse.row
                            && now.duration_since(time) <= Duration::from_millis(500)
                    });
                if double {
                    self.browser().enter();
                    self.preview = None;
                } else {
                    self.last_click = Some((path, mouse.column, mouse.row, now));
                }
            }
        } else {
            self.source().selected = next as u32;
        }
        if mouse.kind == MouseEventKind::Down(MouseButton::Right)
            && let Some(target) = self.selected_path()
        {
            self.right_click(target, mouse.modifiers);
        }
    }

    fn right_click(&mut self, target: PathBuf, modifiers: KeyModifiers) {
        if modifiers.contains(KeyModifiers::CONTROL) {
            self.menu = Some(crate::action_menu::Menu::new(target));
            return;
        }
        let result = if target.is_dir() {
            open::reveal(&target)
        } else {
            open::launch(&target)
        };
        self.notice = Some(match result {
            Ok(()) => format!("Opened: {}", target.display()),
            Err(error) => format!("Cannot open {}: {error}", target.display()),
        });
    }

    /// Runs a navigation button. Kept beside the key handling it mirrors, so
    /// clicking and pressing the key cannot drift apart.
    fn navigate(&mut self, nav: Nav) {
        self.notice = None;
        match nav {
            Nav::Up => {
                self.browser().up();
                self.preview = None;
            }
            Nav::Back | Nav::Forward => {
                if self.browser().history(nav == Nav::Forward) {
                    self.preview = None;
                } else {
                    self.notice = Some("No available directory in history".into());
                }
            }
        }
    }

    fn handle_key(&mut self, key: KeyEvent) -> Action {
        self.last_click = None;
        if let Some(menu) = &mut self.menu {
            return match menu.handle(key) {
                crate::action_menu::Decision::Stay => Action::Continue,
                crate::action_menu::Decision::Close => {
                    self.menu = None;
                    Action::Continue
                }
                crate::action_menu::Decision::Run(action) => {
                    let target = menu.target.clone();
                    self.menu = None;
                    Action::Execute(Box::new((*action, target)))
                }
            };
        }
        self.notice = None;
        if self.mode == Mode::Browse
            && key.modifiers.contains(KeyModifiers::ALT)
            && matches!(key.code, KeyCode::Left | KeyCode::Right)
        {
            if self.browser().history(key.code == KeyCode::Right) {
                self.preview = None;
            } else {
                self.notice = Some("No available directory in history".into());
            }
            return Action::Continue;
        }
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        match (key.code, ctrl) {
            (KeyCode::Char('p'), true) => {
                let target = self.selected_path().unwrap_or_else(|| self.root.clone());
                self.menu = Some(crate::action_menu::Menu::new(target));
                return Action::Continue;
            }
            (KeyCode::Char('b'), true) => {
                let target = if self.mode == Mode::Browse {
                    Some(self.browser().target())
                } else {
                    let mode = self.mode;
                    self.source()
                        .selected_entry()
                        .map(|entry| output_path(&entry, mode))
                };
                self.notice = Some(match target {
                    None => "Nothing selected. Switch to browse to pin this folder.".into(),
                    Some(path) => match crate::favorites::path().and_then(|file| {
                        let added = crate::favorites::update(&file, &path, None)?;
                        crate::favorites::Index::read(&file).map(|index| (added, index))
                    }) {
                        Ok((added, index)) => {
                            self.pinned = index;
                            self.sources[mode_index(Mode::Favorites)] = None;
                            format!(
                                "{}: {}",
                                if added { "Pinned" } else { "Unpinned" },
                                path.display()
                            )
                        }
                        Err(error) => format!("Cannot update favorites: {error}"),
                    },
                });
                return Action::Continue;
            }
            (KeyCode::F(5), _) => {
                self.reload_pinned();
                self.preview = None;
                if self.mode == Mode::Browse {
                    self.browser().refresh();
                } else {
                    self.sources[mode_index(self.mode)] = None;
                    self.source();
                }
                return Action::Continue;
            }
            (KeyCode::Esc, _) => {
                let has_filter = if self.mode == Mode::Browse {
                    !self.browser().filter.is_empty()
                } else {
                    !self.query.is_empty()
                };
                if has_filter {
                    self.set_query("");
                    return Action::Continue;
                }
                return Action::Cancel;
            }
            (KeyCode::Char('c'), true) => return Action::Cancel,
            (KeyCode::Enter, _) => return Action::Accept,
            (KeyCode::Tab, _) => {
                self.switch_mode(1);
                return Action::Continue;
            }
            (KeyCode::BackTab, _) => {
                self.switch_mode(-1);
                return Action::Continue;
            }
            // Hand the selection to the desktop and stay open, as yazi does.
            (KeyCode::Char('o'), true) => {
                if let Some(path) = self.selected_path() {
                    let _ = open::reveal(&path);
                }
                return Action::Continue;
            }
            (KeyCode::Char('e'), true) => {
                if let Some(path) = self.selected_path() {
                    let _ = open::launch(&path);
                }
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

    fn reload_pinned(&mut self) {
        match crate::favorites::Index::load() {
            Ok(index) => self.pinned = index,
            Err(error) => self.notice = Some(format!("Cannot load favorite markers: {error}")),
        }
    }

    fn render(&mut self, area: Rect, frame: &mut ratatui::Frame) {
        self.mouse_rows = (Rect::default(), 0, 0);
        self.mouse_header = Rect::default();
        self.mouse_nav.clear();
        self.mouse_paths.clear();
        if let Some(menu) = &mut self.menu {
            menu.render(area, frame);
            return;
        }
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
            Mode::Recent => vec![Span::styled("zoxide", theme::HEADER)],
            Mode::Favorites => vec![Span::styled("pinned directories", theme::HEADER)],
            _ => location_spans(&self.root),
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
                    self.notice.clone().unwrap_or_else(|| {
                        " Tab: mode  ^P: actions  ^B: pin  F5: refresh  Enter: cd  Esc: clear/exit "
                            .into()
                    }),
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

        self.mouse_header = header_area;
        // The search modes have nothing to step back to, so the tabs start at
        // the first column and no navigation buttons are drawn.
        self.mouse_modes_x = header_area.x + 1;
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

        if let Some(error) = &source.scan_error {
            frame.render_widget(
                Paragraph::new(format!(
                    "Cannot load {}: {error}\nF5: retry  Tab: mode  Esc: cancel",
                    mode.label()
                ))
                .style(Style::default().fg(Color::Red))
                .wrap(Wrap { trim: false }),
                rows_area,
            );
            return;
        }
        if mode == Mode::Recent && !scanning && total == 0 {
            frame.render_widget(
                Paragraph::new("No history yet. F5: refresh  Tab: mode"),
                rows_area,
            );
        }
        if mode == Mode::Favorites && !scanning && total == 0 {
            frame.render_widget(
                Paragraph::new("No favorites yet. Ctrl-B: pin a folder in another mode. Tab: mode")
                    .wrap(Wrap { trim: false }),
                rows_area,
            );
        }

        let height = rows_area.height as u32;
        if height == 0 || count == 0 {
            return;
        }
        let selected = source.selected;
        let first = selected
            .saturating_sub(height - 1)
            .min(count.saturating_sub(height));
        let last = (first + height).min(count);
        self.mouse_rows = (rows_area, first as usize, count as usize);
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
                let marks = RowMarks {
                    icons: self.config.icons,
                    favorite: self.pinned.contains(&output_path(item.data, mode)),
                };
                let icon = marks.prefix(&item.data.display, mode != Mode::Files);
                if !icon.is_empty() {
                    line.spans.insert(1, icons::span(icon));
                }
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
                    self.notice.clone().unwrap_or_else(|| {
                        " Left: up  Right: enter  Alt-Left/Right: history  Tab: mode  ^P: actions  ^B: pin  Enter: cd "
                            .into()
                    }),
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
        let folder = browser
            .cwd
            .file_name()
            .unwrap_or(browser.cwd.as_os_str())
            .to_string_lossy();
        let prompt = Paragraph::new(Line::from(vec![
            Span::styled("> ", theme::PROMPT),
            Span::raw(filter.as_str()),
            Span::styled(format!("   [Filter: {folder}]"), theme::HEADER),
        ]));
        frame.render_widget(prompt, prompt_area);
        if prompt_area.width > 0 && prompt_area.height > 0 {
            frame.set_cursor_position((
                prompt_area.x
                    + (2 + filter.width()).min(prompt_area.width.saturating_sub(1) as usize) as u16,
                prompt_area.y,
            ));
        }

        let shown = browser.rows().len();
        let total = browser.items().len();
        let counts = format!("  {shown}/{total} ");
        let rule_width = (info_area.width as usize).saturating_sub(counts.chars().count());
        let info = Paragraph::new(Line::from(vec![
            Span::styled(counts, theme::INFO),
            Span::styled("─".repeat(rule_width), theme::BORDER),
        ]));
        frame.render_widget(info, info_area);
        let mut location = location_spans(&browser.cwd);
        if self.pinned.contains(&browser.cwd) {
            location.insert(0, icons::span("★ "));
        }
        // Navigation buttons first, so the same moves the keyboard offers are
        // reachable with the mouse; a direction with nowhere to go is dimmed
        // and left out of the clickable areas.
        let available = [
            browser.has_history(false),
            browser.has_history(true),
            browser.cwd.parent().is_some(),
        ];
        self.mouse_nav.clear();
        let mut header: Vec<Span> = Vec::new();
        for (index, ((nav, glyph), enabled)) in Nav::BUTTONS.iter().zip(available).enumerate() {
            let style = if enabled {
                theme::HEADER
            } else {
                theme::BORDER
            };
            header.push(Span::styled(*glyph, style));
            if enabled {
                let x = header_area.x + index as u16 * Nav::WIDTH;
                self.mouse_nav
                    .push((Rect::new(x, header_area.y, Nav::WIDTH, 1), *nav));
            }
        }
        header.push(Span::raw(" "));
        let prefix = Nav::WIDTH * Nav::BUTTONS.len() as u16 + 1;
        self.mouse_modes_x = header_area.x + prefix + 1;
        header.extend(header_line(Mode::Browse, location));
        self.mouse_header = header_area;
        frame.render_widget(Paragraph::new(Line::from(header)), header_area);

        // The middle column is the subject, so it gets the most room.
        let [parent_area, sep1, current_area, sep2, preview_area] = Layout::horizontal([
            Constraint::Percentage(20),
            Constraint::Length(1),
            Constraint::Percentage(45),
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
                    browse_row(
                        &item.label(),
                        item.is_dir,
                        current,
                        true,
                        &[],
                        parent_area.width as usize,
                        RowMarks {
                            icons: self.config.icons,
                            favorite: item.is_dir
                                && browser.cwd.parent().is_some_and(|parent| {
                                    self.pinned.contains(&parent.join(&item.name))
                                }),
                        },
                    )
                })
                .collect();
            if let Some(parent) = browser.cwd.parent() {
                for (row, item) in items.iter().skip(first).take(height).enumerate() {
                    self.mouse_paths.push((
                        Rect::new(
                            parent_area.x,
                            parent_area.y + row as u16,
                            parent_area.width,
                            1,
                        ),
                        parent.join(&item.name),
                    ));
                }
            }
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
                browse_row(
                    &item.label(),
                    item.is_dir,
                    i == selected,
                    false,
                    &row.hits,
                    current_area.width as usize,
                    RowMarks {
                        icons: self.config.icons,
                        favorite: item.is_dir
                            && self.pinned.contains(&browser.cwd.join(&item.name)),
                    },
                )
            })
            .collect();
        frame.render_widget(List::new(rows), current_area);
        self.mouse_rows = (current_area, first, shown);

        // Preview column: contents of the selected directory. The listing is
        // loaded in the update phase, so a burst of scroll events costs no
        // directory reads; until it arrives the column is simply blank.
        let dir = browser.selected_path().filter(|p| p.is_dir());
        if let Some(dir) = dir
            && let Some((_, names)) = self.preview.as_ref().filter(|(p, _)| p == &dir)
        {
            {
                for (row, name) in names.iter().take(preview_area.height as usize).enumerate() {
                    self.mouse_paths.push((
                        Rect::new(
                            preview_area.x,
                            preview_area.y + row as u16,
                            preview_area.width,
                            1,
                        ),
                        dir.join(name),
                    ));
                }
                let rows: Vec<ListItem> = names
                    .iter()
                    .take(preview_area.height as usize)
                    .map(|n| {
                        let is_dir = n.ends_with(std::path::MAIN_SEPARATOR);
                        browse_row(
                            n,
                            is_dir,
                            false,
                            true,
                            &[],
                            preview_area.width as usize,
                            RowMarks {
                                icons: self.config.icons,
                                favorite: is_dir && self.pinned.contains(&dir.join(n)),
                            },
                        )
                    })
                    .collect();
                frame.render_widget(List::new(rows), preview_area);
            }
        }
    }

    /// The directory whose contents the preview pane should show.
    fn preview_target(&mut self) -> Option<PathBuf> {
        if self.mode == Mode::Browse {
            return self.browser().selected_path().filter(|p| p.is_dir());
        }
        let mode = self.mode;
        self.source()
            .selected_entry()
            .map(|entry| output_path(&entry, mode))
            .filter(|path| path.is_dir())
    }

    /// Loads the listing behind the preview pane.
    ///
    /// Called from the update phase and never from the drawing code, because
    /// reading a directory is the only blocking call on that path: doing it per
    /// frame made a fast scroll queue one directory read per notch, and the
    /// picker stopped answering the keyboard until the queue drained.
    fn refresh_preview(&mut self) {
        match self.preview_target() {
            Some(dir) => {
                if !self.preview.as_ref().is_some_and(|(p, _)| p == &dir) {
                    self.preview = Some((dir.clone(), list_dir(&dir)));
                }
            }
            None => self.preview = None,
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
            return;
        };
        // Loaded in the update phase; blank for a frame while scrolling fast.
        let Some((_, names)) = self.preview.as_ref().filter(|(p, _)| p == dir) else {
            return;
        };
        for (row, name) in names.iter().take(inner.height as usize).enumerate() {
            self.mouse_paths.push((
                Rect::new(inner.x, inner.y + row as u16, inner.width, 1),
                dir.join(name),
            ));
        }
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
                let is_dir = n.ends_with(std::path::MAIN_SEPARATOR);
                let marks = RowMarks {
                    icons: self.config.icons,
                    favorite: is_dir && self.pinned.contains(&dir.join(n)),
                };
                let icon = marks.prefix(n, is_dir);
                ListItem::new(
                    Line::from(vec![icons::span(icon), Span::raw(n.as_str())]).style(style),
                )
            })
            .collect();
        frame.render_widget(List::new(items), inner);
    }
}

/// Trims `text` to `width` terminal columns, ending in an ellipsis when it does
/// not fit. Counts display width, so a Japanese name is not cut mid-cell.
fn fit(text: &str, width: usize) -> String {
    if width == 0 {
        return String::new();
    }
    if text.width() <= width {
        return text.to_string();
    }
    let mut out = String::new();
    let mut used = 0;
    for c in text.chars() {
        let w = c.width().unwrap_or(0);
        if used + w > width - 1 {
            break;
        }
        out.push(c);
        used += w;
    }
    out.push('…');
    out
}

/// A favorite marker takes the icon slot, including when Nerd Font icons are disabled.
#[derive(Default, Clone, Copy)]
struct RowMarks {
    icons: bool,
    favorite: bool,
}

impl RowMarks {
    fn prefix(self, name: &str, is_dir: bool) -> &'static str {
        if self.favorite {
            "★ "
        } else {
            icons::prefix(name, is_dir, self.icons)
        }
    }
}

/// One row of a browse column, trimmed to width. Side columns are drawn muted.
fn browse_row(
    label: &str,
    is_dir: bool,
    current: bool,
    side: bool,
    hits: &[u32],
    width: usize,
    marks: RowMarks,
) -> ListItem<'static> {
    let icon = marks.prefix(label, is_dir);
    let icon = if width >= 2 + icon.width() { icon } else { "" };
    let text = fit(label, width.saturating_sub(2 + icon.width()));
    let len = text.chars().count() as u32;
    let kept: Vec<u32> = hits.iter().copied().filter(|&i| i < len).collect();
    let style = match (current, is_dir, side) {
        (true, _, _) => theme::CURRENT,
        (_, true, true) => theme::SIDE_DIR,
        (_, true, false) => theme::DIR,
        (_, false, true) => theme::SIDE,
        (_, false, false) => Style::default(),
    };
    let mut line = highlight_line(current, &text, &kept);
    if !icon.is_empty() {
        line.spans.insert(1, icons::span(icon));
    }
    ListItem::new(line.style(style))
}

/// The path with its last segment in bold, so the folder in the middle column
/// can be found in the header at a glance.
fn location_spans(path: &Path) -> Vec<Span<'static>> {
    let full = path.display().to_string();
    match path.file_name().map(|n| n.to_string_lossy().into_owned()) {
        Some(name) if full.ends_with(&name) => {
            let head = full[..full.len() - name.len()].to_string();
            vec![
                Span::styled(head, theme::HEADER),
                Span::styled(name, theme::HEADER.add_modifier(Modifier::BOLD)),
            ]
        }
        _ => vec![Span::styled(full, theme::HEADER)],
    }
}

/// Mode tabs followed by the location, with the active mode underlined.
fn header_line(mode: Mode, location: Vec<Span<'static>>) -> Vec<Span<'static>> {
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
    header.extend(location);
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
    /// Directories in the column being worked in. Bright enough to read on a
    /// black background, which the terminal's own blue is not.
    pub const DIR: Style = Style::new().fg(Color::Indexed(75));
    /// The side columns are context, not the subject, so they are muted and
    /// the eye lands on the middle column.
    pub const SIDE: Style = Style::new().fg(Color::Indexed(244));
    pub const SIDE_DIR: Style = Style::new().fg(Color::Indexed(67));
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
/// Keep command output separate from the inline picker and shell history.
struct ActionScreen;

impl ActionScreen {
    fn enter() -> Result<Self, Error> {
        crossterm::execute!(io::stderr(), crossterm::terminal::EnterAlternateScreen)?;
        let screen = Self;
        crossterm::execute!(
            io::stderr(),
            crossterm::terminal::Clear(crossterm::terminal::ClearType::All),
            crossterm::cursor::MoveTo(0, 0),
            crossterm::cursor::Show
        )?;
        Ok(screen)
    }
}

impl Drop for ActionScreen {
    fn drop(&mut self) {
        let _ = crossterm::execute!(io::stderr(), crossterm::terminal::LeaveAlternateScreen);
    }
}

fn wait_for_return() -> Result<(), Error> {
    enable_raw_mode()?;
    let result = (|| -> Result<(), Error> {
        loop {
            if let Event::Key(key) = event::read()?
                && key.kind != KeyEventKind::Release
                && (matches!(key.code, KeyCode::Enter | KeyCode::Esc)
                    || (key.code == KeyCode::Char('c')
                        && key.modifiers.contains(KeyModifiers::CONTROL)))
            {
                return Ok(());
            }
        }
    })();
    let restored = disable_raw_mode();
    result?;
    restored?;
    Ok(())
}

/// Owns raw mode and the inline viewport, including error cleanup.
struct TerminalGuard {
    terminal: Terminal<CrosstermBackend<io::Stderr>>,
    mouse: bool,
}

impl TerminalGuard {
    /// Opens an inline viewport at the bottom of the screen, like fzf --height,
    /// so the command history above stays visible.
    fn enter(mouse: bool) -> Result<Self, Error> {
        Self::enter_at(None, mouse)
    }

    fn enter_at(top: Option<u16>, mouse: bool) -> Result<Self, Error> {
        let (_, rows) = crossterm::terminal::size()?;
        let cursor_row = match top {
            Some(top) => {
                let top = top.min(rows.saturating_sub(1));
                crossterm::execute!(io::stderr(), crossterm::cursor::MoveTo(0, top))?;
                top
            }
            None => crossterm::cursor::position().unwrap_or((0, rows)).1,
        };
        enable_raw_mode()?;
        match Self::open(rows, cursor_row) {
            Ok(terminal) => {
                let guard = Self { terminal, mouse };
                if mouse {
                    crossterm::execute!(io::stderr(), EnableMouseCapture)?;
                }
                Ok(guard)
            }
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
        if self.mouse {
            let _ = crossterm::execute!(io::stderr(), DisableMouseCapture);
        }
        // Wipe the viewport so the prompt comes back where the picker opened.
        let top = self.terminal.get_frame().area().y;
        let _ = self.terminal.clear();
        let _ = crossterm::execute!(io::stderr(), crossterm::cursor::MoveTo(0, top));
        let _ = disable_raw_mode();
        let _ = io::stderr().flush();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn favorite_marker_stays_yellow_and_preserves_matches_without_nerd_icons() {
        use ratatui::buffer::Buffer;
        use ratatui::widgets::Widget;
        for icons in [false, true] {
            for current in [false, true] {
                let area = Rect::new(0, 0, 24, 1);
                let mut buffer = Buffer::empty(area);
                let marks = RowMarks {
                    icons,
                    favorite: true,
                };
                let item = browse_row("日本語", true, current, false, &[1], 24, marks);
                List::new(vec![item]).render(area, &mut buffer);
                assert_eq!(buffer[(2, 0)].symbol(), "★");
                assert_eq!(buffer[(2, 0)].fg, Color::Indexed(220));
                let name_x = 2 + "★ ".width() as u16;
                assert_eq!(buffer[(name_x, 0)].symbol(), "日");
                assert_eq!(buffer[(name_x + 2, 0)].symbol(), "本");
                assert_eq!(buffer[(name_x + 2, 0)].fg, theme::MATCH.fg.unwrap());
                if current {
                    assert_eq!(buffer[(2, 0)].bg, theme::CURRENT.bg.unwrap());
                }
                for width in 2..12 {
                    let mut buffer = Buffer::empty(area);
                    List::new(vec![browse_row(
                        "日本語の名前",
                        true,
                        current,
                        false,
                        &[],
                        width,
                        marks,
                    )])
                    .render(area, &mut buffer);
                    for x in width as u16..area.width {
                        assert_eq!(buffer[(x, 0)].symbol(), " ");
                    }
                }
            }
        }
    }

    fn test_picker(root: PathBuf, mode: Mode) -> Picker {
        Picker {
            sources: std::array::from_fn(|_| None),
            browser: None,
            mode,
            query: String::new(),
            root,
            config: Config::default(),
            highlighter: Matcher::new(MatchConfig::DEFAULT.match_paths()),
            preview: None,
            frame_count: 0,
            notice: None,
            pinned: crate::favorites::Index::default(),
            menu: None,
            mouse_rows: (Rect::default(), 0, 0),
            mouse_header: Rect::default(),
            mouse_nav: Vec::new(),
            mouse_modes_x: 0,
            mouse_paths: Vec::new(),
            last_click: None,
        }
    }

    #[test]
    fn double_click_enters_directory_and_filters_its_children() {
        use ratatui::backend::TestBackend;
        let root =
            std::env::temp_dir().join(format!("thither-double-click-{}", std::process::id()));
        std::fs::create_dir_all(root.join("C/ssl")).unwrap();
        std::fs::create_dir_all(root.join("C/other")).unwrap();
        let mut picker = test_picker(root.clone(), Mode::Browse);
        let mut terminal = Terminal::new(TestBackend::new(100, 25)).unwrap();
        terminal
            .draw(|frame| picker.render(frame.area(), frame))
            .unwrap();
        let (area, _, _) = picker.mouse_rows;
        let click = MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: area.x,
            row: area.y,
            modifiers: KeyModifiers::NONE,
        };
        picker.handle_mouse(click);
        assert_eq!(picker.browser().cwd, root);
        picker.handle_mouse(MouseEvent {
            kind: MouseEventKind::Up(MouseButton::Left),
            ..click
        });
        terminal
            .draw(|frame| picker.render(frame.area(), frame))
            .unwrap();
        picker.handle_mouse(click);
        assert_eq!(picker.browser().cwd, root.join("C"));
        for ch in "ssl".chars() {
            picker.handle_key(KeyEvent::new(KeyCode::Char(ch), KeyModifiers::NONE));
        }
        assert_eq!(picker.browser().rows().len(), 1);
        assert_eq!(picker.browser().selected_path(), Some(root.join("C/ssl")));
        terminal
            .draw(|frame| picker.render(frame.area(), frame))
            .unwrap();
        let screen: String = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect();
        assert!(screen.contains("[Filter: C]"));
        drop(picker);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn switching_modes_restores_browse_location_selection_and_filter() {
        let root =
            std::env::temp_dir().join(format!("thither-mode-restore-{}", std::process::id()));
        std::fs::create_dir_all(root.join("ahktest")).unwrap();
        std::fs::create_dir_all(root.join("AWS")).unwrap();
        let mut picker = test_picker(root.clone(), Mode::Browse);
        picker.browser().set_filter("AWS");
        let selected = picker.browser().selected_path();
        picker.switch_mode(1);
        assert_eq!(picker.mode, Mode::Dirs);
        let source = picker.source();
        source.finish_scan();
        while source.matcher.tick(TICK_MS).running {}
        picker.switch_mode(-1);
        assert_eq!(picker.browser().cwd, root);
        assert_eq!(picker.browser().filter, "AWS");
        assert_eq!(picker.browser().selected_path(), selected);
        drop(picker);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn header_buttons_move_back_forward_and_up_and_dim_where_they_cannot() {
        use ratatui::backend::TestBackend;
        let root = crate::testing::temp_dir().join(format!("thither-nav-{}", std::process::id()));
        let child = root.join("child");
        std::fs::create_dir_all(child.join("grandchild")).unwrap();
        let mut picker = test_picker(child.clone(), Mode::Browse);
        let mut terminal = Terminal::new(TestBackend::new(90, 20)).unwrap();
        let draw = |picker: &mut Picker, terminal: &mut Terminal<TestBackend>| {
            terminal
                .draw(|frame| picker.render(Rect::new(0, 0, 90, 20), frame))
                .unwrap();
        };
        let button = |picker: &Picker, nav: Nav| {
            picker
                .mouse_nav
                .iter()
                .find(|(_, kind)| *kind == nav)
                .map(|(area, _)| *area)
        };
        let click = |picker: &mut Picker, area: Rect| {
            picker.handle_mouse(MouseEvent {
                kind: MouseEventKind::Down(MouseButton::Left),
                column: area.x,
                row: area.y,
                modifiers: KeyModifiers::NONE,
            });
        };

        // Nothing visited yet, so only the step to the parent is offered.
        draw(&mut picker, &mut terminal);
        assert!(button(&picker, Nav::Back).is_none());
        assert!(button(&picker, Nav::Forward).is_none());
        let up = button(&picker, Nav::Up).expect("up is available below the root");
        click(&mut picker, up);
        assert_eq!(picker.browser().cwd, root);

        // Having moved, back becomes available and returns to the child.
        draw(&mut picker, &mut terminal);
        let back = button(&picker, Nav::Back).expect("back after moving up");
        click(&mut picker, back);
        assert_eq!(picker.browser().cwd, child);

        // And forward retraces that step.
        draw(&mut picker, &mut terminal);
        let forward = button(&picker, Nav::Forward).expect("forward after going back");
        click(&mut picker, forward);
        assert_eq!(picker.browser().cwd, root);
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn drawing_never_reads_a_directory_so_a_scroll_burst_costs_nothing() {
        use ratatui::backend::TestBackend;
        let root =
            crate::testing::temp_dir().join(format!("thither-preview-idle-{}", std::process::id()));
        std::fs::create_dir_all(root.join("child")).unwrap();
        let mut picker = test_picker(root.clone(), Mode::Browse);
        let mut terminal = Terminal::new(TestBackend::new(90, 20)).unwrap();

        // Every frame drawn while input is still queued must leave the listing
        // alone, otherwise a fast scroll queues one directory read per notch.
        for _ in 0..5 {
            terminal
                .draw(|frame| picker.render(Rect::new(0, 0, 90, 20), frame))
                .unwrap();
            assert!(
                picker.preview.is_none(),
                "rendering loaded a directory listing"
            );
        }

        // Once the queue drains the update phase fills it in.
        picker.refresh_preview();
        let (path, names) = picker.preview.clone().expect("listing after refresh");
        assert_eq!(path, root.join("child"));
        assert!(names.is_empty(), "child is empty: {names:?}");
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn search_preview_click_targets_child_without_changing_search_for_menu() {
        use ratatui::backend::TestBackend;
        let root =
            std::env::temp_dir().join(format!("thither-preview-mouse-{}", std::process::id()));
        let child = root.join("child");
        std::fs::create_dir_all(&child).unwrap();
        let mut picker = test_picker(root.clone(), Mode::Dirs);
        let mut terminal = Terminal::new(TestBackend::new(80, 20)).unwrap();
        // The real loop loads the listing before drawing; rendering never does.
        picker.preview = Some((root.clone(), list_dir(&root)));
        terminal
            .draw(|frame| picker.render_preview(Rect::new(40, 3, 35, 12), frame, Some(&root)))
            .unwrap();
        let (area, target) = picker.mouse_paths.first().unwrap().clone();
        assert_eq!(target, child);
        let mut click = MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Right),
            column: area.x,
            row: area.y,
            modifiers: KeyModifiers::CONTROL,
        };
        picker.handle_mouse(click);
        assert_eq!(picker.menu.as_ref().unwrap().target, child);
        assert_eq!(picker.mode, Mode::Dirs);
        picker.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        click.kind = MouseEventKind::Down(MouseButton::Left);
        click.modifiers = KeyModifiers::NONE;
        picker.handle_mouse(click);
        assert_eq!(picker.mode, Mode::Browse);
        assert_eq!(picker.browser().cwd, child);
        drop(picker);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn mouse_uses_rendered_rows_and_ignores_blank_space_and_disabled_input() {
        use ratatui::backend::TestBackend;
        let root = std::env::temp_dir().join(format!("thither-mouse-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        for index in 0..30 {
            std::fs::create_dir_all(root.join(format!("item-{index:02}"))).unwrap();
        }
        let child = root.join("item-00/child");
        std::fs::create_dir_all(&child).unwrap();
        let mut picker = test_picker(root.clone(), Mode::Browse);
        picker.browser().selected = 20;
        let mut terminal = Terminal::new(TestBackend::new(100, 30)).unwrap();
        terminal
            .draw(|frame| picker.render(Rect::new(2, 5, 90, 15), frame))
            .unwrap();
        let (area, first, _) = picker.mouse_rows;
        assert!(first > 0);
        let mouse = |kind, column, row| MouseEvent {
            kind,
            column,
            row,
            modifiers: KeyModifiers::NONE,
        };
        picker.handle_mouse(mouse(
            MouseEventKind::Down(MouseButton::Left),
            area.x,
            area.y,
        ));
        assert_eq!(picker.browser().selected, first);
        picker.handle_mouse(mouse(MouseEventKind::ScrollDown, area.x, area.y));
        assert_eq!(picker.browser().selected, first + 3);
        picker.config.mouse = false;
        picker.handle_mouse(mouse(MouseEventKind::ScrollUp, area.x, area.y));
        assert_eq!(picker.browser().selected, first + 3);
        picker.config.mouse = true;
        picker.set_query("item-00");
        picker.refresh_preview();
        terminal
            .draw(|frame| picker.render(Rect::new(2, 5, 90, 15), frame))
            .unwrap();
        let (area, _, _) = picker.mouse_rows;
        picker.handle_mouse(mouse(
            MouseEventKind::Down(MouseButton::Left),
            area.x,
            area.y + 2,
        ));
        assert_eq!(picker.browser().selected, 0);
        let (hit, _) = picker
            .mouse_paths
            .iter()
            .find(|(_, path)| path == &child)
            .unwrap()
            .clone();
        picker.handle_mouse(MouseEvent {
            modifiers: KeyModifiers::CONTROL,
            ..mouse(MouseEventKind::Down(MouseButton::Right), hit.x, hit.y)
        });
        assert_eq!(picker.menu.as_ref().unwrap().target, child);
        assert_eq!(picker.browser().cwd, root);
        picker.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        assert!(picker.menu.is_none());
        picker.handle_mouse(MouseEvent {
            modifiers: KeyModifiers::CONTROL,
            ..mouse(MouseEventKind::Down(MouseButton::Right), area.x, area.y)
        });
        assert_eq!(picker.menu.as_ref().unwrap().target, root.join("item-00"));
        picker.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        picker.handle_mouse(mouse(MouseEventKind::Down(MouseButton::Left), hit.x, hit.y));
        assert_eq!(picker.browser().cwd, child);
        terminal
            .draw(|frame| picker.render(Rect::new(2, 5, 90, 15), frame))
            .unwrap();
        // Another sibling in the parent column navigates to it.
        let sibling = root.join("item-00/sibling");
        std::fs::create_dir_all(&sibling).unwrap();
        picker.browser().refresh();
        terminal
            .draw(|frame| picker.render(Rect::new(2, 5, 90, 15), frame))
            .unwrap();
        let (hit, _) = picker
            .mouse_paths
            .iter()
            .find(|(_, path)| path == &sibling)
            .unwrap()
            .clone();
        picker.handle_mouse(mouse(MouseEventKind::Down(MouseButton::Left), hit.x, hit.y));
        assert_eq!(picker.browser().cwd, sibling);
        // The tabs no longer start at the first column: the navigation
        // buttons sit in front of them in browse mode.
        let header = picker.mouse_header;
        picker.handle_mouse(mouse(
            MouseEventKind::Down(MouseButton::Left),
            picker.mouse_modes_x,
            header.y,
        ));
        assert_eq!(picker.mode, Mode::Dirs);
        drop(picker);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    #[cfg(windows)]
    fn network_scan_finishes_with_an_error_and_no_candidates() {
        for mode in [Mode::Dirs, Mode::Files] {
            let mut source = Source::start(
                mode,
                Path::new(r"\\unreachable.invalid\share"),
                &Config::default(),
            );
            source.finish_scan();
            assert!(source.scan_error.as_ref().unwrap().contains("blocked"));
            assert!(source.scan_done.load(Ordering::Acquire));
            assert_eq!(source.matcher.snapshot().item_count(), 0);
        }
    }

    #[test]
    fn escape_clears_active_filter_before_exit_and_ctrl_c_exits_immediately() {
        for mode in [Mode::Browse, Mode::Dirs] {
            let root = std::env::temp_dir().join("thither-escape-nonexistent-root");
            let mut picker = test_picker(root, mode);
            picker.set_query("ss");
            let escape = KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE);
            assert!(matches!(picker.handle_key(escape), Action::Continue));
            assert!(if mode == Mode::Browse {
                picker.browser().filter.is_empty()
            } else {
                picker.query.is_empty()
            });
            assert!(matches!(picker.handle_key(escape), Action::Cancel));
            picker.set_query("ss");
            assert!(matches!(
                picker.handle_key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL)),
                Action::Cancel
            ));
        }
    }

    #[test]
    fn history_errors_are_visible_on_entry_and_mode_switch_but_empty_history_is_distinct() {
        use ratatui::backend::TestBackend;
        for via_tab in [false, true] {
            for error in [
                Some("cannot run zoxide: missing"),
                Some("zoxide query failed: fixture error"),
                None,
            ] {
                let root = std::env::temp_dir().join("thither-nonexistent-history-test-root");
                let mut picker = test_picker(root.clone(), Mode::Recent);
                let mut source = Source::start(Mode::Dirs, &root, &Config::default());
                source.finish_scan();
                source.mode = Mode::Recent;
                source.scanner = Some(std::thread::spawn(move || {
                    error.map_or(Ok(()), |error| Err(error.to_string()))
                }));
                source.finish_scan();
                picker.sources[mode_index(Mode::Recent)] = Some(source);
                if via_tab {
                    picker.mode = Mode::Files;
                    picker.switch_mode(1);
                }
                let mut terminal = Terminal::new(TestBackend::new(100, 16)).unwrap();
                terminal
                    .draw(|frame| picker.render(frame.area(), frame))
                    .unwrap();
                let text: String = terminal
                    .backend()
                    .buffer()
                    .content
                    .iter()
                    .map(|cell| cell.symbol())
                    .collect();
                if let Some(error) = error {
                    assert!(text.contains(error), "{text}");
                    assert!(text.contains("F5: retry"));
                    assert!(!text.contains("No history yet"));
                } else {
                    assert!(text.contains("No history yet"));
                }
                picker.switch_mode(1);
                assert_eq!(picker.mode, Mode::Favorites);
            }
        }
    }

    #[test]
    #[ignore = "local performance measurement; set THITHER_BENCH_ROOT and run in release mode"]
    fn benchmark_local_tree() {
        use ratatui::backend::TestBackend;
        use std::time::Instant;
        let root = std::env::var_os("THITHER_BENCH_ROOT")
            .map(PathBuf::from)
            .unwrap_or_else(|| std::env::current_dir().unwrap());
        assert!(root.is_dir());
        eprintln!(
            "root={} OS={} arch={} profile={} excludes={:?}",
            root.display(),
            std::env::consts::OS,
            std::env::consts::ARCH,
            if cfg!(debug_assertions) {
                "debug"
            } else {
                "release"
            },
            Config::default().exclude
        );
        for mode in [Mode::Dirs, Mode::Files] {
            for trial in 1..=3 {
                let start = Instant::now();
                let mut source = Source::start(mode, &root, &Config::default());
                source.finish_scan();
                assert!(source.scan_error.is_none());
                let scan = start.elapsed();
                while source.matcher.tick(TICK_MS).running {}
                source.refresh_browse_order();
                let ready = start.elapsed();
                let count = source.matcher.snapshot().item_count();
                let filter_start = Instant::now();
                source.set_query("src", false);
                while source.matcher.tick(TICK_MS).running {}
                let filter = filter_start.elapsed();
                eprintln!(
                    "mode={} trial={trial} items={count} scan_ms={:.3} ready_ms={:.3} filter_ms={:.3}",
                    mode.label(),
                    scan.as_secs_f64() * 1000.0,
                    ready.as_secs_f64() * 1000.0,
                    filter.as_secs_f64() * 1000.0
                );
            }
        }
        let start = Instant::now();
        let mut picker = test_picker(root, Mode::Browse);
        picker.browser();
        let load = start.elapsed();
        let mut terminal = Terminal::new(TestBackend::new(120, 30)).unwrap();
        terminal
            .draw(|frame| picker.render(frame.area(), frame))
            .unwrap();
        let mut samples = Vec::new();
        for _ in 0..100 {
            let start = Instant::now();
            terminal
                .draw(|frame| picker.render(frame.area(), frame))
                .unwrap();
            samples.push(start.elapsed());
        }
        samples.sort();
        eprintln!(
            "browse_load_ms={:.3} cached_render_p50_ms={:.3} cached_render_p95_ms={:.3} (TestBackend, excludes terminal I/O and process startup)",
            load.as_secs_f64() * 1000.0,
            samples[49].as_secs_f64() * 1000.0,
            samples[94].as_secs_f64() * 1000.0
        );
    }

    #[test]
    fn icon_color_survives_selection_without_changing_filename_or_background() {
        use ratatui::buffer::Buffer;
        use ratatui::widgets::Widget;

        for current in [false, true] {
            let area = Rect::new(0, 0, 24, 1);
            let mut buffer = Buffer::empty(area);
            let item = browse_row(
                "表.xlsx",
                false,
                current,
                false,
                &[0],
                24,
                RowMarks {
                    icons: true,
                    favorite: false,
                },
            );
            List::new(vec![item]).render(area, &mut buffer);
            let icon = &buffer[(2, 0)];
            assert_eq!(icon.fg, Color::Indexed(71));
            let filename_x = 2 + icons::prefix("表.xlsx", false, true).width() as u16;
            assert_eq!(buffer[(filename_x, 0)].fg, theme::MATCH.fg.unwrap());
            assert_eq!(
                buffer[(filename_x + 2, 0)].fg,
                if current {
                    theme::CURRENT.fg.unwrap()
                } else {
                    Color::Reset
                }
            );
            if current {
                assert_eq!(icon.bg, theme::CURRENT.bg.unwrap());
            }
        }
    }

    #[test]
    fn browse_icons_preserve_highlights_and_fit_narrow_columns() {
        use ratatui::buffer::Buffer;
        use ratatui::widgets::Widget;

        for enabled in [false, true] {
            let area = Rect::new(0, 0, 24, 1);
            let mut buffer = Buffer::empty(area);
            let item = browse_row(
                "日本.rs",
                false,
                false,
                false,
                &[1],
                24,
                RowMarks {
                    icons: enabled,
                    favorite: false,
                },
            );
            List::new(vec![item]).render(area, &mut buffer);
            let offset = if enabled {
                icons::prefix("日本.rs", false, true).width()
            } else {
                0
            };
            assert_eq!(buffer[(2 + offset as u16, 0)].symbol(), "日");
            let matched = &buffer[(4 + offset as u16, 0)];
            assert_eq!(matched.symbol(), "本");
            assert_eq!(matched.fg, theme::MATCH.fg.unwrap());
        }

        for width in 2..12 {
            let area = Rect::new(0, 0, 20, 1);
            let mut buffer = Buffer::empty(area);
            let item = browse_row(
                "日本語の名前.rs",
                false,
                false,
                false,
                &[],
                width,
                RowMarks {
                    icons: true,
                    favorite: false,
                },
            );
            List::new(vec![item]).render(area, &mut buffer);
            for x in width as u16..area.width {
                assert_eq!(buffer[(x, 0)].symbol(), " ", "overflow at width {width}");
            }
        }
    }

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
    fn fit_trims_by_display_width() {
        assert_eq!(fit("docs", 10), "docs");
        assert_eq!(fit("devsense.composer-php", 10), "devsense.…");
        // Japanese names take two columns per character.
        assert_eq!(fit("画面録画", 8), "画面録画");
        assert_eq!(fit("画面録画", 7), "画面録…");
        assert_eq!(fit("anything", 0), "");
    }

    #[test]
    fn location_spans_bold_the_last_segment() {
        #[cfg(windows)]
        let (path, parent) = (r"C:\Users\example\thither", r"C:\Users\example\");
        #[cfg(not(windows))]
        let (path, parent) = ("/home/example/thither", "/home/example/");
        let spans = location_spans(Path::new(path));
        let parts: Vec<(String, bool)> = spans
            .iter()
            .map(|s| {
                (
                    s.content.to_string(),
                    s.style.add_modifier.contains(Modifier::BOLD),
                )
            })
            .collect();
        assert_eq!(
            parts,
            vec![(parent.to_string(), false), ("thither".to_string(), true)]
        );
    }

    #[test]
    fn mode_order_wraps_both_ways() {
        assert_eq!(mode_index(Mode::Dirs), 0);
        assert_eq!(mode_index(Mode::Browse), 4);
        let next = |m: Mode, step: isize| {
            let i = mode_index(m) as isize + step;
            MODE_ORDER[i.rem_euclid(MODE_ORDER.len() as isize) as usize]
        };
        assert_eq!(next(Mode::Recent, 1), Mode::Favorites);
        assert_eq!(next(Mode::Favorites, 1), Mode::Browse);
        assert_eq!(next(Mode::Browse, 1), Mode::Dirs);
        assert_eq!(next(Mode::Dirs, -1), Mode::Browse);
    }

    #[test]
    fn list_dir_puts_directories_first() {
        let tmp = std::env::temp_dir().join(format!("thither-preview-{}", std::process::id()));
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
