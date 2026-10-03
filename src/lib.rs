#![no_std]
#![doc = include_str!("../README.md")]

extern crate alloc;
#[cfg(any(test, feature = "std"))]
extern crate std;

use alloc::collections::{BTreeMap, BTreeSet};
use alloc::format;
use alloc::string::String;
use alloc::string::ToString;
use alloc::vec::Vec;
#[cfg(feature = "std")]
use std::eprintln;
#[cfg(feature = "std")]
use std::path::{Path, PathBuf};

mod alpha;
mod cfg;
#[cfg(feature = "std")]
mod config;
#[cfg(feature = "std")]
mod discover;
mod drift;
pub mod extract;
mod fence;
mod normalize;
mod report;

/// Reader for an `include_str!` doc splice, given the containing file and
/// the path as written. Yields the resolved path and text.
type IncludeRead<'a> = dyn Fn(&str, &str) -> Option<(String, String)> + 'a;
pub use report::{annotations, human, json};

/// Scan parameters. In `run`, unset values fall back to `.dejadoc.toml`,
/// then to defaults.
#[derive(Debug, Clone, Default)]
pub struct Dejadoc {
    package: Option<String>,
    all_targets: bool,
    threshold: Option<usize>,
    min_tokens: Option<usize>,
}

impl Dejadoc {
    /// Restrict the scan to one workspace member by name.
    #[must_use]
    pub fn package(mut self, name: impl Into<String>) -> Self {
        self.package = Some(name.into());
        self
    }

    /// Also scan bin and example targets. Applies to `run` only.
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

    /// Read parameters from a `.dejadoc.toml` file at `path`. The values
    /// fill parameters that are not set yet. Under `run` the workspace
    /// root `.dejadoc.toml` still applies as a fallback.
    ///
    /// # Errors
    ///
    /// Fails when the file cannot be read or is not valid TOML.
    #[cfg(feature = "std")]
    pub fn config(mut self, path: impl Into<PathBuf>) -> Result<Self> {
        let file = config::load(&path.into())?;
        self.threshold = self.threshold.or(file.threshold);
        self.min_tokens = self.min_tokens.or(file.min_tokens);
        Ok(self)
    }

    /// Scan `root` (any directory inside the workspace) and report
    /// duplicated doctests. Discovery, parsing and extraction run on the
    /// thread `group` would use, so a deeply nested source file does not
    /// end the caller's stack.
    ///
    /// # Errors
    ///
    /// Fails when `cargo metadata` cannot resolve the workspace, when the
    /// config cannot be read or parsed, or when a module file cannot be
    /// canonicalized.
    ///
    /// # Panics
    ///
    /// Panics when the scan panics, a bug in this crate.
    #[cfg(feature = "std")]
    pub fn run(self, root: impl AsRef<Path>) -> Result<Report> {
        let root = root.as_ref();
        on_large_stack(GROUP_STACK, || self.clone().run_here(root))
    }

    /// `run` on the calling thread's stack.
    #[cfg(feature = "std")]
    fn run_here(self, root: &Path) -> Result<Report> {
        let Self {
            package,
            all_targets,
            threshold,
            min_tokens,
        } = self;
        let workspace = discover::workspace(root, package.as_deref(), all_targets)?;
        let cfg = config::load(&workspace.root.join(".dejadoc.toml"))?;
        let root_str = workspace.root.to_string_lossy().into_owned();
        let read = |file: &str, p: &str| -> Option<(String, String)> {
            let dir = Path::new(file).parent()?;
            let inc = dir.join(p);
            match std::fs::read_to_string(&inc) {
                Ok(text) => Some((inc.to_string_lossy().into_owned(), text)),
                Err(err) => {
                    eprintln!("dejadoc: cannot read doc include {}: {err}", inc.display());
                    None
                }
            }
        };
        let mut targets = Vec::new();
        for target in &workspace.targets {
            targets.push(TargetScan {
                name: target.name.clone(),
                files: discover::module_tree(target)?,
            });
        }
        Ok(group_here(
            &extract_all(&targets, &root_str, &read),
            threshold.or(cfg.threshold).unwrap_or(DEFAULT_THRESHOLD),
            min_tokens.or(cfg.min_tokens).unwrap_or(DEFAULT_MIN_TOKENS),
        ))
    }

