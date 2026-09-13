#![no_std]
#![doc = include_str!("../README.md")]

extern crate alloc;
#[cfg(any(test, feature = "std"))]
extern crate std;

use alloc::collections::{BTreeMap, BTreeSet};
use alloc::string::String;
use alloc::string::ToString;
use alloc::vec::Vec;
#[cfg(feature = "std")]
use std::eprintln;
#[cfg(feature = "std")]
use std::format;
#[cfg(feature = "std")]
use std::path::{Path, PathBuf};

pub mod config;
#[cfg(feature = "std")]
pub mod discover;
pub mod extract;
pub mod fence;
pub mod normalize;
pub mod report;

pub use report::{human, json};

/// Scan parameters. Values left unset fall back to `.dejadoc.toml`, then
/// to defaults (threshold 2, min-tokens 0).
#[cfg(feature = "std")]
#[derive(Debug, Clone, Default)]
pub struct Dejadoc {
    package: Option<String>,
    all_targets: bool,
    threshold: Option<usize>,
    min_tokens: Option<usize>,
    config: Option<PathBuf>,
}

#[cfg(feature = "std")]
impl Dejadoc {
    /// Restrict the scan to one workspace member by name.
    #[must_use]
    pub fn package(mut self, name: impl Into<String>) -> Self {
        self.package = Some(name.into());
        self
    }

    /// Also scan bin and example targets.
    #[must_use]
    pub fn all_targets(mut self) -> Self {
        self.all_targets = true;
        self
    }

    /// Report only groups with at least this many sites.
    #[must_use]
    pub fn threshold(mut self, n: usize) -> Self {
        self.threshold = Some(n);
        self
    }

    /// Ignore blocks with fewer tokens than this.
    #[must_use]
    pub fn min_tokens(mut self, n: usize) -> Self {
        self.min_tokens = Some(n);
        self
    }

    /// Read parameters from an explicit `.dejadoc.toml` location.
    #[must_use]
    pub fn config(mut self, path: impl Into<PathBuf>) -> Self {
        self.config = Some(path.into());
        self
    }

    /// Scan `root` (any directory inside the workspace) and report
    /// duplicated doctests.
    ///
    /// # Errors
    ///
    /// Fails when `cargo metadata` cannot resolve the workspace, when the
    /// config file cannot be read or is not valid TOML, or when a module
    /// file cannot be canonicalized.
    pub fn run(self, root: impl AsRef<Path>) -> Result<Report> {
        let Self {
            package,
            all_targets,
            threshold,
            min_tokens,
            config,
        } = self;
        let workspace = discover::workspace(root.as_ref(), package.as_deref(), all_targets)?;
        let config_path = config.unwrap_or_else(|| workspace.root.join(".dejadoc.toml"));
        let cfg = config::load(&config_path)?;
        let threshold = threshold.or(cfg.threshold).unwrap_or(2);
        let min_tokens = min_tokens.or(cfg.min_tokens).unwrap_or(0);

        let mut blocks = Vec::new();
        for target in &workspace.targets {
            for (path, file, segments) in discover::module_tree(target)? {
                let prefix = match segments.first() {
                    Some(_) => format!("{}::{}", target.name, segments.join("::")),
                    None => target.name.clone(),
                };
                let file_str = path.to_string_lossy().into_owned();
                let root_str = workspace.root.to_string_lossy().into_owned();
                let read_include = move |p: &str| {
                    let dir = path.parent()?;
                    let inc = dir.join(p);
                    match std::fs::read_to_string(&inc) {
                        Ok(text) => Some((inc.to_string_lossy().into_owned(), text)),
                        Err(err) => {
                            eprintln!(
                                "dejadoc: warning: cannot read doc include {}: {err}",
                                inc.display()
                            );
                            None
                        }
                    }
                };
                blocks.extend(extract::extract(
                    &prefix,
                    &file_str,
                    &file,
                    &root_str,
                    &read_include,
                ));
            }
        }
        Ok(group(&blocks, threshold, min_tokens))
    }
}

