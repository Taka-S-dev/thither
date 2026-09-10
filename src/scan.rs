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
/// Setting `cancel` stops the walk early; `done` is set when the thread is finished either way.
pub fn spawn(
    root: PathBuf,
    mode: Mode,
    exclude: &[String],
    injector: Injector<Entry>,
    done: Arc<AtomicBool>,
    cancel: Arc<AtomicBool>,
) -> JoinHandle<Result<(), String>> {
    if matches!(mode, Mode::Recent | Mode::Favorites) {
        return std::thread::spawn(move || {
            let result = push_saved(&injector, mode);
            done.store(true, Ordering::Release);
            result
        });
    }
    let exclude: Arc<HashSet<OsString>> = Arc::new(exclude.iter().map(OsString::from).collect());
    std::thread::spawn(move || {
        if let Err(error) = check_scan_root(&root) {
            done.store(true, Ordering::Release);
            return Err(error);
        }
        let mut builder = WalkBuilder::new(&root);
        builder
            .standard_filters(false)
            .follow_links(false)
            .threads(std::thread::available_parallelism().map_or(4, |n| n.get()));
        builder.filter_entry(move |entry| {
            !skip_reparse_directory(entry)
                && !(entry.file_type().is_some_and(|t| t.is_dir())
                    && exclude.contains(entry.file_name()))
        });

        builder.build_parallel().run(|| {
            let root = root.clone();
            let injector = injector.clone();
            let cancel = cancel.clone();
            Box::new(move |entry| {
                if cancel.load(Ordering::Relaxed) {
                    return WalkState::Quit;
                }
                if let Ok(entry) = entry
                    && let Some(item) = to_entry(&root, &entry, mode)
                {
                    injector.push(item, |item, cols| cols[0] = item.display.as_str().into());
                }
                WalkState::Continue
            })
        });
        done.store(true, Ordering::Release);
        Ok(())
    })
}

#[cfg(windows)]
fn check_scan_root(root: &Path) -> Result<(), String> {
    check_windows_volume(root)?;
    // Resolve a local junction used as the scan root before walking its target.
    match std::fs::canonicalize(root) {
        Ok(resolved) => check_windows_volume(&resolved),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(format!(
            "Cannot verify scan location: {error}. Use browse instead."
        )),
    }
}

#[cfg(windows)]
fn check_windows_volume(root: &Path) -> Result<(), String> {
    use std::path::{Component, Prefix};
    let absolute = std::path::absolute(root).map_err(|error| error.to_string())?;
    let drive = match absolute.components().next() {
        Some(Component::Prefix(prefix)) => match prefix.kind() {
            Prefix::Disk(drive) | Prefix::VerbatimDisk(drive) => drive,
            _ => return Err(
                "Recursive scan blocked on network or unsupported paths. Use browse (Tab) instead."
                    .into(),
            ),
        },
        _ => return Err("Cannot verify scan drive. Use browse (Tab) instead.".into()),
    };
    let name = [drive as u16, b':' as u16, b'\\' as u16, 0];
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetDriveTypeW(root: *const u16) -> u32;
    }
    // SAFETY: name is a valid, NUL-terminated UTF-16 drive root for this call.
    match unsafe { GetDriveTypeW(name.as_ptr()) } {
        2 | 3 | 5 | 6 => Ok(()),
        _ => Err(
            "Recursive scan blocked on network or unverified drives. Use browse (Tab) instead."
                .into(),
        ),
    }
}

#[cfg(not(windows))]
fn check_scan_root(_root: &Path) -> Result<(), String> {
    Ok(())
}

#[cfg(windows)]
fn skip_reparse_directory(entry: &DirEntry) -> bool {
    use std::os::windows::fs::MetadataExt;
    entry.depth() > 0
        && entry.file_type().is_some_and(|kind| kind.is_dir())
        && entry
            .metadata()
            .map_or(true, |metadata| metadata.file_attributes() & 0x400 != 0)
}

#[cfg(not(windows))]
fn skip_reparse_directory(_entry: &DirEntry) -> bool {
    false
}

#[cfg(all(test, windows))]
mod network_tests {
    use super::*;

    #[test]
    fn unc_paths_are_blocked_without_contacting_the_server() {
        for path in [
            r"\\unreachable.invalid\share",
            r"\\?\UNC\unreachable.invalid\share",
            "//unreachable.invalid/share",
        ] {
            assert!(
                check_scan_root(Path::new(path))
                    .unwrap_err()
                    .contains("blocked")
            );
        }
        assert!(check_scan_root(&std::env::current_dir().unwrap()).is_ok());
    }
}

fn to_entry(root: &Path, entry: &DirEntry, mode: Mode) -> Option<Entry> {
    if entry.depth() == 0 {
        return None;
    }
    let is_dir = entry.file_type()?.is_dir();
    let wanted = match mode {
        Mode::Dirs => is_dir,
        Mode::Files => !is_dir,
        Mode::Recent | Mode::Favorites | Mode::Browse => false,
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

/// Recent directories in zoxide's order (highest score first). zoxide owns the
/// history; reading its database directly would tie tadoru to its file format.
pub fn recent() -> Result<Vec<PathBuf>, String> {
    let output = std::process::Command::new("zoxide")
        .args(["query", "--list"])
        .output()
        .map_err(|err| match err.kind() {
            // Saying only that a program is missing leaves the reader to work
            // out which parts of tadoru still work and what to do about it.
            std::io::ErrorKind::NotFound => {
                "recent mode lists the directories zoxide remembers, and zoxide is not installed. \
                 Install it (winget install ajeetdsouza.zoxide), or use favorites for the places \
                 you care about: Ctrl-B pins the selected folder. dirs, files and browse need nothing."
                    .to_string()
            }
            _ => format!("cannot run zoxide: {err}"),
        })?;
    if !output.status.success() {
        return Err(format!(
            "zoxide query failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter(|l| !l.is_empty())
        .map(PathBuf::from)
        .collect())
}

fn push_saved(injector: &Injector<Entry>, mode: Mode) -> Result<(), String> {
    let paths = if mode == Mode::Favorites {
        crate::favorites::path()
            .and_then(|path| crate::favorites::read(&path))
            .map_err(|error| error.to_string())?
    } else {
        recent()?
    };
    for path in paths {
        let display = path.to_string_lossy().into_owned();
        injector.push(Entry { path, display }, |item, cols| {
            cols[0] = item.display.as_str().into()
        });
    }
    Ok(())
}
