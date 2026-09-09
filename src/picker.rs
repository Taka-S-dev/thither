use std::io::{self, Write};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use nucleo::pattern::{CaseMatching, Normalization};
use nucleo::{Config as MatchConfig, Matcher, Nucleo};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, Paragraph};

use crate::config::Config;
use crate::scan::{self, Entry};
use crate::{Mode, PickArgs};

type Error = Box<dyn std::error::Error>;

/// Nucleo tick budget per frame. Keeps redraws under 16 ms while a scan is running.
const TICK_MS: u64 = 10;
const POLL: Duration = Duration::from_millis(16);

struct Picker {
    matcher: Nucleo<Entry>,
    /// Recomputes match positions for the visible rows only.
    highlighter: Matcher,
    query: String,
    selected: u32,
    mode: Mode,
    root: PathBuf,
    scan_done: Arc<AtomicBool>,
}

enum Action {
    Continue,
    Accept,
    Cancel,
}

pub fn run(args: PickArgs, root: PathBuf, config: Config) -> Result<Option<PathBuf>, Error> {
    let matcher = Nucleo::new(MatchConfig::DEFAULT.match_paths(), Arc::new(|| {}), None, 1);
    let scan_done = Arc::new(AtomicBool::new(false));
    let mut scanner = Some(scan::spawn(
        root.clone(),
        args.mode,
        &config.exclude,
        matcher.injector(),
        scan_done.clone(),
    ));
    let mut picker = Picker {
        matcher,
        highlighter: Matcher::new(MatchConfig::DEFAULT.match_paths()),
        query: String::new(),
        selected: 0,
        mode: args.mode,
        root,
        scan_done,
    };
    picker.set_query(&args.query);

    if args.select_1 {
        if let Some(handle) = scanner.take() {
            handle.join().map_err(|_| "scan thread panicked")?;
        }
        while picker.matcher.tick(TICK_MS).running {}
        let snapshot = picker.matcher.snapshot();
        if snapshot.matched_item_count() == 1 {
            let item = snapshot.get_matched_item(0).expect("one match");
            return Ok(Some(output_path(item.data, args.mode)));
        }
    }

    let result = picker.run_tui();
    drop(picker);
    if let Some(handle) = scanner {
        let _ = handle.join();
    }
    result.map(|entry| entry.map(|e| output_path(&e, args.mode)))
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

impl Picker {
    fn set_query(&mut self, query: &str) {
        let append = query.starts_with(&self.query);
        self.query = query.to_string();
        self.matcher.pattern.reparse(
            0,
            &self.query,
            CaseMatching::Ignore,
            Normalization::Smart,
            append,
        );
        self.selected = 0;
    }

    fn run_tui(&mut self) -> Result<Option<Entry>, Error> {
        let mut terminal = TerminalGuard::enter()?;
        loop {
            self.matcher.tick(TICK_MS);
            self.clamp_selection();
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
                    let snapshot = self.matcher.snapshot();
                    let Some(item) = snapshot.get_matched_item(self.selected) else {
                        continue;
                    };
                    let path = item.data.path.clone();
                    let display = item.data.display.clone();
                    return Ok(Some(Entry { path, display }));
                }
            }
        }
    }

    fn handle_key(&mut self, key: KeyEvent) -> Action {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        match (key.code, ctrl) {
            (KeyCode::Esc, _) | (KeyCode::Char('c'), true) => Action::Cancel,
            (KeyCode::Enter, _) => Action::Accept,
            (KeyCode::Up, _) | (KeyCode::Char('k'), true) => {
                self.selected = self.selected.saturating_sub(1);
                Action::Continue
            }
            (KeyCode::Down, _) | (KeyCode::Char('j'), true) => {
                self.selected = self.selected.saturating_add(1);
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

    fn clamp_selection(&mut self) {
        let count = self.matcher.snapshot().matched_item_count();
        if count == 0 {
            self.selected = 0;
        } else if self.selected >= count {
            self.selected = count - 1;
        }
    }

    fn render(&mut self, area: Rect, frame: &mut ratatui::Frame) {
        let snapshot = self.matcher.snapshot();
        let count = snapshot.matched_item_count();
        let total = snapshot.item_count();
        let scanning = if self.scan_done.load(Ordering::Acquire) {
            ""
        } else {
            " scanning"
        };

        let block = Block::default()
            .borders(Borders::ALL)
            .title(format!(" [{}] {} ", self.mode.label(), self.root.display()))
            .title_top(Line::from(format!(" {count}/{total}{scanning} ")).right_aligned())
            .title_bottom(" Enter: cd  Esc: cancel ");
        let inner = block.inner(area);
        frame.render_widget(block, area);

        let [prompt_area, list_area] =
            Layout::vertical([Constraint::Length(1), Constraint::Min(0)]).areas(inner);

        let prompt = Paragraph::new(Line::from(vec![
            Span::styled("> ", Style::default().add_modifier(Modifier::BOLD)),
            Span::raw(&self.query),
        ]));
        frame.render_widget(prompt, prompt_area);
        frame.set_cursor_position((
            prompt_area.x + 2 + self.query.chars().count() as u16,
            prompt_area.y,
        ));

        let height = list_area.height as u32;
        if height == 0 || count == 0 {
            return;
        }
        let first = self
            .selected
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
                let marker = if idx == self.selected { "> " } else { "  " };
                let mut line = highlight_line(marker, &item.data.display, &indices);
                if idx == self.selected {
                    line = line.style(Style::default().add_modifier(Modifier::REVERSED));
                }
                ListItem::new(line)
            })
            .collect();
        frame.render_widget(List::new(items), list_area);
    }
}

/// Builds one row, colouring the characters at `indices` (sorted, char positions).
fn highlight_line<'a>(marker: &'a str, text: &'a str, indices: &[u32]) -> Line<'a> {
    let matched = Style::default()
        .fg(Color::Cyan)
        .add_modifier(Modifier::BOLD);
    let mut spans = vec![Span::raw(marker)];
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
    fn enter() -> Result<Self, Error> {
        enable_raw_mode()?;
        let mut stderr = io::stderr();
        if let Err(err) = crossterm::execute!(stderr, EnterAlternateScreen) {
            let _ = disable_raw_mode();
            return Err(err.into());
        }
        let terminal = Terminal::new(CrosstermBackend::new(stderr))?;
        Ok(Self { terminal })
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
        let _ = disable_raw_mode();
        let _ = crossterm::execute!(io::stderr(), LeaveAlternateScreen);
        let _ = io::stderr().flush();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pieces(line: &Line) -> Vec<(String, bool)> {
        line.spans
            .iter()
            .map(|s| (s.content.to_string(), s.style.fg == Some(Color::Cyan)))
            .collect()
    }

    #[test]
    fn highlights_runs_of_matched_chars() {
        let line = highlight_line("> ", "openssl", &[0, 1, 2]);
        assert_eq!(
            pieces(&line),
            vec![
                ("> ".into(), false),
                ("ope".into(), true),
                ("nssl".into(), false)
            ]
        );
    }

    #[test]
    fn highlights_scattered_and_multibyte_chars() {
        let line = highlight_line("  ", r"ドキュメント\src", &[1, 7]);
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
        let line = highlight_line("  ", "docs", &[]);
        assert_eq!(
            pieces(&line),
            vec![("  ".into(), false), ("docs".into(), false)]
        );
    }
}
