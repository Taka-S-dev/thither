use std::path::PathBuf;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use nucleo::pattern::{CaseMatching, Normalization, Pattern};
use nucleo::{Config, Matcher, Utf32Str};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, List, ListItem, Paragraph, Wrap};
use unicode_width::UnicodeWidthStr;

use crate::actions::{self, Action, RunMode};

pub enum Decision {
    Stay,
    Close,
    Run(Box<Action>),
}

pub struct Menu {
    pub target: PathBuf,
    items: Vec<Action>,
    query: String,
    visible: Vec<usize>,
    selected: usize,
    error: Option<String>,
    mouse_rows: Rect,
    mouse_first: usize,
}

impl Menu {
    pub fn new(target: PathBuf) -> Self {
        let (items, error) = actions::load(&target);
        let visible = (0..items.len()).collect();
        Self {
            target,
            items,
            query: String::new(),
            visible,
            selected: 0,
            error,
            mouse_rows: Rect::default(),
            mouse_first: 0,
        }
    }

    /// Which action a letter runs. A setting that claims a letter takes it from
    /// the built-in action that had it, so one key never runs two things.
    fn owner(&self, ch: char) -> Option<usize> {
        let ch = ch.to_ascii_lowercase();
        self.items
            .iter()
            .rposition(|action| action.key() == Some(ch))
    }

    /// The letter to print beside a row, which is only the row that wins it.
    fn shown_key(&self, index: usize) -> Option<char> {
        let ch = self.items[index].key()?;
        (self.owner(ch) == Some(index)).then_some(ch)
    }

    pub fn handle(&mut self, key: KeyEvent) -> Decision {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        // Alt runs an action wherever it sits, including one the filter hides,
        // so the key stays the same whether or not anything has been typed.
        if key.modifiers.contains(KeyModifiers::ALT) && !ctrl {
            if let KeyCode::Char(ch) = key.code
                && let Some(index) = self.owner(ch)
            {
                return Decision::Run(Box::new(self.items[index].clone()));
            }
            return Decision::Stay;
        }
        match (key.code, ctrl) {
            (KeyCode::Esc, _) | (KeyCode::Char('c' | 'p'), true) => return Decision::Close,
            (KeyCode::Enter, _) => {
                if let Some(&index) = self.visible.get(self.selected) {
                    return Decision::Run(Box::new(self.items[index].clone()));
                }
            }
            (KeyCode::Up, _) | (KeyCode::Char('k'), true) => {
                self.selected = self.selected.saturating_sub(1)
            }
            (KeyCode::Down, _) | (KeyCode::Char('j'), true) => {
                self.selected = (self.selected + 1).min(self.visible.len().saturating_sub(1))
            }
            (KeyCode::Backspace, _) => {
                self.query.pop();
                self.filter();
            }
            (KeyCode::Char('u'), true) => {
                self.query.clear();
                self.filter();
            }
            (KeyCode::Char(ch), false) if crate::keys::is_typed_text(&key) => {
                self.query.push(ch);
                self.filter();
            }
            _ => {}
        }
        Decision::Stay
    }

    fn filter(&mut self) {
        self.selected = 0;
        let pattern = Pattern::parse(&self.query, CaseMatching::Ignore, Normalization::Smart);
        let mut matcher = Matcher::new(Config::DEFAULT);
        let mut buffer = Vec::new();
        let mut matches: Vec<(u32, usize)> = self
            .items
            .iter()
            .enumerate()
            .filter_map(|(index, action)| {
                pattern
                    .score(Utf32Str::new(action.name(), &mut buffer), &mut matcher)
                    .map(|score| (score, index))
            })
            .collect();
        matches.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
        self.visible = matches.into_iter().map(|(_, index)| index).collect();
    }

    /// Select a displayed action; true requests execution through the Enter path.
    pub fn handle_mouse(&mut self, mouse: MouseEvent) -> bool {
        if !self.mouse_rows.contains((mouse.column, mouse.row).into()) {
            return false;
        }
        match mouse.kind {
            MouseEventKind::Down(MouseButton::Left) => {
                let row = self.mouse_first + (mouse.row - self.mouse_rows.y) as usize;
                if row < self.visible.len() {
                    self.selected = row;
                    return true;
                }
            }
            MouseEventKind::ScrollUp => self.selected = self.selected.saturating_sub(3),
            MouseEventKind::ScrollDown => {
                self.selected = self
                    .selected
                    .saturating_add(3)
                    .min(self.visible.len().saturating_sub(1))
            }
            _ => {}
        }
        false
    }

