//! Command-line interface for trash-pro.
//!
//! Usage:
//!   trash-pro [OPTIONS] <PATH>...
//!   trash-pro list [--temp-dir <DIR>]
//!   trash-pro restore [--overwrite] <ITEM>...

use std::path::PathBuf;
use std::process::ExitCode;

use trash_pro::{delete_all, list_recoverable, restore_all, Destination, ItemKind, Options};

const USAGE: &str = "\
trash-pro: move files and folders to the OS trash/recycle bin, with a temp-dir fallback

Usage: trash-pro [OPTIONS] <PATH>...
       trash-pro list [--temp-dir <DIR>]
       trash-pro restore [--overwrite] <ITEM>...

Commands:
  (default)           Move PATHs to the OS trash (temp fallback when unavailable)
  list                List items in the fallback directory with their original paths
  restore             Move fallback items back to their original paths

Options (default command):
      --force-temp      Skip the OS trash; move straight into the temp fallback directory
      --no-fallback     Do not fall back to the temp directory; fail when the trash is unavailable
      --temp-dir <DIR>  Fallback destination (default: system temp dir + /trash-pro)

Options (list):
      --temp-dir <DIR>  Fallback directory to list (default: system temp dir + /trash-pro)

Options (restore):
      --overwrite       Replace whatever now occupies an item's original location

  -h, --help            Print help
  -V, --version         Print version

Note: to trash a file literally named `list` or `restore`, prefix it (`./list`).
";

fn main() -> ExitCode {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    match argv.first().map(String::as_str) {
        Some("list") => cmd_list(&argv[1..]),
        Some("restore") => cmd_restore(&argv[1..]),
        _ => cmd_trash(&argv),
    }
}

fn help() -> ExitCode {
    print!("{USAGE}");
    ExitCode::SUCCESS
}

fn cmd_trash(argv: &[String]) -> ExitCode {
    let mut opts = Options::default();
    let mut paths: Vec<PathBuf> = Vec::new();

    let mut args = argv.iter();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "-h" | "--help" => return help(),
            "-V" | "--version" => {
                println!("trash-pro {}", env!("CARGO_PKG_VERSION"));
                return ExitCode::SUCCESS;
            }
            "--force-temp" => opts.force_temp = true,
            "--no-fallback" => opts.fallback_to_temp = false,
            "--temp-dir" => match args.next() {
                Some(dir) => opts.temp_dir = Some(PathBuf::from(dir)),
                None => return missing_value("--temp-dir"),
            },
            _ => match arg.strip_prefix("--temp-dir=") {
                Some(dir) => opts.temp_dir = Some(PathBuf::from(dir)),
                None if arg.starts_with('-') => return unknown_option(arg),
                None => paths.push(PathBuf::from(arg)),
            },
        }
    }

    if paths.is_empty() {
        eprintln!("error: at least one PATH is required");
        eprintln!("{USAGE}");
        return ExitCode::from(2);
    }

    let mut failed = false;
    for outcome in delete_all(&paths, &opts) {
        match outcome.result {
            Ok(Destination::Trash) => println!("trashed\t{}", outcome.path.display()),
            Ok(Destination::Temp(dest)) => {
                println!("temp\t{}\t->\t{}", outcome.path.display(), dest.display())
            }
            Err(e) => {
                failed = true;
                println!("error\t{}\t{}", outcome.path.display(), e);
            }
        }
    }

    if failed {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    }
}

fn cmd_list(argv: &[String]) -> ExitCode {
    let mut temp_dir: Option<PathBuf> = None;

    let mut args = argv.iter();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "-h" | "--help" => return help(),
            "--temp-dir" => match args.next() {
                Some(dir) => temp_dir = Some(PathBuf::from(dir)),
                None => return missing_value("--temp-dir"),
            },
            _ => match arg.strip_prefix("--temp-dir=") {
                Some(dir) => temp_dir = Some(PathBuf::from(dir)),
                None => return unknown_option(arg),
            },
        }
    }

    match list_recoverable(temp_dir) {
        Ok(items) => {
            for it in items {
                let kind = match it.kind {
                    ItemKind::File => "file",
                    ItemKind::Directory => "directory",
                    ItemKind::Symlink => "symlink",
                };
                println!(
                    "{}\t{}\t{}\t{}",
                    it.item.display(),
                    it.original.display(),
                    it.deleted_at,
                    kind
                );
            }
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}

fn cmd_restore(argv: &[String]) -> ExitCode {
    let mut overwrite = false;
    let mut items: Vec<PathBuf> = Vec::new();

    for arg in argv {
        match arg.as_str() {
            "-h" | "--help" => return help(),
            "--overwrite" => overwrite = true,
            _ if arg.starts_with('-') => return unknown_option(arg),
            _ => items.push(PathBuf::from(arg)),
        }
    }

    if items.is_empty() {
        eprintln!("error: at least one ITEM is required");
        eprintln!("{USAGE}");
        return ExitCode::from(2);
    }

    let mut failed = false;
    for outcome in restore_all(&items, overwrite) {
        match outcome.result {
            Ok(original) => {
                println!("restored\t{}\t->\t{}", outcome.item.display(), original.display())
            }
            Err(e) => {
                failed = true;
                println!("error\t{}\t{}", outcome.item.display(), e);
            }
        }
    }

    if failed {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    }
}

fn missing_value(option: &str) -> ExitCode {
    eprintln!("error: {option} requires a value");
    eprintln!("{USAGE}");
    ExitCode::from(2)
}

fn unknown_option(arg: &str) -> ExitCode {
    eprintln!("error: unknown option {arg}");
    eprintln!("{USAGE}");
    ExitCode::from(2)
}
