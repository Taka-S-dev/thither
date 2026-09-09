mod config;
mod picker;
mod scan;

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

#[derive(Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum Mode {
    /// Directories below the root.
    Dirs,
    /// Files below the root. Prints the parent directory of the chosen file.
    Files,
    /// Recently visited directories from zoxide.
    Recent,
}

impl Mode {
    pub fn label(self) -> &'static str {
        match self {
            Mode::Dirs => "dirs",
            Mode::Files => "files",
            Mode::Recent => "recent",
        }
    }
}

#[derive(Clone, Copy, ValueEnum)]
enum Shell {
    Powershell,
    Cmd,
    Bash,
}

const EXIT_CANCELLED: u8 = 1;
const EXIT_ERROR: u8 = 2;

fn main() -> ExitCode {
    let cli = Cli::parse();
    let result = match cli.command {
        Command::Pick(args) => pick(args),
        Command::Init { .. } => Err("init is not implemented yet".into()),
    };
    match result {
        Ok(Some(path)) => {
            println!("{}", path.display());
            ExitCode::SUCCESS
        }
        Ok(None) => ExitCode::from(EXIT_CANCELLED),
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
    if args.mode == Mode::Recent {
        return Err("--mode recent is not implemented yet".into());
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