/// Failure of a scan.
#[cfg(feature = "std")]
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// `cargo metadata` could not resolve the workspace.
    #[error("workspace: {0}")]
    Workspace(#[source] cargo_metadata::Error),
    /// A file could not be read.
    #[error("I/O: {0}")]
    Io(#[source] std::io::Error),
    /// The config file cannot be read or is not valid TOML.
    #[error("config: {0}")]
    Config(#[source] toml::de::Error),
}

/// A `Result` with [`Error`] as its default error type.
#[cfg(feature = "std")]
pub type Result<T, E = Error> = std::result::Result<T, E>;

#[cfg(feature = "std")]
impl From<std::io::Error> for Error {
    fn from(err: std::io::Error) -> Self {
        Error::Io(err)
    }
}

#[cfg(feature = "std")]
impl From<cargo_metadata::Error> for Error {
    fn from(err: cargo_metadata::Error) -> Self {
        Error::Workspace(err)
    }
}

#[cfg(feature = "std")]
impl From<toml::de::Error> for Error {
    fn from(err: toml::de::Error) -> Self {
        Error::Config(err)
    }
}

/// A doctest block as found in one doc site.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct DocTest {
    /// File the block's text lives in, workspace-relative.
    pub file: String,
    /// Line of the opening fence, 1-based.
    pub line: u32,
    /// Item path, e.g. `mycrate::parser::parse`.
    pub item: String,
    /// Doctest attributes: `no_run`, `should_panic`, …
    pub info: Vec<String>,
    /// Raw block body.
    pub code: String,
    /// The block carries the `dejadoc` allow token.
    #[serde(skip)]
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
    /// Token count of the canonical form.
    #[serde(skip)]
    pub tokens: usize,
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

#[cfg(feature = "std")]
/// Exit status for a finished scan.
#[must_use]
pub fn exit_code(report: &Report, no_fail: bool) -> std::process::ExitCode {
    use std::process::ExitCode;
    if report.groups.is_empty() || no_fail {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(1)
    }
}

/// Group doctests by canonical form, dropping allowed sites, blocks under
/// `min_tokens`, and groups under `threshold`.
#[must_use]
pub fn group(blocks: &[DocTest], threshold: usize, min_tokens: usize) -> Report {
    let total = blocks.len();
    let mut unique: BTreeSet<String> = BTreeSet::new();
    let mut by_hash: BTreeMap<String, (bool, usize, Vec<DocTest>)> = BTreeMap::new();
    for block in blocks {
        if block.allow {
            continue;
        }
        let canonical = normalize::canonicalize(&block.code);
        let hash = blake3::hash(canonical.text.as_bytes())
            .to_hex()
            .as_str()
            .to_string();
        unique.insert(hash.clone());
        if canonical.tokens >= min_tokens {
            by_hash
                .entry(hash)
                .or_insert_with(|| (canonical.unparsed, canonical.tokens, Vec::new()))
                .2
                .push(block.clone());
        }
    }
    let groups = by_hash
        .into_iter()
        .filter(|(_, (_, _, sites))| sites.len() >= threshold)
        .map(|(hash, (unparsed, tokens, mut sites))| {
            sites.sort_by(|a, b| (a.file.as_str(), a.line).cmp(&(b.file.as_str(), b.line)));
            Group {
                id: hash[..8].to_string(),
                hash,
                unparsed,
                tokens,
                sites,
            }
        })
        .collect();
    Report {
        total,
        unique: unique.len(),
        groups,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    fn dt(file: &str, line: u32, item: &str, code: &str, allow: bool) -> DocTest {
        DocTest {
            file: file.to_string(),
            line,
            item: item.to_string(),
            info: Vec::new(),
            code: code.to_string(),
            allow,
        }
    }

    #[test]
    #[cfg(feature = "std")]
    fn run_outside_a_workspace_is_a_workspace_error() {
        let err = Dejadoc::default()
            .run("/nonexistent-dejadoc-root")
            .unwrap_err();
        assert!(matches!(err, Error::Workspace(_)));
    }

    #[test]
    fn allowed_sites_excluded_from_groups() {
        let blocks = vec![
            dt("a.rs", 1, "m::a", "let x = 1;", false),
            dt("b.rs", 2, "m::b", "let x = 1;", true),
            dt("c.rs", 3, "m::c", "let x = 1;", false),
        ];
        let report = group(&blocks, 2, 0);
        assert_eq!(report.total, 3);
        assert_eq!(report.groups.len(), 1);
        assert_eq!(report.groups[0].sites.len(), 2);
    }

    #[test]
    fn threshold_filters_groups() {
        let blocks = vec![
            dt("a.rs", 1, "m::a", "let x = 1;", false),
            dt("b.rs", 2, "m::b", "let x = 1;", false),
        ];
        let report = group(&blocks, 3, 0);
        assert_eq!(report.groups, Vec::new());
        assert_eq!(report.unique, 1);
    }

    #[test]
    fn min_tokens_excludes_from_groups_but_counts_unique() {
        let blocks = vec![
            dt("a.rs", 1, "m::a", "let x = 1;", false),
            dt("b.rs", 2, "m::b", "let x = 1;", false),
        ];
        let report = group(&blocks, 2, 100);
        assert_eq!(report.groups, Vec::new());
        assert_eq!(report.unique, 1);
    }

    #[test]
    fn sites_sorted_by_file_and_line() {
        let blocks = vec![
            dt("b.rs", 9, "m::b", "let x = 1;", false),
            dt("a.rs", 4, "m::a", "let x = 1;", false),
            dt("a.rs", 2, "m::a", "let x = 1;", false),
        ];
        let report = group(&blocks, 2, 0);
        let sites = &report.groups[0].sites;
        let pos: Vec<(String, u32)> = sites.iter().map(|s| (s.file.clone(), s.line)).collect();
        assert_eq!(
            pos,
            vec![
                ("a.rs".to_string(), 2),
                ("a.rs".to_string(), 4),
                ("b.rs".to_string(), 9)
            ]
        );
    }

    #[test]
    fn id_is_hash_prefix() {
        let blocks = vec![
            dt("a.rs", 1, "m::a", "let x = 1;", false),
            dt("b.rs", 2, "m::b", "let x = 1;", false),
        ];
        let report = group(&blocks, 2, 0);
        let g = &report.groups[0];
        assert_eq!(g.id, &g.hash[..8]);
        assert_eq!(g.hash.len(), 64);
    }

    #[test]
    fn unparsed_propagates() {
        let blocks = vec![
            dt("a.rs", 1, "m::a", "@ nope", false),
            dt("b.rs", 2, "m::b", "@ nope", false),
        ];
        let report = group(&blocks, 2, 0);
        assert!(report.groups[0].unparsed);
    }

    #[test]
    fn attributes_do_not_split_groups() {
        let mut a = dt("a.rs", 1, "m::a", "let x = 1;", false);
        a.info = vec!["no_run".to_string()];
        let b = dt("b.rs", 2, "m::b", "let x = 1;", false);
        let report = group(&[a, b], 2, 0);
        assert_eq!(report.groups.len(), 1);
        assert_eq!(report.groups[0].sites[0].info, vec!["no_run".to_string()]);
    }

    #[test]
    fn no_duplicates_is_clean() {
        let blocks = vec![
            dt("a.rs", 1, "m::a", "let x = 1;", false),
            dt("b.rs", 2, "m::b", "let y = 2;", false),
        ];
        let report = group(&blocks, 2, 0);
        assert_eq!(report.groups, Vec::new());
        assert_eq!(report.unique, 2);
        assert_eq!(report.total, 2);
    }
}
