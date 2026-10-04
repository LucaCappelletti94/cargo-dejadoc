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
    bin_name = "cargo dejadoc",
    version,
    about = "Find duplicated Rust doctests and functions across a workspace"
)]
struct Args {
    /// Restrict to one workspace member by name.
    #[arg(short, long)]
    package: Option<String>,
    /// Scan bin and example doctests in addition to lib doctests.
    #[arg(long)]
    all_targets: bool,
    /// Machine-readable output.
    #[arg(long)]
    json: bool,
    /// Report groups with at least this many sites (default 2).
    #[arg(short = 't', long, value_name = "N")]
    threshold: Option<usize>,
    /// Skip doctests with fewer tokens than this (default 0).
    #[arg(long, value_name = "N")]
    min_tokens: Option<usize>,
    /// Skip the duplicate function check.
    #[arg(long)]
    no_functions: bool,
    /// Skip functions whose body counts fewer tokens than this, a path or an operator counting as one (default 30).
    #[arg(long, value_name = "N")]
    fn_min_tokens: Option<usize>,
    /// Explicit `.dejadoc.toml` location.
    #[arg(long, value_name = "PATH")]
    config: Option<PathBuf>,
    /// Exit 0 even when duplicates are found.
    #[arg(long)]
    no_fail: bool,
    /// Print each group's code.
    #[arg(short, long)]
    verbose: bool,
    /// Also print GitHub workflow annotations, one per copy to remove.
    #[arg(long)]
    github: bool,
}

fn main() -> ExitCode {
    let mut args: Vec<String> = std::env::args().collect();
    if args.get(1).is_some_and(|a| a == "dejadoc") {
        args.remove(1);
    }
    match run(&Args::parse_from(args)) {
        Ok(code) => code,
        Err(err) => {
            eprintln!("dejadoc: {err}");
            ExitCode::from(2)
        }
    }
}

fn run(args: &Args) -> dejadoc::Result<ExitCode> {
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
    if args.no_functions {
        scan = scan.no_functions();
    }
    if let Some(n) = args.fn_min_tokens {
        scan = scan.fn_min_tokens(n);
    }
    if let Some(config) = &args.config {
        scan = scan.config(config)?;
    }
    let report = scan.run(Path::new("."))?;
    let out = if args.json {
        dejadoc::json(&report)
    } else {
        let mut out = dejadoc::human(&report, args.verbose);
        if args.github {
            out.push('\n');
            out.push_str(&dejadoc::annotations(&report, args.no_fail));
        }
        out
    };
    println!("{out}");
    Ok(dejadoc::exit_code(&report, args.no_fail))
}
