mod action_menu;
mod actions;
mod browse;
mod config;
mod favorites;
mod icons;
mod open;
mod picker;
mod scan;
mod setup;
mod shim;
#[cfg(test)]
mod testing;

use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, Subcommand, ValueEnum};

/// Directory jumper for cmd.exe, PowerShell and bash.
#[derive(Parser)]
#[command(name = "tadoru", version, about)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Create or validate the user-defined action menu.
    Actions {
        #[command(subcommand)]
        command: ActionsCommand,
    },
    /// Add, remove or list pinned directories (independent of zoxide).
    Favorite {
        #[command(subcommand)]
        command: FavoriteCommand,
    },
    /// Pick a path interactively and print it to stdout.
    Pick(PickArgs),
    /// Put the shell integration in place, after showing what it will write.
    Setup {
        /// Which shell to set up. Detected from the environment when omitted.
        #[arg(value_enum)]
        shell: Option<Shell>,
        /// Write without asking, for an unattended install.
        #[arg(long)]
        yes: bool,
    },
    /// Print shell integration code for the given shell.
    Init {
        #[arg(value_enum)]
        shell: Shell,
        /// Write c, cf, z and zi as script files (.cmd or .ps1) into this directory
        /// instead of printing. A PATH folder holding them and tadoru.exe needs no profile edit.
        #[arg(long, value_name = "DIR")]
        out: Option<PathBuf>,
    },
}

#[derive(Subcommand)]
enum ActionsCommand {
    Init,
    Check,
}

#[derive(Subcommand)]
enum FavoriteCommand {
    /// Pin a directory. Defaults to the current directory.
    Add { path: Option<PathBuf> },
    /// Unpin a directory, including one that no longer exists.
    Remove { path: Option<PathBuf> },
    /// Print the pinned directories.
    List,
}

#[derive(clap::Args)]
pub struct PickArgs {
    /// What to list.
    #[arg(long, value_enum, default_value_t = Mode::Dirs)]
    pub mode: Mode,
    /// Initial query. The TADORU_QUERY environment variable takes precedence.
    #[arg(long, default_value = "")]
    pub query: String,
    /// Directory to scan. Defaults to the current directory.
    #[arg(long)]
    pub root: Option<PathBuf>,
    /// Print the only candidate without showing the picker.
    #[arg(long)]
    pub select_1: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
pub enum Mode {
    /// Directories below the root.
    Dirs,
    /// Files below the root. Prints the parent directory of the chosen file.
    Files,
    /// Recently visited directories from zoxide.
    Recent,
    /// Pinned directories, independent of zoxide.
    Favorites,
    /// Walk the tree one level at a time.
    Browse,
}

impl Mode {
    pub fn label(self) -> &'static str {
        match self {
            Mode::Dirs => "dirs",
            Mode::Files => "files",
            Mode::Recent => "recent",
            Mode::Favorites => "favorites",
            Mode::Browse => "browse",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
enum Shell {
    Powershell,
    Cmd,
    Bash,
}

impl Shell {
    fn label(self) -> &'static str {
        match self {
            Shell::Powershell => "powershell",
            Shell::Cmd => "cmd",
            Shell::Bash => "bash",
        }
    }
}

enum Outcome {
    Path(PathBuf),
    Cancelled,
    Done,
}

const EXIT_CANCELLED: u8 = 1;
const EXIT_ERROR: u8 = 2;

fn main() -> ExitCode {
    let cli = Cli::parse();
    let result = match cli.command {
        Command::Actions { command } => {
            let result = match command {
                ActionsCommand::Init => actions::init(),
                ActionsCommand::Check => actions::check(),
            };
            result
                .map(|path| {
                    eprintln!("{}", path.display());
                    Outcome::Done
                })
                .map_err(Into::into)
        }
        Command::Favorite { command } => favorite(command).map(|()| Outcome::Done),
        Command::Pick(args) => pick(args).map(|p| match p {
            Some(path) => Outcome::Path(path),
            None => Outcome::Cancelled,
        }),
        Command::Setup { shell, yes } => setup(shell, yes).map(|()| Outcome::Done),
        Command::Init { shell, out } => init(shell, out).map(|()| Outcome::Done),
    };
    match result {
        Ok(Outcome::Path(path)) => {
            println!("{}", path.display());
            ExitCode::SUCCESS
        }
        Ok(Outcome::Done) => ExitCode::SUCCESS,
        Ok(Outcome::Cancelled) => ExitCode::from(EXIT_CANCELLED),
        Err(err) => {
            eprintln!("tadoru: {err}");
            ExitCode::from(EXIT_ERROR)
        }
    }
}

fn favorite(command: FavoriteCommand) -> Result<(), Box<dyn std::error::Error>> {
    let file = favorites::path()?;
    let (path, add) = match command {
        FavoriteCommand::List => {
            for path in favorites::read(&file)? {
                println!("{}", path.display());
            }
            return Ok(());
        }
        FavoriteCommand::Add { path } => (path, true),
        FavoriteCommand::Remove { path } => (path, false),
    };
    let path = path.unwrap_or(std::env::current_dir()?);
    favorites::update(&file, &path, Some(add))?;
    eprintln!(
        "{}: {}",
        if add { "Pinned" } else { "Unpinned" },
        path.display()
    );
    Ok(())
}

fn pick(mut args: PickArgs) -> Result<Option<PathBuf>, Box<dyn std::error::Error>> {
    if let Ok(query) = std::env::var("TADORU_QUERY") {
        args.query = query;
    }
    let root = match args.root.take() {
        Some(root) => root,
        None => std::env::current_dir()?,
    };
    let root = std::path::absolute(&root)?;
    if !root.is_dir() {
        return Err(format!("not a directory: {}", root.display()).into());
    }
    let config = config::Config::load()?;
    picker::run(args, root, config)
}

fn setup(shell: Option<Shell>, yes: bool) -> Result<(), Box<dyn std::error::Error>> {
    let plan = setup::plan(shell)?;
    eprintln!("{}", plan.describe());
    if plan.is_noop() {
        return Ok(());
    }
    if !yes && !setup::confirm()? {
        eprintln!("Nothing was written.");
        return Ok(());
    }
    setup::apply(&plan)?;
    eprintln!("Done. Open a new shell, or reload the file, to pick it up.");
    Ok(())
}

fn init(shell: Shell, out: Option<PathBuf>) -> Result<(), Box<dyn std::error::Error>> {
    let written = match (shell, out) {
        (Shell::Powershell, None) => {
            print!("{}", shim::powershell()?);
            return Ok(());
        }
        (Shell::Powershell, Some(dir)) => shim::write_powershell(&dir)?,
        (Shell::Cmd, None) => {
            for (name, body) in shim::cmd()? {
                println!("rem ===== {name}");
                print!("{body}");
            }
            return Ok(());
        }
        (Shell::Cmd, Some(dir)) => shim::write_cmd(&dir)?,
        (Shell::Bash, None) => {
            print!("{}", shim::bash()?);
            return Ok(());
        }
        (Shell::Bash, Some(_)) => {
            return Err("bash needs functions, so use: eval \"$(tadoru init bash)\"".into());
        }
    };
    for path in written {
        eprintln!("wrote {}", path.display());
    }
    Ok(())
}
