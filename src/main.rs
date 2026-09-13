use clap::Parser;
use std::path::{Path, PathBuf};

use std::process::ExitCode;

#[derive(clap::Parser)]
#[command(
    name = "dejadoc",
    version,
    about = "Find duplicated Rust doctests across a workspace"
)]
struct Args {
    /// Restrict to one workspace member by name.
    #[arg(short, long)]
    package: Option<String>,
    /// Scan bin and example targets in addition to lib targets.
    #[arg(long)]
    all_targets: bool,
    /// Machine-readable output.
    #[arg(long)]
    json: bool,
    /// Report groups with at least this many sites (default 2).
    #[arg(short = 't', long, value_name = "N")]
    threshold: Option<usize>,
    /// Skip blocks with fewer tokens than this (default 0).
    #[arg(long, value_name = "N")]
    min_tokens: Option<usize>,
    /// Explicit `.dejadoc.toml` location.
    #[arg(long, value_name = "PATH")]
    config: Option<PathBuf>,
    /// Exit 0 even when duplicates are found.
    #[arg(long)]
    no_fail: bool,
    /// Print each group's code.
    #[arg(short, long)]
    verbose: bool,
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args()
        .skip(1)
        .filter(|a| a != "dejadoc")
        .collect();
    let args = Args::parse_from(args);
    let opts = dejadoc::Options {
        package: args.package,
        all_targets: args.all_targets,
        threshold: args.threshold,
        min_tokens: args.min_tokens,
        config: args.config,
    };
    match dejadoc::run(Path::new("."), &opts) {
        Ok(report) => {
            let out = if args.json {
                dejadoc::json(&report)
            } else {
                dejadoc::human(&report, args.verbose)
            };
            println!("{out}");
            dejadoc::exit_code(&report, args.no_fail)
        }
        Err(err) => {
            eprintln!("dejadoc: {err:#}");
            ExitCode::from(2)
        }
    }
}
