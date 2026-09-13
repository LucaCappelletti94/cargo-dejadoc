//! CLI argument parsing and exit codes for the cargo-dejadoc binary.

use clap::Parser;
use std::path::{Path, PathBuf};

use std::process::ExitCode;

#[derive(clap::Parser)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "the bools are clap CLI flags, not an accumulating design"
)]
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
    let mut args: Vec<String> = std::env::args().collect();
    if args.get(1).is_some_and(|a| a == "dejadoc") {
        args.remove(1);
    }
    let args = Args::parse_from(args);
    let scan = match build(&args) {
        Ok(scan) => scan,
        Err(err) => {
            eprintln!("dejadoc: {err}");
            return ExitCode::from(2);
        }
    };
    match scan.run(Path::new(".")) {
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
            eprintln!("dejadoc: {err}");
            ExitCode::from(2)
        }
    }
}

fn build(args: &Args) -> dejadoc::Result<dejadoc::Dejadoc> {
    let mut scan = dejadoc::Dejadoc::default();
    if let Some(package) = &args.package {
        scan = scan.package(package);
    }
    if args.all_targets {
        scan = scan.all_targets();
    }
    if let Some(threshold) = args.threshold {
        scan = scan.threshold(threshold);
    }
    if let Some(min_tokens) = args.min_tokens {
        scan = scan.min_tokens(min_tokens);
    }
    if let Some(config) = &args.config {
        scan = scan.config(config)?;
    }
    Ok(scan)
}
