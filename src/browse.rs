//! Browse mode: walk the tree one level at a time, yazi style, with the
//! parent on the left, the current directory in the middle and the selected
//! entry's contents on the right. Typing filters the current level only.

use std::path::{Path, PathBuf};

use nucleo::pattern::{CaseMatching, Normalization, Pattern};
use nucleo::{Config, Matcher, Utf32Str};

/// Upper bound on entries read per directory. Keeps a pathological folder from
/// freezing the picker; nobody scrolls past this many rows anyway.
pub const DIR_LIMIT: usize = 20_000;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Item {
    pub name: String,
    pub is_dir: bool,
}

impl Item {
    /// Name as shown: directories carry a trailing separator.
    pub fn label(&self) -> String {
        if self.is_dir {
            format!("{}{}", self.name, std::path::MAIN_SEPARATOR)
        } else {
            self.name.clone()
        }
    }
}

/// Directory contents, directories first, each group sorted case-insensitively.
pub fn read_dir(dir: &Path) -> Vec<Item> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut items: Vec<Item> = entries
        .flatten()
        .take(DIR_LIMIT)
        .map(|e| Item {
            name: e.file_name().to_string_lossy().into_owned(),
            is_dir: e.file_type().is_ok_and(|t| t.is_dir()),
        })
        .collect();
    items.sort_by_cached_key(|i| (!i.is_dir, i.name.to_lowercase()));
    items
}

/// One row of the middle column after filtering.
pub struct Row {
    pub index: usize,
    /// Char positions matched by the filter, sorted, for highlighting.
    pub hits: Vec<u32>,
}

pub struct Browser {
    pub cwd: PathBuf,
    items: Vec<Item>,
    pub filter: String,
    rows: Vec<Row>,
    pub selected: usize,
    matcher: Matcher,
    /// Names to reselect when going up, keyed by the directory being left.
    remembered: Vec<(PathBuf, String)>,
}

impl Browser {
    pub fn new(cwd: PathBuf) -> Self {
        let mut browser = Self {
            cwd: PathBuf::new(),
            items: Vec::new(),
            filter: String::new(),
            rows: Vec::new(),
            selected: 0,
            matcher: Matcher::new(Config::DEFAULT),
            remembered: Vec::new(),
        };
        browser.load(cwd, None);
        browser
    }

    pub fn items(&self) -> &[Item] {
        &self.items
    }

    pub fn rows(&self) -> &[Row] {
        &self.rows
    }

    pub fn selected_item(&self) -> Option<&Item> {
        self.rows.get(self.selected).map(|r| &self.items[r.index])
    }

    /// Absolute path of the selected entry.
    pub fn selected_path(&self) -> Option<PathBuf> {
        self.selected_item().map(|i| self.cwd.join(&i.name))
    }

    /// The directory Enter would cd into: the selected directory, or the
    /// current one when a file (or nothing) is selected.
    pub fn target(&self) -> PathBuf {
        match self.selected_item() {
            Some(item) if item.is_dir => self.cwd.join(&item.name),
            _ => self.cwd.clone(),
        }
    }

    fn load(&mut self, dir: PathBuf, reselect: Option<&str>) {
        self.items = read_dir(&dir);
        self.cwd = dir;
        self.filter.clear();
        self.apply_filter();
        if let Some(name) = reselect
            && let Some(pos) = self
                .rows
                .iter()
                .position(|r| self.items[r.index].name == name)
        {
            self.selected = pos;
        }
    }

    /// Steps into the selected directory. Files are left alone.
    pub fn enter(&mut self) {
        let Some(item) = self.selected_item() else {
            return;
        };
        if !item.is_dir {
            return;
        }
        let name = item.name.clone();
        let next = self.cwd.join(&name);
        self.remembered.push((self.cwd.clone(), name));
        self.load(next, None);
    }

    /// Goes to the parent directory, reselecting the one just left.
    pub fn up(&mut self) {
        let Some(parent) = self.cwd.parent().map(Path::to_path_buf) else {
            return;
        };
        let left = self
            .cwd
            .file_name()
            .map(|n| n.to_string_lossy().into_owned());
        self.remembered.retain(|(dir, _)| dir != &parent);
        self.load(parent, left.as_deref());
    }