    pub fn render(&mut self, area: Rect, frame: &mut ratatui::Frame) {
        self.mouse_rows = Rect::default();
        let block = Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .title(" Actions ")
            .title_bottom(" Click / Enter: run  Alt+key: run directly  Esc / Ctrl-P: back ");
        let inner = block.inner(area);
        frame.render_widget(block, area);
        let [target, prompt, warning, rows] = Layout::vertical([
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(if self.error.is_some() { 3 } else { 0 }),
            Constraint::Min(0),
        ])
        .areas(inner);
        frame.render_widget(
            Paragraph::new(self.target.display().to_string())
                .style(Style::default().fg(Color::Cyan)),
            target,
        );
        frame.render_widget(Paragraph::new(format!("> {}", self.query)), prompt);
        if prompt.width > 0 && prompt.height > 0 {
            frame.set_cursor_position((
                prompt.x + (2 + self.query.width() as u16).min(prompt.width - 1),
                prompt.y,
            ));
        }
        if let Some(error) = &self.error {
            frame.render_widget(
                Paragraph::new(error.as_str())
                    .style(Style::default().fg(Color::Red))
                    .wrap(Wrap { trim: false }),
                warning,
            );
        }
        if self.visible.is_empty() {
            frame.render_widget(Paragraph::new("No matching actions"), rows);
            return;
        }
        let first = self
            .selected
            .saturating_sub((rows.height as usize).saturating_sub(1));
        self.mouse_rows = rows;
        self.mouse_first = first;
        let shown: Vec<(usize, usize)> = self
            .visible
            .iter()
            .enumerate()
            .skip(first)
            .take(rows.height as usize)
            .map(|(row, &index)| (row, index))
            .collect();
        // The keys go in a column of their own on the left, so they can be read
        // down the list. Nothing is indented when no visible action has one.
        let keyed = shown
            .iter()
            .any(|&(_, index)| self.shown_key(index).is_some());
        let items: Vec<ListItem> = shown
            .iter()
            .map(|&(row, index)| {
                let action = &self.items[index];
                let suffix = if action.run_mode() == RunMode::Terminal {
                    "  [terminal]"
                } else {
                    ""
                };
                let key = match (keyed, self.shown_key(index)) {
                    (true, Some(ch)) => format!("alt+{ch}  "),
                    (true, None) => " ".repeat(7),
                    (false, _) => String::new(),
                };
                let line = Line::from(vec![
                    Span::raw(if row == self.selected { "▌ " } else { "  " }),
                    Span::styled(key, Style::default().fg(Color::DarkGray)),
                    Span::raw(action.name().to_string()),
                    Span::styled(suffix, Style::default().fg(Color::DarkGray)),
                ]);
                let style = if row == self.selected {
                    Style::default()
                        .fg(Color::White)
                        .bg(Color::Indexed(236))
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default()
                };
                ListItem::new(line).style(style)
            })
            .collect();
        frame.render_widget(List::new(items), rows);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::{Terminal, backend::TestBackend};

    fn menu_of(items: Vec<Action>) -> Menu {
        let visible = (0..items.len()).collect();
        Menu {
            target: PathBuf::from("selected file.txt"),
            items,
            query: String::new(),
            visible,
            selected: 0,
            error: None,
            mouse_rows: Rect::default(),
            mouse_first: 0,
        }
    }

    #[test]
    fn an_alt_key_runs_its_action_even_while_the_filter_hides_it() {
        let mut menu = menu_of(vec![Action::Reveal, Action::Copy]);
        for ch in "reveal".chars() {
            menu.handle(KeyEvent::new(KeyCode::Char(ch), KeyModifiers::NONE));
        }
        assert!(
            !menu.visible.contains(&1),
            "copy path should be filtered out"
        );
        assert!(matches!(
            menu.handle(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::ALT)),
            Decision::Run(action) if matches!(*action, Action::Copy)
        ));
        // A letter nothing claims does nothing, rather than closing or typing.
        assert!(matches!(
            menu.handle(KeyEvent::new(KeyCode::Char('z'), KeyModifiers::ALT)),
            Decision::Stay
        ));
        assert_eq!(menu.query, "reveal");
    }

