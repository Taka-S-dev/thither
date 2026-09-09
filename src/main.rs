mod browse;
mod config;
mod open;
mod picker;
mod scan;
mod shim;

use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, Subcommand, ValueEnum};

/// Directory jumper for cmd.exe, PowerShell and bash.
#[derive(Parser)]
#[command(name = "navkit", version, about)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Pick a path interactively and print it to stdout.
    Pick(PickArgs),
    /// Print shell integration code for the given shell.
    Init {
        #[arg(value_enum)]
        shell: Shell,
        /// Write c, cf, z and zi as script files (.cmd or .ps1) into this directory
        /// instead of printing. A PATH folder holding them and navkit.exe needs no profile edit.
        #[arg(long, value_name = "DIR")]
        out: Option<PathBuf>,
    },
}

#[derive(clap::Args)]
pub struct PickArgs {
    /// What to list.
    #[arg(long, value_enum, default_value_t = Mode::Dirs)]
    pub mode: Mode,
    /// Initial query. The NAVKIT_QUERY environment variable takes precedence.
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
    /// Walk the tree one level at a time.
    Browse,
}

impl Mode {
    pub fn label(self) -> &'static str {
        match self {
            Mode::Dirs => "dirs",
            Mode::Files => "files",
            Mode::Recent => "recent",
            Mode::Browse => "browse",
        }
    }
}

#[derive(Clone, Copy, ValueEnum)]
enum Shell {
    Powershell,
    Cmd,
    Bash,
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
        Command::Pick(args) => pick(args).map(|p| match p {
            Some(path) => Outcome::Path(path),
            None => Outcome::Cancelled,
        }),
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
            eprintln!("navkit: {err}");
            ExitCode::from(EXIT_ERROR)
        }
    }
}

fn pick(mut args: PickArgs) -> Result<Option<PathBuf>, Box<dyn std::error::Error>> {
    if let Ok(query) = std::env::var("NAVKIT_QUERY") {
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
            return Err("bash needs functions, so use: eval \"$(navkit init bash)\"".into());
        }
    };
    for path in written {
        eprintln!("wrote {}", path.display());
    }
    Ok(())
}