    pub fn move_selection(&mut self, delta: isize) {
        if self.rows.is_empty() {
            self.selected = 0;
            return;
        }
        let last = self.rows.len() - 1;
        let next = self.selected as isize + delta;
        self.selected = next.clamp(0, last as isize) as usize;
    }

    pub fn set_filter(&mut self, filter: &str) {
        self.filter = filter.to_string();
        self.apply_filter();
    }

    /// Recomputes the visible rows. With a filter, rows are ranked by match
    /// score; without one they keep directory order.
    fn apply_filter(&mut self) {
        self.selected = 0;
        if self.filter.is_empty() {
            self.rows = (0..self.items.len())
                .map(|index| Row {
                    index,
                    hits: Vec::new(),
                })
                .collect();
            return;
        }
        let pattern = Pattern::parse(&self.filter, CaseMatching::Ignore, Normalization::Smart);
        let mut buf = Vec::new();
        let mut scored: Vec<(u32, Row)> = self
            .items
            .iter()
            .enumerate()
            .filter_map(|(index, item)| {
                let haystack = Utf32Str::new(&item.name, &mut buf);
                let mut hits = Vec::new();
                let score = pattern.indices(haystack, &mut self.matcher, &mut hits)?;
                hits.sort_unstable();
                hits.dedup();
                Some((score, Row { index, hits }))
            })
            .collect();
        scored.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.index.cmp(&b.1.index)));
        self.rows = scored.into_iter().map(|(_, row)| row).collect();
    }

    /// Entries of the parent directory, with the position of the current one.
    pub fn parent_listing(&self) -> Option<(Vec<Item>, Option<usize>)> {
        let parent = self.cwd.parent()?;
        let items = read_dir(parent);
        let here = self.cwd.file_name().map(|n| n.to_string_lossy());
        let pos = here.and_then(|h| items.iter().position(|i| i.name == h));
        Some((items, pos))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Each test gets its own tree: tests run in parallel inside one process.
    fn fixture(name: &str) -> PathBuf {
        let root =
            std::env::temp_dir().join(format!("thither-browse-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("src/deep")).unwrap();
        std::fs::create_dir_all(root.join("docs")).unwrap();
        std::fs::write(root.join("README.md"), "").unwrap();
        std::fs::write(root.join("src/main.rs"), "").unwrap();
        root
    }

    #[test]
    fn lists_directories_first_then_files() {
        let root = fixture("list");
        let b = Browser::new(root.clone());
        let labels: Vec<String> = b.items().iter().map(Item::label).collect();
        let sep = std::path::MAIN_SEPARATOR;
        assert_eq!(
            labels,
            [
                format!("docs{sep}"),
                format!("src{sep}"),
                "README.md".into()
            ]
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn enter_and_up_keep_the_place() {
        let root = fixture("enter_up");
        let mut b = Browser::new(root.clone());
        b.move_selection(1); // src
        b.enter();
        assert_eq!(b.cwd, root.join("src"));
        assert_eq!(b.selected_item().unwrap().name, "deep");
        b.enter();
        assert_eq!(b.cwd, root.join("src").join("deep"));
        b.up();
        b.up();
        assert_eq!(b.cwd, root);
        assert_eq!(b.selected_item().unwrap().name, "src");
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn enter_on_a_file_does_nothing_and_target_is_cwd() {
        let root = fixture("file");
        let mut b = Browser::new(root.clone());
        b.move_selection(2); // README.md
        b.enter();
        assert_eq!(b.cwd, root);
        assert_eq!(b.target(), root);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn filter_narrows_the_current_level_and_marks_hits() {
        let root = fixture("filter");
        let mut b = Browser::new(root.clone());
        b.set_filter("rd");
        let names: Vec<&str> = b
            .rows()
            .iter()
            .map(|r| b.items()[r.index].name.as_str())
            .collect();
        assert_eq!(names, ["README.md"]);
        assert_eq!(b.rows()[0].hits, vec![0, 3]);
        b.set_filter("");
        assert_eq!(b.rows().len(), 3);
        std::fs::remove_dir_all(root).unwrap();
    }
}
