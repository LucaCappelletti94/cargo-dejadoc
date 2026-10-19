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
    about = "Find duplicated Rust doctests, functions and context blocks across a workspace or GitHub organization"
)]
struct Args {
    /// Restrict to one package by name.
    #[arg(short, long)]
    package: Option<String>,
    /// Scan crates in every accessible repository of a GitHub organization or user, by default
    /// the owner of the current repository.
    #[arg(long, value_name = "ORG", num_args = 0..=1)]
    #[expect(
        clippy::option_option,
        reason = "clap encodes a flag whose value may be omitted as nested options"
    )]
    organization: Option<Option<String>>,
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
    /// Also check for duplicated functions, off by default.
    #[arg(long)]
    functions: bool,
    /// Skip functions whose body counts fewer tokens than this, a path or an operator counting as one (default 30).
    #[arg(long, value_name = "N")]
    fn_min_tokens: Option<usize>,
    /// Also check for duplicated context blocks, off by default.
    #[arg(long)]
    context_blocks: bool,
    /// Skip context blocks whose canonical form counts fewer tokens than this (default 30).
    #[arg(long, value_name = "N")]
    context_min_tokens: Option<usize>,
    /// Local context selection.
    #[arg(long, value_name = "SCOPE")]
    context_select: Option<dejadoc::ContextScope>,
    /// Workspace context comparison corpus.
    #[arg(long, value_name = "SCOPE")]
    context_search: Option<dejadoc::ContextScope>,
    /// Maximum dependency distance for context reuse, zero disables discovery.
    #[arg(long, value_name = "N")]
    context_dependency_depth: Option<usize>,
    /// Include development dependency edges.
    #[arg(long)]
    context_dev_dependencies: bool,
    /// Include build dependency edges.
    #[arg(long)]
    context_build_dependencies: bool,
    /// Explore optional dependency paths outside the configured features.
    #[arg(long)]
    context_all_optional_dependencies: bool,
    /// Cargo features enabled for dependency discovery.
    #[arg(long, value_delimiter = ',', value_name = "FEATURES")]
    context_features: Option<Vec<String>>,
    /// Enable every workspace feature for dependency discovery.
    #[arg(long)]
    context_all_features: bool,
    /// Disable default workspace features for dependency discovery.
    #[arg(long)]
    context_no_default_features: bool,
    /// Target triple used for dependency discovery.
    #[arg(long, value_name = "TRIPLE")]
    context_target: Option<String>,
    /// Persistent analysis cache directory.
    #[arg(long, value_name = "PATH")]
    context_cache_dir: Option<String>,
    /// Check the latest published release for reusable APIs, default true.
    #[arg(long, value_name = "BOOL")]
    context_check_latest: Option<bool>,
    /// Include dependency reuse findings in the duplicate gate.
    #[arg(long)]
    context_fail_on_reuse: bool,
    /// Explicit `.dejadoc.toml` location.
    #[arg(long, value_name = "PATH")]
    config: Option<PathBuf>,
    /// Exit 0 even when duplicates are found.
    #[arg(long)]
    no_fail: bool,
    /// Print code for doctest and function groups.
    #[arg(short, long)]
    verbose: bool,
    /// Also print GitHub workflow annotations for duplicate sites.
    #[arg(long)]
    github: bool,
}

fn main() -> ExitCode {
    let mut args: Vec<String> = std::env::args().collect();
    if args.get(1).is_some_and(|a| a == "dejadoc") {
        args.remove(1);
    }
    match run(Args::parse_from(args)) {
        Ok(code) => code,
        Err(err) => {
            eprintln!("dejadoc: {err}");
            ExitCode::from(2)
        }
    }
}

fn run(args: Args) -> dejadoc::Result<ExitCode> {
    let mut scan = dejadoc::Dejadoc::default();
    if let Some(package) = args.package {
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
    if args.functions {
        scan = scan.functions();
    }
    if let Some(n) = args.fn_min_tokens {
        scan = scan.fn_min_tokens(n);
    }
    if args.context_blocks {
        scan = scan.context_blocks();
    }
    if let Some(n) = args.context_min_tokens {
        scan = scan.context_min_tokens(n);
    }
    if let Some(scope) = args.context_select {
        scan = scan.context_select(scope);
    }
    if let Some(scope) = args.context_search {
        scan = scan.context_search(scope);
    }
    scan = scan.context_dependencies(dejadoc::DependencySearch {
        depth: args.context_dependency_depth,
        include_dev: args.context_dev_dependencies.then_some(true),
        include_build: args.context_build_dependencies.then_some(true),
        all_optional: args.context_all_optional_dependencies.then_some(true),
        features: args.context_features,
        all_features: args.context_all_features.then_some(true),
        no_default_features: args.context_no_default_features.then_some(true),
        target: args.context_target,
        cache_dir: args.context_cache_dir,
        check_latest: args.context_check_latest,
        fail_on_findings: args.context_fail_on_reuse.then_some(true),
    });
    if let Some(config) = args.config {
        scan = scan.config(config)?;
    }
    let report = match args.organization {
        Some(Some(organization)) => scan.run_organization(&organization)?,
        Some(None) => scan.run_organization(&dejadoc::repository_owner(".")?)?,
        None => scan.run(Path::new("."))?,
    };
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