    /// Scan resolved targets. Runs no `cargo metadata` and reads no files,
    /// so it works under `no_std`. `root` is stripped from site paths and
    /// `read` resolves an `include_str!` doc splice, given the file that
    /// contains it and the path as written, to the resolved path and text.
    /// `package` filters the passed targets by name. Values loaded
    /// through `config` apply here too.
    #[must_use]
    pub fn run_targets(self, root: &str, targets: &[TargetScan], read: &IncludeRead<'_>) -> Report {
        let Self {
            package,
            all_targets: _,
            threshold,
            min_tokens,
        } = self;
        let blocks = extract_all(
            targets
                .iter()
                .filter(|t| package.as_deref().is_none_or(|p| p == t.name)),
            root,
            read,
        );
        group(
            &blocks,
            threshold.unwrap_or(DEFAULT_THRESHOLD),
            min_tokens.unwrap_or(DEFAULT_MIN_TOKENS),
        )
    }
}

/// One resolved target.
#[derive(Debug, Clone)]
pub struct TargetScan {
    /// The target name, used as the item-path prefix.
    pub name: String,
    /// The target's files.
    pub files: Vec<SourceFile>,
}

/// One file of a target.
#[derive(Clone)]
pub struct SourceFile {
    /// Site path, stripped of the `root` prefix.
    pub path: String,
    /// Module-path segments from the target root, empty for the root file.
    pub segments: Vec<String>,
    /// The parsed file.
    pub parsed: syn::File,
}

impl core::fmt::Debug for SourceFile {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("SourceFile")
            .field("path", &self.path)
            .field("segments", &self.segments)
            .finish_non_exhaustive()
    }
}

/// Sites a group needs when no threshold is set.
const DEFAULT_THRESHOLD: usize = 2;

/// Token floor when no `min-tokens` is set.
const DEFAULT_MIN_TOKENS: usize = 0;

/// The doctests of every file of `targets`.
fn extract_all<'t>(
    targets: impl IntoIterator<Item = &'t TargetScan>,
    root: &str,
    read: &IncludeRead<'_>,
) -> Vec<DocTest> {
    let mut blocks = Vec::new();
    for target in targets {
        for file in &target.files {
            let prefix = match file.segments.first() {
                Some(_) => format!("{}::{}", target.name, file.segments.join("::")),
                None => target.name.clone(),
            };
            blocks.extend(extract::extract(
                &prefix,
                &file.path,
                &file.parsed,
                root,
                read,
            ));
        }
    }
    blocks
}

/// Failure of a scan.
#[cfg(feature = "std")]
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// The workspace could not be resolved.
    #[error("workspace: {0}")]
    Workspace(#[from] cargo_metadata::Error),
    /// A file could not be read.
    #[error("I/O: {0}")]
    Io(#[from] std::io::Error),
    /// The config file could not be read or parsed.
    #[error("config: {0}")]
    Config(#[from] toml::de::Error),
}

/// A `Result` with [`Error`] as its default error type.
#[cfg(feature = "std")]
pub type Result<T, E = Error> = std::result::Result<T, E>;

/// A doctest block as found in one doc site.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct DocTest {
    /// File the block's text lives in, workspace-relative.
    pub file: String,
    /// Line of the opening fence, 1-based.
    pub line: u32,
    /// Line of the closing fence or last indented line, 1-based, `None` when the block
    /// spans an `include_str!` boundary or an escaped-newline literal.
    pub end: Option<u32>,
    /// Item path, e.g. `mycrate::parser::parse`.
    pub item: String,
    /// Doctest attributes such as `no_run` and `should_panic`.
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
    /// True when the canonical form came from the text fallback, for a body
    /// that does not parse or exceeds the nesting or token cap.
    pub unparsed: bool,
    /// Token count of the canonical form.
    pub tokens: usize,
    /// All sites, the one to keep first. Sites whose code names their own
    /// item come first, file and line order breaks ties.
    pub sites: Vec<DocTest>,
}

/// Outcome of a scan.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct Report {
    /// Doctests scanned, allowed ones included, counting a block shared by
    /// several items once.
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
        ExitCode::FAILURE
    }
}