    #[test]
    fn a_configured_key_takes_the_letter_from_the_built_in_action() {
        let custom = Action::Custom {
            definition: crate::actions::test_definition("Compare", Some("alt+c")),
            config_dir: PathBuf::from("config"),
        };
        let menu = menu_of(vec![Action::Reveal, Action::Copy, custom]);
        assert_eq!(menu.owner('c'), Some(2));
        assert_eq!(menu.shown_key(2), Some('c'));
        // The built-in keeps its letter in the enum but must not advertise one
        // it no longer runs.
        assert_eq!(menu.shown_key(1), None);
        assert_eq!(menu.shown_key(0), Some('f'));

        let mut menu = menu;
        let mut terminal = Terminal::new(TestBackend::new(44, 8)).unwrap();
        terminal
            .draw(|frame| menu.render(frame.area(), frame))
            .unwrap();
        let buffer = terminal.backend().buffer();
        // Columns 0 and 43 are the border.
        let row = |y: u16| -> String {
            (1..43)
                .map(|x| buffer[(x, y)].symbol())
                .collect::<String>()
                .trim_end()
                .to_string()
        };
        let rows: Vec<String> = (3..6).map(row).collect();
        // The names line up because the row without a key is padded to match.
        assert_eq!(rows[0], "▌ alt+f  Open in file manager");
        assert_eq!(rows[1], "         Copy path");
        assert_eq!(rows[2], "  alt+c  Compare");
    }

    #[test]
    fn alt_and_ctrl_chars_stay_out_of_the_filter() {
        let mut menu = Menu {
            target: PathBuf::from("selected file.txt"),
            items: vec![Action::Reveal, Action::Copy],
            query: String::new(),
            visible: vec![0, 1],
            selected: 0,
            error: None,
            mouse_rows: Rect::default(),
            mouse_first: 0,
        };
        menu.handle(KeyEvent::new(KeyCode::Char('d'), KeyModifiers::ALT));
        menu.handle(KeyEvent::new(KeyCode::Char('d'), KeyModifiers::CONTROL));
        assert!(menu.query.is_empty());
        assert_eq!(menu.visible, [0, 1]);
        menu.handle(KeyEvent::new(KeyCode::Char('d'), KeyModifiers::NONE));
        assert_eq!(menu.query, "d");
    }

    #[test]
    fn filtering_keeps_target_fixed_and_escape_only_closes_the_menu() {
        let target = PathBuf::from("selected file.txt");
        let mut menu = Menu {
            target: target.clone(),
            items: vec![Action::Reveal, Action::Copy],
            query: String::new(),
            visible: vec![0, 1],
            selected: 0,
            error: None,
            mouse_rows: Rect::default(),
            mouse_first: 0,
        };
        for ch in "copy".chars() {
            menu.handle(KeyEvent::new(KeyCode::Char(ch), KeyModifiers::NONE));
        }
        assert_eq!(menu.visible, [1]);
        assert_eq!(menu.target, target);
        let mut terminal = Terminal::new(TestBackend::new(60, 20)).unwrap();
        terminal
            .draw(|frame| menu.render(Rect::new(3, 4, 50, 12), frame))
            .unwrap();
        let click = |row| MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: menu.mouse_rows.x,
            row,
            modifiers: KeyModifiers::NONE,
        };
        let valid = click(menu.mouse_rows.y);
        let blank = click(menu.mouse_rows.y + 2);
        let header = click(menu.mouse_rows.y - 1);
        assert!(!menu.handle_mouse(blank));
        assert!(!menu.handle_mouse(header));
        assert!(menu.handle_mouse(valid));
        assert!(
            matches!(menu.handle(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)), Decision::Run(action) if matches!(*action, Action::Copy))
        );
        menu.handle(KeyEvent::new(KeyCode::Char('x'), KeyModifiers::NONE));
        assert!(menu.visible.is_empty());
        assert!(matches!(
            menu.handle(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
            Decision::Stay
        ));
        for (width, height) in [(2, 2), (40, 12), (100, 24)] {
            let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
            terminal
                .draw(|frame| menu.render(frame.area(), frame))
                .unwrap();
        }
        assert!(matches!(
            menu.handle(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)),
            Decision::Close
        ));
    }
}
