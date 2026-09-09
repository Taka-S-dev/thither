use std::collections::HashSet;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;

use ignore::{DirEntry, WalkBuilder, WalkState};
use nucleo::Injector;

use crate::Mode;

/// One candidate. `display` is the path relative to the root, used for matching and
/// on screen; `path` is what gets printed on selection.
pub struct Entry {
    pub path: PathBuf,
    pub display: String,
}

/// Walks `root` on background threads, pushing entries into `injector` as they are found.
/// `.gitignore` files are not honoured: build outputs are valid destinations too.
pub fn spawn(
    root: PathBuf,
    mode: Mode,
    exclude: &[String],
    injector: Injector<Entry>,
    done: Arc<AtomicBool>,
) -> JoinHandle<()> {
    let exclude: Arc<HashSet<OsString>> = Arc::new(exclude.iter().map(OsString::from).collect());
    std::thread::spawn(move || {
        let mut builder = WalkBuilder::new(&root);
        builder
            .standard_filters(false)
            .follow_links(false)
            .threads(std::thread::available_parallelism().map_or(4, |n| n.get()));
        builder.filter_entry(move |entry| {
            !(entry.file_type().is_some_and(|t| t.is_dir()) && exclude.contains(entry.file_name()))
        });

        builder.build_parallel().run(|| {
            let root = root.clone();
            let injector = injector.clone();
            Box::new(move |entry| {
                if let Ok(entry) = entry
                    && let Some(item) = to_entry(&root, &entry, mode)
                {
                    injector.push(item, |item, cols| cols[0] = item.display.as_str().into());
                }
                WalkState::Continue
            })
        });
        done.store(true, Ordering::Release);
    })
}

fn to_entry(root: &Path, entry: &DirEntry, mode: Mode) -> Option<Entry> {
    if entry.depth() == 0 {
        return None;
    }
    let is_dir = entry.file_type()?.is_dir();
    let wanted = match mode {
        Mode::Dirs => is_dir,
        Mode::Files => !is_dir,
        Mode::Recent => false,
    };
    if !wanted {
        return None;
    }
    let display = entry
        .path()
        .strip_prefix(root)
        .ok()?
        .to_string_lossy()
        .into_owned();
    Some(Entry {
        path: entry.path().to_path_buf(),
        display,
    })
}