/// Group doctests by canonical form, dropping allowed sites, blocks under
/// `min_tokens`, and groups under `threshold`.
///
/// Blocks sharing a file and line are one doc block reached through several
/// items, so only the first is kept. With the `std` feature the
/// canonicalization runs on a thread with a 1 GiB reserved stack, falling
/// back to the calling thread when the system refuses that thread.
///
/// # Panics
///
/// Panics when canonicalization panics, a bug in this crate.
#[must_use]
pub fn group(blocks: &[DocTest], threshold: usize, min_tokens: usize) -> Report {
    #[cfg(feature = "std")]
    {
        on_large_stack(GROUP_STACK, || group_here(blocks, threshold, min_tokens))
    }
    #[cfg(not(feature = "std"))]
    group_here(blocks, threshold, min_tokens)
}

/// `f` on a thread with `stack` reserved, or on the calling thread when the
/// system refuses that thread.
#[cfg(feature = "std")]
fn on_large_stack<T: Send>(stack: usize, f: impl Fn() -> T + Sync) -> T {
    std::thread::scope(|scope| {
        match std::thread::Builder::new()
            .stack_size(stack)
            .spawn_scoped(scope, &f)
        {
            Ok(worker) => worker.join().expect("the scan does not panic"),
            Err(_) => f(),
        }
    })
}

/// Stack reserved for discovery and canonicalization, room for deeply nested
/// source files and for `MAX_NESTING` and `MAX_TOKENS` bodies with a wide
/// margin even in a debug build.
#[cfg(any(test, feature = "std"))]
const GROUP_STACK: usize = 1 << 30;

