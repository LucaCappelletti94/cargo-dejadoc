//! Find duplicated Rust doctests across a workspace.

use std::path::{Path, PathBuf};

pub mod config;
pub mod discover;
pub mod extract;
pub mod fence;
pub mod normalize;
pub mod report;

pub use report::{human, json};

/// Options for a scan. CLI values win over `.dejadoc.toml` values, which win
/// over defaults.
#[derive(Debug, Clone, Default)]
pub struct Options {
    /// Restrict to one workspace member by name.
    pub package: Option<String>,
    /// Scan bin and example targets in addition to lib targets.
    pub all_targets: bool,
    /// Report groups with at least this many sites.
    pub threshold: Option<usize>,
    pub min_tokens: Option<usize>,
    /// Explicit `.dejadoc.toml` location.
    pub config: Option<PathBuf>,
}

/// A doctest block as found in one doc site.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct DocTest {
    /// File the block's text lives in, workspace-relative.
    pub file: PathBuf,
    /// Line of the opening fence, 1-based.
    pub line: u32,
    /// Item path, e.g. `mycrate::parser::parse`.
    pub item: String,
    /// Doctest attributes: `no_run`, `should_panic`, …
    pub info: Vec<String>,
    /// Raw block body.
    pub code: String,
    /// The block carries the `dejadoc` allow token.
    pub allow: bool,
}

/// Duplicated doctests sharing one canonical form.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct Group {
    /// Stable short id, blake3 hex prefix.
    pub id: String,
    /// Full blake3 hex of the canonical form.
    pub hash: String,
    /// True when the canonical form came from the text fallback.
    pub unparsed: bool,
    /// All sites, ordered by file and line.
    pub sites: Vec<DocTest>,
}

/// Outcome of a scan.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct Report {
    /// Doctests scanned, allowed ones included.
    pub total: usize,
    /// Distinct canonical forms among non-allowed doctests.
    pub unique: usize,
    /// Groups with at least `threshold` sites, ordered by hash.
    pub groups: Vec<Group>,
}

/// Exit status for a finished scan.
pub fn exit_code(report: &Report, no_fail: bool) -> std::process::ExitCode {
    use std::process::ExitCode;
    if report.groups.is_empty() || no_fail {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(1)
    }
}

/// Scan `root` (any directory inside the workspace) and report duplicated
/// doctests.
pub fn run(root: &Path, opts: &Options) -> anyhow::Result<Report> {
    let workspace = discover::workspace(root, opts.package.as_deref(), opts.all_targets)?;
    let config_path = opts
        .config
        .clone()
        .unwrap_or_else(|| workspace.root.join(".dejadoc.toml"));
    let cfg = config::load(&config_path)?;
    let threshold = opts.threshold.or(cfg.threshold).unwrap_or(2);
    let min_tokens = opts.min_tokens.or(cfg.min_tokens).unwrap_or(0);

    let mut blocks = Vec::new();
    for target in &workspace.targets {
        for (path, file) in discover::module_tree(target)? {
            blocks.extend(extract::extract(target, &path, &file, &workspace.root));
        }
    }
    Ok(group(&blocks, threshold, min_tokens))
}

/// Group doctests by canonical form, dropping allowed sites, blocks under
/// `min_tokens`, and groups under `threshold`.
fn group(blocks: &[DocTest], _threshold: usize, _min_tokens: usize) -> Report {
    // Implemented in the normalize + group phase.
    Report {
        total: blocks.len(),
        unique: 0,
        groups: Vec::new(),
    }
}
