//! Command-line interface for trash-pro.
//!
//! Usage: trash-pro [OPTIONS] <PATH>...

use std::path::PathBuf;
use std::process::ExitCode;

use trash_pro::{delete_all, Destination, Options};

const USAGE: &str = "\
trash-pro: move files and folders to the OS trash/recycle bin, with a temp-dir fallback

Usage: trash-pro [OPTIONS] <PATH>...

Options:
      --force-temp      Skip the OS trash; move straight into the temp fallback directory
      --no-fallback     Do not fall back to the temp directory; fail when the trash is unavailable
      --temp-dir <DIR>  Fallback destination (default: system temp dir + /trash-pro)
  -h, --help            Print help
  -V, --version         Print version
";

fn main() -> ExitCode {
    let mut opts = Options::default();
    let mut paths: Vec<PathBuf> = Vec::new();

    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "-h" | "--help" => {
                print!("{USAGE}");
                return ExitCode::SUCCESS;
            }
            "-V" | "--version" => {
                println!("trash-pro {}", env!("CARGO_PKG_VERSION"));
                return ExitCode::SUCCESS;
            }
            "--force-temp" => opts.force_temp = true,
            "--no-fallback" => opts.fallback_to_temp = false,
            "--temp-dir" => match args.next() {
                Some(dir) => opts.temp_dir = Some(PathBuf::from(dir)),
                None => {
                    eprintln!("error: --temp-dir requires a value");
                    eprintln!("{USAGE}");
                    return ExitCode::from(2);
                }
            },
            _ => match arg.strip_prefix("--temp-dir=") {
                Some(dir) => opts.temp_dir = Some(PathBuf::from(dir)),
                None if arg.starts_with('-') => {
                    eprintln!("error: unknown option {arg}");
                    eprintln!("{USAGE}");
                    return ExitCode::from(2);
                }
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