/// `group` on the calling thread's stack.
fn group_here(blocks: &[DocTest], threshold: usize, min_tokens: usize) -> Report {
    let mut by_site: BTreeMap<(&str, u32), &DocTest> = BTreeMap::new();
    for block in blocks {
        by_site
            .entry((block.file.as_str(), block.line))
            .or_insert(block);
    }
    let total = by_site.len();
    let mut unique: BTreeSet<String> = BTreeSet::new();
    let mut by_hash: BTreeMap<String, (bool, usize, Vec<DocTest>)> = BTreeMap::new();
    for block in by_site.into_values() {
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
            sites.sort_by_cached_key(|site| !names_its_item(site));
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

/// Whether `site`'s code names its item, the last word of the item path
/// appearing as a whole word. A copy that names another item was most
/// likely copied from it, so the one naming its own item is kept.
fn names_its_item(site: &DocTest) -> bool {
    let word = |c: char| c.is_alphanumeric() || c == '_';
    site.item
        .rsplit(|c: char| !word(c))
        .find(|name| !name.is_empty())
        .is_some_and(|name| site.code.split(|c: char| !word(c)).any(|w| w == name))
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    fn dt(file: &str, line: u32, item: &str, code: &str, allow: bool) -> DocTest {
        DocTest {
            file: file.to_string(),
            line,
            end: None,
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
    #[cfg(feature = "std")]
    fn config_loads_values_immediately() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("config.toml");
        std::fs::write(&file, "threshold = 10\n").unwrap();
        let targets = vec![
            scan_target("alpha", "alpha/src/lib.rs", &[], "one"),
            scan_target("beta", "beta/src/lib.rs", &[], "two"),
        ];
        // The file value applies through the infallible entry point.
        let report =
            Dejadoc::default()
                .config(&file)
                .unwrap()
                .run_targets("", &targets, &|_f, _i| None);
        assert_eq!(report.groups, Vec::new());
        // Builder values set after config still win.
        let report = Dejadoc::default()
            .config(&file)
            .unwrap()
            .threshold(2)
            .run_targets("", &targets, &|_f, _i| None);
        assert_eq!(report.groups.len(), 1);
    }

    #[test]
    #[cfg(feature = "std")]
    fn config_bad_toml_fails_at_build() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("bad.toml");
        std::fs::write(&file, "threshold = \"high\"\n").unwrap();
        let err = Dejadoc::default().config(&file).unwrap_err();
        assert!(matches!(err, Error::Config(_)));
    }

    fn scan_target(name: &str, path: &str, segments: &[&str], item: &str) -> TargetScan {
        let src =
            format!("/// Doc.\n///\n/// ```\n/// fn dup() {{}}\n/// ```\npub fn {item}() {{}}\n");
        target_from(name, path, segments, &src)
    }

    /// A one-file target parsed from `src`.
    fn target_from(name: &str, path: &str, segments: &[&str], src: &str) -> TargetScan {
        TargetScan {
            name: name.to_string(),
            files: vec![SourceFile {
                path: path.to_string(),
                segments: segments.iter().map(ToString::to_string).collect(),
                parsed: match syn::parse_str(src) {
                    Ok(parsed) => parsed,
                    Err(err) => panic!("parse fixture: {err}"),
                },
            }],
        }
    }

    #[test]
    fn run_targets_groups_duplicate_doctests_in_memory() {
        let targets = vec![
            scan_target("alpha", "src/lib.rs", &[], "one"),
            scan_target("beta", "src/parser.rs", &["parser"], "two"),
        ];
        let report = Dejadoc::default().run_targets("", &targets, &|_file, _inc| None);
        assert_eq!(report.total, 2);
        assert_eq!(report.unique, 1);
        assert_eq!(report.groups.len(), 1);
        let sites = &report.groups[0].sites;
        assert_eq!(sites[0].file, "src/lib.rs");
        assert_eq!(sites[0].item, "alpha::one");
        assert_eq!(sites[1].file, "src/parser.rs");
        assert_eq!(sites[1].item, "beta::parser::two");
    }

    #[test]
    fn one_doc_block_shared_by_several_items_is_not_a_duplicate() {
        let shared = "use dioxus::prelude::*;\nlet value = use_signal(|| 0);";
        let blocks = vec![
            dt(
                "docs/rules_of_hooks.md",
                97,
                "hooks::use_context",
                shared,
                false,
            ),
            dt(
                "docs/rules_of_hooks.md",
                97,
                "hooks::use_callback",
                shared,
                false,
            ),
            dt(
                "docs/rules_of_hooks.md",
                97,
                "hooks::use_coroutine",
                shared,
                false,
            ),
        ];
        let report = group(&blocks, 2, 0);
        assert_eq!(report.total, 1);
        assert_eq!(report.unique, 1);
        assert_eq!(report.groups, Vec::new());
    }

    #[test]
    fn shared_doc_block_keeps_grouping_against_a_real_copy() {
        let shared = "let x = 1;\nassert_eq!(x, 1);";
        let blocks = vec![
            dt("docs/guide.md", 12, "crate::beta", shared, false),
            dt("docs/guide.md", 12, "crate::alpha", shared, false),
            dt("src/lib.rs", 40, "crate::copy", shared, false),
        ];
        let report = group(&blocks, 2, 0);
        assert_eq!(report.total, 2);
        assert_eq!(report.groups.len(), 1);
        let sites = &report.groups[0].sites;
        assert_eq!(sites.len(), 2);
        assert_eq!(sites[0].item, "crate::beta");
        assert_eq!(sites[1].item, "crate::copy");
    }

    #[test]
    fn run_targets_groups_alpha_equivalent_doctests() {
        let target = |name: &str, body: &str| {
            let src = format!("/// ```\n/// {body}\n/// ```\npub fn f() {{}}\n");
            target_from(name, &format!("{name}/src/lib.rs"), &[], &src)
        };
        let targets = vec![
            target("alpha", "let pino = 1; pino + 1"),
            target("beta", "let abete = 1; abete + 1"),
        ];
        let report = Dejadoc::default().run_targets("", &targets, &|_file, _inc| None);
        assert_eq!(report.total, 2);
        assert_eq!(report.unique, 1);
        assert_eq!(report.groups.len(), 1);
        assert_eq!(report.groups[0].sites.len(), 2);
    }

    #[test]
    fn source_file_debug_shows_identity() {
        let target = scan_target("alpha", "src/lib.rs", &[], "one");
        let dbg = format!("{:?}", target.files[0]);
        assert!(dbg.contains("SourceFile"));
        assert!(dbg.contains("path: \"src/lib.rs\""));
        assert!(dbg.contains("segments: []"));
    }

    #[test]
    fn run_targets_applies_threshold_and_package() {
        let targets = vec![
            scan_target("alpha", "alpha/src/lib.rs", &[], "one"),
            scan_target("beta", "beta/src/lib.rs", &[], "two"),
        ];
        let narrow = Dejadoc::default()
            .threshold(3)
            .run_targets("", &targets, &|_f, _i| None);
        assert_eq!(narrow.total, 2);
        assert_eq!(narrow.groups, Vec::new());
        let filtered =
            Dejadoc::default()
                .package("beta")
                .threshold(1)
                .run_targets("", &targets, &|_f, _i| None);
        assert_eq!(filtered.total, 1);
        assert_eq!(filtered.groups[0].sites[0].file, "beta/src/lib.rs");
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
    fn a_site_naming_its_item_comes_first() {
        let order = |blocks: &[DocTest]| -> Vec<String> {
            group(blocks, 2, 0).groups[0]
                .sites
                .iter()
                .map(|s| s.item.clone())
                .collect()
        };
        // A test copied from `parse` onto `lex` names only `parse`.
        assert_eq!(
            order(&[
                dt("a.rs", 1, "m::lex", "parse(\"x\");", false),
                dt("b.rs", 2, "m::Parser::parse", "parse(\"x\");", false),
            ]),
            ["m::Parser::parse", "m::lex"]
        );
        // A raw item name counts without its `r#`, a longer word never counts.
        assert_eq!(
            order(&[
                dt("a.rs", 1, "m::read", "read_all(r#match);", false),
                dt("b.rs", 2, "m::r#match", "read_all(r#match);", false),
            ]),
            ["m::r#match", "m::read"]
        );
        // Both or neither naming their item keep file and line order.
        assert_eq!(
            order(&[
                dt("b.rs", 1, "m::f", "f(); g();", false),
                dt("a.rs", 2, "m::g", "f(); g();", false),
            ]),
            ["m::g", "m::f"]
        );
        assert_eq!(
            order(&[
                dt("b.rs", 1, "m::f", "h();", false),
                dt("a.rs", 2, "m::g", "h();", false),
            ]),
            ["m::g", "m::f"]
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

    #[test]
    #[cfg(feature = "std")]
    fn a_long_closure_chain_canonicalizes_on_a_small_caller_stack() {
        // 5,000 chained closures overflow even an 8 MiB stack in a debug build.
        let code = format!("let f = {}1;", "|x| ".repeat(5000));
        let report = std::thread::Builder::new()
            .stack_size(2 << 20)
            .spawn(move || group(&[dt("a.rs", 1, "m::a", &code, false)], 1, 0))
            .unwrap()
            .join()
            .unwrap();
        assert!(!report.groups[0].unparsed);
    }

    #[test]
    #[cfg(feature = "std")]
    fn a_refused_worker_thread_falls_back_to_the_calling_thread() {
        let blocks = vec![
            dt("a.rs", 1, "m::a", "let x = 1;", false),
            dt("b.rs", 2, "m::b", "let y = 1;", false),
        ];
        // No system reserves half the address space for one thread's stack.
        let refused = on_large_stack(usize::MAX / 2, || group_here(&blocks, 2, 0));
        assert_eq!(refused, group(&blocks, 2, 0));
        assert_eq!(refused.groups.len(), 1);
    }

    #[test]
    #[cfg(feature = "std")]
    fn a_deeply_nested_source_file_scans_on_a_small_caller_stack() {
        // 5,000 nested blocks overflow even an 8 MiB stack while `syn` parses them.
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("src")).unwrap();
        std::fs::write(
            dir.path().join("Cargo.toml"),
            "[package]\nname = \"deep\"\nversion = \"0.1.0\"\nedition = \"2021\"\n[workspace]\n",
        )
        .unwrap();
        let body = format!("{}1{}", "{".repeat(5000), "}".repeat(5000));
        let lib = format!("/// ```\n/// let x = 1;\n/// ```\npub fn f() -> u8 {{ {body} }}\n");
        std::fs::write(dir.path().join("src").join("lib.rs"), lib).unwrap();
        let root = dir.path().to_path_buf();
        let report = std::thread::Builder::new()
            .stack_size(2 << 20)
            .spawn(move || Dejadoc::default().run(root))
            .unwrap()
            .join()
            .unwrap()
            .unwrap();
        assert_eq!(report.total, 1);
    }
}
