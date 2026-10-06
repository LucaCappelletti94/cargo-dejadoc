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

mod cfg;
#[cfg(feature = "std")]
mod config;
mod contexts;
#[cfg(feature = "std")]
mod discover;
pub mod extract;
mod fence;
mod functions;
mod normalize;
mod report;

/// Reader for an `include_str!` doc splice, given the containing file and
/// the path as written. Yields the resolved path and text.
type IncludeRead<'a> = dyn Fn(&str, &str) -> Option<(String, String)> + 'a;
pub use report::{annotations, human, json};
pub use syn_canon::ContextKind;

/// Scan parameters. In `run`, unset values fall back to `.dejadoc.toml`,
/// then to defaults.
#[derive(Debug, Clone, Default)]
#[expect(clippy::struct_excessive_bools, reason = "independent scan switches")]
pub struct Dejadoc {
    package: Option<String>,
    all_targets: bool,
    threshold: Option<usize>,
    min_tokens: Option<usize>,
    functions: bool,
    fn_min_tokens: Option<usize>,
    context_blocks: bool,
    context_min_tokens: Option<usize>,
    scan_generated: bool,
    generated_markers: Vec<String>,
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

    /// Run the duplicate function check, off by default.
    #[must_use]
    pub fn functions(mut self) -> Self {
        self.functions = true;
        self
    }

    /// Ignore functions whose body counts fewer tokens than this, a path, an operator or a
    /// lifetime counting as one and a comma as none.
    #[must_use]
    pub fn fn_min_tokens(mut self, n: usize) -> Self {
        self.fn_min_tokens = Some(n);
        self
    }

    /// Run approximate lexical context-block detection, off by default.
    #[must_use]
    pub fn context_blocks(mut self) -> Self {
        self.context_blocks = true;
        self
    }

    /// Ignore lexical contexts below this canonical weighted-token count.
    #[must_use]
    pub fn context_min_tokens(mut self, n: usize) -> Self {
        self.context_min_tokens = Some(n);
        self
    }

    /// Scan files marked generated, which are skipped otherwise.
    #[must_use]
    pub fn scan_generated(mut self) -> Self {
        self.scan_generated = true;
        self
    }

    /// Also treat a file as generated when one of its first lines contains `marker`.
    #[must_use]
    pub fn generated_marker(mut self, marker: impl Into<String>) -> Self {
        self.generated_markers.push(marker.into());
        self
    }

    /// Fill unset parameters from the `.dejadoc.toml` at `path`, before the workspace root one under `run`.
    ///
    /// # Errors
    ///
    /// Fails when the file cannot be read or is not valid TOML.
    #[cfg(feature = "std")]
    pub fn config(self, path: impl Into<PathBuf>) -> Result<Self> {
        Ok(self.fill_from(&config::load(&path.into())?))
    }

    /// Fill unset parameters from `file`.
    #[cfg(feature = "std")]
    fn fill_from(self, file: &config::Config) -> Self {
        Self {
            threshold: self.threshold.or(file.threshold),
            min_tokens: self.min_tokens.or(file.min_tokens),
            functions: self.functions || file.functions == Some(true),
            fn_min_tokens: self.fn_min_tokens.or(file.fn_min_tokens),
            context_blocks: self.context_blocks || file.context_blocks == Some(true),
            context_min_tokens: self.context_min_tokens.or(file.context_min_tokens),
            scan_generated: self.scan_generated || file.scan_generated == Some(true),
            generated_markers: self
                .generated_markers
                .into_iter()
                .chain(file.generated_markers.iter().flatten().cloned())
                .collect(),
            ..self
        }
    }

    /// Scan the workspace containing `root` on a large stack and report enabled duplicate categories.
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
        let workspace = discover::workspace(root, self.package.as_deref(), self.all_targets)?;
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
                library: target.library,
            });
        }
        let scan = Self {
            package: None,
            ..self.fill_from(&cfg)
        };
        Ok(scan.scan(&root_str, &targets, &read, group_here))
    }

    /// Scan resolved targets. Runs no `cargo metadata` and reads no files,
    /// so it works under `no_std`. `root` is stripped from site paths and
    /// `read` resolves an `include_str!` doc splice, given the file that
    /// contains it and the path as written, to the resolved path and text.
    /// `package` filters the passed targets by name. Values loaded
    /// through `config` apply here too. Functions and lexical contexts canonicalize on the calling thread.
    #[must_use]
    pub fn run_targets(self, root: &str, targets: &[TargetScan], read: &IncludeRead<'_>) -> Report {
        self.scan(root, targets, read, group)
    }

    /// Group doctests, then run enabled source-syntax checks.
    fn scan(
        self,
        root: &str,
        targets: &[TargetScan],
        read: &IncludeRead<'_>,
        group_doctests: fn(&[DocTest], usize, usize) -> Report,
    ) -> Report {
        let targets: Vec<&TargetScan> = targets
            .iter()
            .filter(|t| self.package.as_deref().is_none_or(|p| p == t.name))
            .collect();
        let threshold = self.threshold.unwrap_or(DEFAULT_THRESHOLD);
        let scanned = |file: &SourceFile| {
            self.scan_generated || !is_generated(&file.text, &self.generated_markers)
        };
        let mut report = group_doctests(
            &extract_all(targets.iter().copied(), root, read, &scanned),
            threshold,
            self.min_tokens.unwrap_or(DEFAULT_MIN_TOKENS),
        );
        if self.functions {
            // Targets sharing a root file are one crate, walked once.
            let mut roots = BTreeSet::new();
            let crates = targets
                .iter()
                .filter(|target| roots.insert(target.files.first().map(|f| f.path.as_str())));
            let sites = crates.flat_map(|target| {
                let crate_root = target.files.first().map_or("", |f| {
                    f.path
                        .strip_prefix(root)
                        .map_or(f.path.as_str(), |p| p.trim_start_matches('/'))
                });
                target
                    .files
                    .iter()
                    .filter(|file| scanned(file))
                    .flat_map(move |file| {
                        functions::functions(
                            &module_prefix(target, file),
                            file,
                            root,
                            crate_root,
                            target.library,
                        )
                    })
            });
            let (total, unique, groups) = group_functions(
                sites,
                threshold,
                self.fn_min_tokens.unwrap_or(DEFAULT_FN_MIN_TOKENS),
            );
            report.functions = total;
            report.unique_functions = unique;
            report.groups.extend(groups);
        }
        if self.context_blocks {
            let mut forms = contexts::Forms::new(
                self.context_min_tokens
                    .unwrap_or(DEFAULT_CONTEXT_MIN_TOKENS),
            );
            for target in &targets {
                for file in target.files.iter().filter(|file| scanned(file)) {
                    forms.file(&module_prefix(target, file), file, root);
                }
            }
            let (total, unique, groups) = forms.finish(threshold);
            report.context_blocks = total;
            report.unique_context_blocks = unique;
            report.context_groups = groups;
        }
        report
    }
}

/// One resolved target.
#[derive(Debug, Clone)]
pub struct TargetScan {
    /// The target name, used as the item-path prefix.
    pub name: String,
    /// The target's files, its root file first, whose path tells two targets of one name apart.
    pub files: Vec<SourceFile>,
    /// Whether the target is a Rust library, whose public functions other crates may call.
    pub library: bool,
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
    /// The file's source text.
    pub text: String,
    /// Whether rustdoc collects doctests from this file.
    pub rustdoc: bool,
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

/// Body-token floor for functions when no `fn-min-tokens` is set.
const DEFAULT_FN_MIN_TOKENS: usize = 30;

/// Weighted-token floor for lexical contexts.
const DEFAULT_CONTEXT_MIN_TOKENS: usize = 30;

/// The module path of `file` in `target`.
fn module_prefix(target: &TargetScan, file: &SourceFile) -> String {
    match file.segments.first() {
        Some(_) => format!("{}::{}", target.name, file.segments.join("::")),
        None => target.name.clone(),
    }
}

/// Whether `text` declares itself generated in its first five lines, where rustfmt looks: by
/// `@generated`, by Go's `// Code generated … DO NOT EDIT.` line, or by one of `markers`.
fn is_generated(text: &str, markers: &[String]) -> bool {
    text.lines().take(5).any(|line| {
        let line = line.trim();
        line.contains("@generated")
            || (line.starts_with("// Code generated ") && line.ends_with(" DO NOT EDIT."))
            || markers.iter().any(|marker| line.contains(marker.as_str()))
    })
}

/// The doctests of every file of `targets` rustdoc collects from and `scanned` keeps.
fn extract_all<'t>(
    targets: impl IntoIterator<Item = &'t TargetScan>,
    root: &str,
    read: &IncludeRead<'_>,
    scanned: &dyn Fn(&SourceFile) -> bool,
) -> Vec<DocTest> {
    let mut blocks = Vec::new();
    for target in targets {
        for file in target.files.iter().filter(|f| f.rustdoc && scanned(f)) {
            blocks.extend(extract::extract(
                &module_prefix(target, file),
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

/// A duplicated site, a doctest block or a function.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct DocTest {
    /// File the site's text lives in, workspace-relative.
    pub file: String,
    /// Line of the opening fence or of a function's first attribute, 1-based.
    pub line: u32,
    /// Line of the closing fence, last indented line or closing brace, 1-based, `None` when a
    /// block spans an `include_str!` boundary or an escaped-newline literal.
    pub end: Option<u32>,
    /// Item path, e.g. `mycrate::parser::parse`.
    pub item: String,
    /// Doctest attributes such as `no_run` and `should_panic`.
    pub info: Vec<String>,
    /// Raw block body, or a function's source text.
    pub code: String,
    /// The site carries the `dejadoc` allow token or comment.
    #[serde(skip)]
    pub allow: bool,
    /// The self type of a method, the trait of a default method.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub self_type: Option<String>,
    /// A `pub` function or method of a library, which other crates may call.
    #[serde(skip_serializing_if = "core::ops::Not::not")]
    pub public: bool,
}

/// What a group's sites are.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    /// Doctest blocks.
    Doctest,
    /// Functions of one module.
    Function,
}

/// What to do with the copies of a function group, the first matching row of the
/// rule table winning.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, serde::Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Remedy {
    /// Methods on different self types, which a generic or a macro can share.
    GenericOrMacro,
    /// Copies a trait or an exported symbol needs, which can't be deleted, so a helper or a
    /// macro shares them.
    HelperOrMacro,
    /// Free functions or inherent methods apart only by `cfg`, one function under both replaces them.
    MergeCfg,
    /// Copies that can go, all but the first deleted or updated.
    Delete,
}

/// Duplicated sites sharing one canonical form.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct Group {
    /// What the sites are.
    pub kind: Kind,
    /// Stable short id, blake3 hex prefix.
    pub id: String,
    /// Full blake3 hex of the canonical form, with the module scope for functions.
    pub hash: String,
    /// True when the canonical form came from the text fallback, for a body
    /// that does not parse or exceeds the nesting or token cap.
    pub unparsed: bool,
    /// Token count of the canonical form, of the body alone for a function, a path, an
    /// operator or a lifetime counting as one and a comma as none.
    pub tokens: usize,
    /// All sites, the one to keep first. Sites whose code names their own
    /// item come first, file and line order breaks ties.
    pub sites: Vec<DocTest>,
    /// What to do with the copies, `None` for doctests, whose copies always go.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub remedy: Option<Remedy>,
}

/// One approximate lexical context match at its original source range.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct ContextSite {
    /// Workspace-relative source file.
    pub file: String,
    /// First source line, 1-based.
    pub line: u32,
    /// First source byte column, 1-based.
    pub column: usize,
    /// Last source line, 1-based.
    pub end: u32,
    /// Exclusive ending source byte column, 1-based.
    pub end_column: usize,
    /// Containing item path.
    pub item: String,
    /// The lexical context boundary.
    #[serde(serialize_with = "contexts::serialize_kind")]
    pub kind: ContextKind,
}

/// Approximate lexical contexts sharing an exact canonical form.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct ContextGroup {
    /// Stable short identifier.
    pub id: String,
    /// Full canonical-form blake3 hash.
    pub hash: String,
    /// Canonical weighted-token count.
    pub tokens: usize,
    /// Source occurrences, ordered by location without a designated keeper.
    pub sites: Vec<ContextSite>,
}

/// Outcome of a scan.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct Report {
    /// Doctests scanned, allowed ones included, counting a block shared by
    /// several items once.
    pub total: usize,
    /// Distinct canonical forms among non-allowed doctests.
    pub unique: usize,
    /// Functions scanned, allowed ones included.
    pub functions: usize,
    /// Distinct canonical forms among non-allowed functions, per module.
    pub unique_functions: usize,
    /// Doctest groups, then function groups, each ordered by hash.
    pub groups: Vec<Group>,
    /// Distinct lexical source contexts scanned.
    pub context_blocks: usize,
    /// Distinct qualifying lexical canonical forms.
    pub unique_context_blocks: usize,
    /// Approximate lexical duplicate groups after containment suppression.
    pub context_groups: Vec<ContextGroup>,
}

#[cfg(feature = "std")]
/// Exit status for a finished scan.
pub fn exit_code(report: &Report, no_fail: bool) -> std::process::ExitCode {
    use std::process::ExitCode;
    if (report.groups.is_empty() && report.context_groups.is_empty()) || no_fail {
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
    let entries = by_site.into_values().filter(|b| !b.allow).map(|block| {
        let canonical = if block.info.iter().any(|tag| tag == "compile_fail") {
            normalize::canonicalize_failing(&block.code)
        } else {
            normalize::canonicalize(&block.code)
        };
        let mut hashed = canonical.text.clone();
        for tag in checked_attributes(&block.info) {
            hashed.push('\n');
            hashed.push_str(tag);
        }
        let hash = blake3::hash(hashed.as_bytes()).to_hex().to_string();
        (
            hash,
            canonical.unparsed,
            canonical.tokens,
            (block.clone(), ()),
        )
    });
    let (unique, groups) = bucket(Kind::Doctest, entries, threshold, min_tokens, |_| None);
    Report {
        total,
        unique,
        functions: 0,
        unique_functions: 0,
        groups,
        context_blocks: 0,
        unique_context_blocks: 0,
        context_groups: Vec::new(),
    }
}

/// The doctest attributes that change what rustdoc checks, sorted: `compile_fail` with its
/// error codes, `should_panic`, `test_harness` and the edition. `no_run` only skips running.
fn checked_attributes(info: &[String]) -> Vec<&str> {
    let mut tags: Vec<&str> = info
        .iter()
        .map(String::as_str)
        .filter(|tag| {
            matches!(*tag, "compile_fail" | "should_panic" | "test_harness")
                || tag.starts_with("edition")
                || (tag.len() == 5
                    && tag.starts_with('E')
                    && tag[1..].bytes().all(|b| b.is_ascii_digit()))
        })
        .collect();
    tags.sort_unstable();
    tags
}

/// The function count, distinct forms and groups, each function canonicalized as a one-item file.
fn group_functions(
    sites: impl Iterator<Item = functions::FnSite>,
    threshold: usize,
    min_tokens: usize,
) -> (usize, usize, Vec<Group>) {
    let mut total = 0;
    let entries = sites
        .inspect(|_| total += 1)
        .filter(|f| !f.site.allow)
        .map(|f| {
            let file = syn::File {
                shebang: None,
                frontmatter: None,
                attrs: Vec::new(),
                items: alloc::vec![syn::Item::Fn(f.func)],
            };
            let canonical = syn_canon::canonicalize(file);
            let tokens = normalize::body_tokens(canonical.clone());
            let keyed = format!("{}\n{canonical}", f.scope);
            let hash = blake3::hash(keyed.as_bytes()).to_hex().to_string();
            (hash, false, tokens, (f.site, f.context))
        });
    let (unique, groups) = bucket(Kind::Function, entries, threshold, min_tokens, |sites| {
        Some(functions::remedy(sites))
    });
    (total, unique, groups)
}

/// One hash's text fallback flag, token count and sites with their context.
type Bucket<C> = (bool, usize, Vec<(DocTest, C)>);

/// The distinct hash count of `entries` and their groups of at least `threshold` sites and
/// `min_tokens` tokens, each site carrying the context `remedy` reads.
fn bucket<C>(
    kind: Kind,
    entries: impl Iterator<Item = (String, bool, usize, (DocTest, C))>,
    threshold: usize,
    min_tokens: usize,
    remedy: impl Fn(&[(DocTest, C)]) -> Option<Remedy>,
) -> (usize, Vec<Group>) {
    let mut unique: BTreeSet<String> = BTreeSet::new();
    let mut by_hash: BTreeMap<String, Bucket<C>> = BTreeMap::new();
    for (hash, unparsed, tokens, site) in entries {
        unique.insert(hash.clone());
        if tokens >= min_tokens {
            by_hash
                .entry(hash)
                .or_insert_with(|| (unparsed, tokens, Vec::new()))
                .2
                .push(site);
        }
    }
    let groups = by_hash
        .into_iter()
        .filter(|(_, (_, _, sites))| sites.len() >= threshold)
        .map(|(hash, (unparsed, tokens, mut sites))| {
            sites.sort_by(|(a, _), (b, _)| {
                (a.file.as_str(), a.line).cmp(&(b.file.as_str(), b.line))
            });
            sites.sort_by_cached_key(|(site, _)| !names_its_item(site));
            let remedy = remedy(&sites);
            Group {
                kind,
                id: hash[..8].to_string(),
                hash,
                unparsed,
                tokens,
                sites: sites.into_iter().map(|(site, _)| site).collect(),
                remedy,
            }
        })
        .collect();
    (unique.len(), groups)
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
            self_type: None,
            public: false,
        }
    }

    #[test]
    fn attributes_that_change_what_rustdoc_checks_split_a_group() {
        // diesel's connection docs compile on one backend and must fail on the others.
        let pair = |a: &[&str], b: &[&str]| {
            let mut one = dt("a.rs", 1, "m::f", "let x = 1;", false);
            let mut two = dt("b.rs", 2, "m::g", "let x = 1;", false);
            one.info = a.iter().map(ToString::to_string).collect();
            two.info = b.iter().map(ToString::to_string).collect();
            group(&[one, two], 2, 0).groups.len()
        };
        for split in [
            (&["compile_fail"][..], &[][..]),
            (&["should_panic"][..], &[][..]),
            (&["test_harness"][..], &[][..]),
            (&["edition2015"][..], &["edition2021"][..]),
            (
                &["compile_fail", "E0277"][..],
                &["compile_fail", "E0308"][..],
            ),
        ] {
            assert_eq!(pair(split.0, split.1), 0, "{split:?}");
        }
        assert_eq!(pair(&["no_run"], &[]), 1);
        for unchecked in ["Eabcd", "12345", "E02777"] {
            assert_eq!(pair(&[unchecked], &[]), 1, "{unchecked}");
        }
        assert_eq!(
            pair(&["should_panic", "no_run"], &["no_run", "should_panic"]),
            1
        );
    }

    #[test]
    fn a_compile_fail_doctest_keeps_the_mut_it_may_be_about() {
        let pair = |info: &[&str]| {
            let mut one = dt("a.rs", 1, "m::f", "let v = Vec::new();\nv.push(1);", false);
            let mut two = dt(
                "b.rs",
                2,
                "m::g",
                "let mut v = Vec::new();\nv.push(1);",
                false,
            );
            one.info = info.iter().map(ToString::to_string).collect();
            two.info.clone_from(&one.info);
            group(&[one, two], 2, 0).groups.len()
        };
        assert_eq!(pair(&[]), 1);
        assert_eq!(pair(&["no_run"]), 1);
        assert_eq!(pair(&["compile_fail"]), 0);
        assert_eq!(pair(&["compile_fail", "E0596"]), 0);
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
        std::fs::write(&file, "min-tokens = 1000\n").unwrap();
        let report =
            Dejadoc::default()
                .config(&file)
                .unwrap()
                .run_targets("", &targets, &|_f, _i| None);
        assert_eq!(report.groups, Vec::new());
    }

    #[test]
    #[cfg(feature = "std")]
    fn config_turns_the_function_check_on_or_lowers_its_floor() {
        let dir = tempfile::tempdir().unwrap();
        let src = "fn one() -> u8 { 1 + 2 }\nfn two() -> u8 { 1 + 2 }\n";
        let scan_with = |toml: &str| {
            let file = dir.path().join("config.toml");
            std::fs::write(&file, toml).unwrap();
            let target = target_from("c", "src/lib.rs", &[], src);
            let scan = Dejadoc::default().config(&file).unwrap();
            scan.run_targets("", &[target], &|_f, _i| None).groups.len()
        };
        assert_eq!(scan_with("fn-min-tokens = 0\n"), 0);
        assert_eq!(scan_with("fn-min-tokens = 0\nfunctions = false\n"), 0);
        assert_eq!(scan_with("fn-min-tokens = 0\nfunctions = true\n"), 1);
    }

    #[test]
    fn the_function_check_runs_only_when_enabled() {
        let src = format!("fn one{BODY}\nfn two{BODY}\n");
        let scan = |scan: Dejadoc| {
            let target = target_from("c", "src/lib.rs", &[], &src);
            let report = scan.run_targets("", &[target], &|_f, _i| None);
            (report.functions, report.groups.len())
        };
        assert_eq!(scan(Dejadoc::default()), (0, 0));
        assert_eq!(scan(Dejadoc::default().functions()), (2, 1));
    }

    #[test]
    #[cfg(feature = "std")]
    fn config_marks_more_files_generated_or_scans_them_anyway() {
        let dir = tempfile::tempdir().unwrap();
        let groups = |header: &str, toml: &str, builder: Dejadoc| {
            let file = dir.path().join("config.toml");
            std::fs::write(&file, toml).unwrap();
            let src = format!("{header}fn one() -> u8 {{ 1 + 2 }}\nfn two() -> u8 {{ 1 + 2 }}\n");
            fn_groups(builder.fn_min_tokens(0).config(&file).unwrap(), &src).len()
        };
        let marked = "// @generated\n";
        let bindgen = "/* automatically generated by rust-bindgen */\n";
        assert_eq!(groups(marked, "", Dejadoc::default()), 0);
        assert_eq!(
            groups(marked, "scan-generated = true\n", Dejadoc::default()),
            1
        );
        assert_eq!(
            groups(marked, "scan-generated = false\n", Dejadoc::default()),
            0
        );
        assert_eq!(
            groups(
                bindgen,
                "generated-markers = [\"rust-bindgen\"]\n",
                Dejadoc::default()
            ),
            0
        );
        // A builder marker survives a config file that names other markers.
        let builder = Dejadoc::default().generated_marker("rust-bindgen");
        assert_eq!(
            groups(bindgen, "generated-markers = [\"protoc\"]\n", builder),
            0
        );
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
                text: src.to_string(),
                rustdoc: true,
            }],
            library: true,
        }
    }

    /// A function whose body counts 33 tokens, above the default floor.
    const BODY: &str = "(values: &[u32], limit: u32) -> u32 { let mut total = 0; for value in values { if *value > limit { total += value * 2; } else { total -= 1; } } let doubled = total + limit; doubled }";

    /// The function groups of a one-file scan of `src` with the function check on, as item lists.
    fn fn_groups(scan: Dejadoc, src: &str) -> Vec<Vec<String>> {
        let report = scan.functions().run_targets(
            "",
            &[target_from("c", "src/lib.rs", &[], src)],
            &|_f, _i| None,
        );
        report
            .groups
            .iter()
            .filter(|g| g.kind == Kind::Function)
            .map(|g| g.sites.iter().map(|s| s.item.clone()).collect())
            .collect()
    }

    #[test]
    fn a_pub_function_or_method_of_a_library_is_public() {
        let public = |src: &str, library: bool| -> Vec<bool> {
            let mut target = target_from("c", "src/lib.rs", &[], src);
            target.library = library;
            let report = Dejadoc::default()
                .functions()
                .run_targets("", &[target], &|_f, _i| None);
            report.groups[0].sites.iter().map(|s| s.public).collect()
        };
        let free = format!("fn one{BODY}\npub fn two{BODY}\npub(crate) fn three{BODY}\n");
        assert_eq!(public(&free, true), [false, true, false]);
        assert_eq!(public(&free, false), [false, false, false]);
        let methods = format!("struct A;\nimpl A {{ fn one{BODY}\npub fn two{BODY} }}\n");
        assert_eq!(public(&methods, true), [false, true]);
    }

    #[test]
    fn a_cfg_attr_counts_only_when_it_wraps_a_live_attribute() {
        for wrapped in [
            "inline",
            "allow(dead_code), inline(always)",
            "doc = \"x\"",
            "cfg_attr(windows, cold)",
        ] {
            let src = format!("#[cfg_attr(unix, {wrapped})]\nfn one{BODY}\nfn two{BODY}\n");
            assert_eq!(
                fn_groups(Dejadoc::default(), &src),
                [["c::one", "c::two"]],
                "{wrapped}"
            );
        }
        for wrapped in [
            "case(1)",
            "inline, test",
            "cfg_attr(windows, should_panic)",
            "doc = include_str!(\"a.md\")",
        ] {
            let src = format!("#[cfg_attr(unix, {wrapped})]\nfn one{BODY}\nfn two{BODY}\n");
            assert_eq!(
                fn_groups(Dejadoc::default(), &src),
                Vec::<Vec<String>>::new(),
                "{wrapped}"
            );
        }
    }

    #[test]
    fn functions_differing_in_name_visibility_and_inert_attributes_group() {
        let src = format!(
            "pub fn one{BODY}\n#[inline]\n#[cfg(unix)]\n/// Doc.\nfn two{BODY}\n#[must_use]\npub(crate) fn three{BODY}\n"
        );
        assert_eq!(
            fn_groups(Dejadoc::default(), &src),
            [["c::one", "c::two", "c::three"]]
        );
    }

    #[test]
    fn function_hashes_do_not_depend_on_the_workspace_location() {
        let src = format!("fn one{BODY}\nfn two{BODY}\n");
        let hash = |root: &str| {
            let target = target_from("c", &format!("{root}/src/lib.rs"), &[], &src);
            Dejadoc::default()
                .functions()
                .run_targets(root, &[target], &|_f, _i| None)
                .groups[0]
                .hash
                .clone()
        };
        assert_eq!(hash("/base"), hash("/head"));
    }

    #[test]
    fn methods_of_types_sharing_a_last_segment_span_types() {
        for (one, two) in [
            ("Foo<u8>", "Foo<u16>"),
            ("a::Foo", "b::Foo"),
            ("dyn Shape", "dyn Area"),
        ] {
            let src = format!("impl {one} {{ fn run{BODY} }}\nimpl {two} {{ fn run{BODY} }}\n");
            let report = Dejadoc::default().functions().run_targets(
                "",
                &[target_from("c", "src/lib.rs", &[], &src)],
                &|_f, _i| None,
            );
            let types: Vec<Option<&str>> = report.groups[0]
                .sites
                .iter()
                .map(|s| s.self_type.as_deref())
                .collect();
            assert_eq!(types, [Some(one), Some(two)]);
            assert_eq!(
                report.groups[0].remedy,
                Some(Remedy::GenericOrMacro),
                "{one} {two}"
            );
        }
    }

    #[test]
    fn a_file_marked_generated_in_its_first_five_lines_is_skipped() {
        let body = format!(
            "/// ```\n/// let x = 1;\n/// let y = x + 1;\n/// ```\nfn one{BODY}\n/// ```\n/// let x = 1;\n/// let y = x + 1;\n/// ```\nfn two{BODY}\n"
        );
        let scan = |header: &str, dejadoc: Dejadoc| {
            let target = target_from("c", "src/lib.rs", &[], &format!("{header}{body}"));
            let report = dejadoc
                .functions()
                .run_targets("", &[target], &|_f, _i| None);
            (report.total, report.functions, report.groups.len())
        };
        for header in [
            "// This file is @generated by syn-internal-codegen.\n",
            "\n\n\n\n// @generated\n",
            "// Code generated by software.amazon.smithy.rust.codegen.smithy-rs. DO NOT EDIT.\n",
        ] {
            assert_eq!(scan(header, Dejadoc::default()), (0, 0, 0), "{header}");
            assert_eq!(
                scan(header, Dejadoc::default().scan_generated()),
                (2, 2, 2),
                "{header}"
            );
        }
        for header in [
            "\n\n\n\n\n// @generated\n",
            "// Code generated by hand, edit freely.\n",
            "/* automatically generated by rust-bindgen 0.72.1 */\n",
        ] {
            assert_eq!(scan(header, Dejadoc::default()), (2, 2, 2), "{header}");
        }
        let bindgen = "/* automatically generated by rust-bindgen 0.72.1 */\n";
        assert_eq!(
            scan(
                bindgen,
                Dejadoc::default().generated_marker("by rust-bindgen")
            ),
            (0, 0, 0)
        );
    }

    #[test]
    fn a_methods_name_does_not_bind_the_free_function_its_body_calls() {
        // tauri-plugin-dialog: each method forwards to the free function of its own name.
        let src = format!(
            "struct A;\nimpl A {{ fn pick_files{BODY}\nfn pick_folders{BODY} }}\nimpl A {{\nfn one(self, values: &[u32], limit: u32) -> u32 {{ let mut total = 0; for value in values {{ if *value > limit {{ total += pick_files(values, limit); }} else {{ total -= 1; }} }} total }}\nfn two(self, values: &[u32], limit: u32) -> u32 {{ let mut total = 0; for value in values {{ if *value > limit {{ total += pick_folders(values, limit); }} else {{ total -= 1; }} }} total }}\n}}\n"
        );
        assert_eq!(
            fn_groups(Dejadoc::default(), &src),
            [["c::A::pick_files", "c::A::pick_folders"]]
        );
        let methods = "struct A;\nimpl A {\nfn one(self, values: &[u32], limit: u32) -> u32 { let mut total = 0; for value in values { if *value > limit { total += one(values, limit); } else { total -= 1; } } let doubled = total + limit; doubled }\nfn two(self, values: &[u32], limit: u32) -> u32 { let mut total = 0; for value in values { if *value > limit { total += two(values, limit); } else { total -= 1; } } let doubled = total + limit; doubled }\n}\n";
        assert_eq!(
            fn_groups(Dejadoc::default(), methods),
            Vec::<Vec<String>>::new()
        );
        // A free function's name is in scope, so recursion still renames with it.
        let free = "fn one(values: &[u32], limit: u32) -> u32 { let mut total = 0; for value in values { if *value > limit { total += one(values, limit); } else { total -= 1; } } let doubled = total + limit; doubled }\nfn two(values: &[u32], limit: u32) -> u32 { let mut total = 0; for value in values { if *value > limit { total += two(values, limit); } else { total -= 1; } } let doubled = total + limit; doubled }\n";
        assert_eq!(fn_groups(Dejadoc::default(), free), [["c::one", "c::two"]]);
    }

    #[test]
    fn two_targets_sharing_a_root_file_are_one_crate() {
        // allo-isolate lists `tests/containers.rs` as both an example and a test.
        let src = format!("fn one{BODY}\nfn two{BODY}\n");
        let targets = [
            target_from("containers", "tests/containers.rs", &[], &src),
            target_from("containers", "tests/containers.rs", &[], &src),
        ];
        let report = Dejadoc::default()
            .functions()
            .run_targets("", &targets, &|_f, _i| None);
        let items: Vec<Vec<&str>> = report
            .groups
            .iter()
            .map(|g| g.sites.iter().map(|s| s.item.as_str()).collect())
            .collect();
        assert_eq!(items, [["containers::one", "containers::two"]]);
        assert_eq!((report.functions, report.unique_functions), (2, 1));
    }

    #[test]
    fn two_targets_sharing_a_name_are_two_crates() {
        // Two packages may each hold a `show_posts` bin.
        let src = format!("fn main{BODY}\n");
        let targets = [
            target_from("show_posts", "mysql/src/bin/show_posts.rs", &[], &src),
            target_from("show_posts", "pg/src/bin/show_posts.rs", &[], &src),
        ];
        let report = Dejadoc::default()
            .functions()
            .run_targets("", &targets, &|_f, _i| None);
        assert_eq!(report.groups, Vec::new());
        assert_eq!(report.unique_functions, 2);
    }

    #[test]
    fn functions_compare_within_one_module_only() {
        let src = format!("fn one{BODY}\nmod inner {{ fn two{BODY} }}\n");
        assert_eq!(
            fn_groups(Dejadoc::default(), &src),
            Vec::<Vec<String>>::new()
        );
        let src = format!("mod inner {{ fn one{BODY}\nfn two{BODY} }}\n");
        assert_eq!(
            fn_groups(Dejadoc::default(), &src),
            [["c::inner::one", "c::inner::two"]]
        );
    }

    #[test]
    fn a_free_function_and_a_method_of_one_body_span_types() {
        let src = format!(
            "struct A;\nfn run{BODY}\nimpl A {{ fn run{BODY} }}\nimpl A {{ fn again{BODY} }}\n"
        );
        let report = Dejadoc::default().functions().run_targets(
            "",
            &[target_from("c", "src/lib.rs", &[], &src)],
            &|_f, _i| None,
        );
        let group = &report.groups[0];
        let types: Vec<Option<&str>> = group.sites.iter().map(|s| s.self_type.as_deref()).collect();
        assert_eq!(types, [None, Some("A"), Some("A")]);
        assert_eq!(group.remedy, Some(Remedy::GenericOrMacro));
        let same = format!("struct A;\nimpl A {{ fn run{BODY} }}\nimpl A {{ fn again{BODY} }}\n");
        let report = Dejadoc::default().functions().run_targets(
            "",
            &[target_from("c", "src/lib.rs", &[], &same)],
            &|_f, _i| None,
        );
        assert_eq!(report.groups[0].remedy, Some(Remedy::Delete));
    }

    /// The remedy of the one function group of `src`.
    fn remedy_of(src: &str) -> Option<Remedy> {
        let report = Dejadoc::default().functions().run_targets(
            "",
            &[target_from("c", "src/lib.rs", &[], src)],
            &|_f, _i| None,
        );
        assert_eq!(report.groups.len(), 1, "{src}");
        report.groups[0].remedy
    }

    #[test]
    fn every_function_group_gets_the_remedy_of_its_first_matching_rule() {
        let rows = [
            // Trait methods of one type, a trait impl or a default method.
            (
                format!(
                    "struct A;\ntrait P {{}}\ntrait Q {{}}\nimpl P for A {{ fn a{BODY} }}\nimpl Q for A {{ fn b{BODY} }}\n"
                ),
                Remedy::HelperOrMacro,
            ),
            (
                format!("trait T {{ fn a{BODY}\nfn b{BODY} }}\n"),
                Remedy::HelperOrMacro,
            ),
            (
                format!(
                    "struct A;\ntrait P {{}}\nimpl A {{ fn a{BODY} }}\nimpl P for A {{ fn b{BODY} }}\n"
                ),
                Remedy::HelperOrMacro,
            ),
            // An exported symbol can't be deleted either.
            (
                format!("#[no_mangle]\npub extern \"C\" fn a{BODY}\npub extern \"C\" fn b{BODY}\n"),
                Remedy::HelperOrMacro,
            ),
            (
                format!(
                    "#[unsafe(no_mangle)]\npub extern \"C\" fn a{BODY}\npub extern \"C\" fn b{BODY}\n"
                ),
                Remedy::HelperOrMacro,
            ),
            (
                format!(
                    "pub extern \"C\" fn a{BODY}\n#[unsafe(export_name = \"b\")]\npub extern \"C\" fn b{BODY}\n"
                ),
                Remedy::HelperOrMacro,
            ),
            (
                format!(
                    "#[export_name = \"a\"]\npub extern \"C\" fn a{BODY}\npub extern \"C\" fn b{BODY}\n"
                ),
                Remedy::HelperOrMacro,
            ),
            // Free functions and inherent methods apart only by their `cfg`.
            (
                format!("#[cfg(unix)]\nfn a{BODY}\nfn b{BODY}\n"),
                Remedy::MergeCfg,
            ),
            (
                format!(
                    "struct A;\nimpl A {{ #[cfg(unix)] fn a{BODY}\n#[cfg(windows)] fn b{BODY} }}\n"
                ),
                Remedy::MergeCfg,
            ),
            // A `cfg` on an enclosing inline module or impl block counts too.
            (
                format!(
                    "#[cfg(unix)]\nmod m {{ fn a{BODY} }}\n#[cfg(not(unix))]\nmod m {{ fn a{BODY} }}\n"
                ),
                Remedy::MergeCfg,
            ),
            (
                format!(
                    "struct A;\n#[cfg(unix)]\nimpl A {{ fn a{BODY} }}\n#[cfg(not(unix))]\nimpl A {{ fn a{BODY} }}\n"
                ),
                Remedy::MergeCfg,
            ),
            (
                format!(
                    "#[cfg(unix)]\nmod m {{ #[cfg(test)] fn a{BODY}\n#[cfg(test)] fn b{BODY} }}\n"
                ),
                Remedy::Delete,
            ),
            // One `cfg` set in any order deletes, and `cfg_attr` is no configuration.
            (
                format!("#[cfg_attr(unix, inline)]\nfn a{BODY}\nfn b{BODY}\n"),
                Remedy::Delete,
            ),
            (
                format!(
                    "#[cfg(unix)]\n#[cfg(feature = \"x\")]\nfn a{BODY}\n#[cfg(feature = \"x\")]\n#[cfg(unix)]\nfn b{BODY}\n"
                ),
                Remedy::Delete,
            ),
            (format!("fn a{BODY}\nfn b{BODY}\n"), Remedy::Delete),
            // An earlier row wins over a later one.
            (
                format!(
                    "struct A;\nstruct B;\ntrait P {{}}\nimpl P for A {{ #[cfg(unix)] fn a{BODY} }}\nimpl P for B {{ fn a{BODY} }}\n"
                ),
                Remedy::GenericOrMacro,
            ),
            (
                format!(
                    "struct A;\ntrait P {{}}\ntrait Q {{}}\nimpl P for A {{ #[cfg(unix)] fn a{BODY} }}\nimpl Q for A {{ fn b{BODY} }}\n"
                ),
                Remedy::HelperOrMacro,
            ),
        ];
        for (src, remedy) in rows {
            assert_eq!(remedy_of(&src), Some(remedy), "{src}");
        }
    }

    #[test]
    fn doctest_groups_carry_no_remedy() {
        let blocks = [
            dt("a.rs", 1, "m::f", "let x = 1;", false),
            dt("b.rs", 2, "m::g", "let x = 1;", false),
        ];
        assert_eq!(group(&blocks, 2, 0).groups[0].remedy, None);
    }

    #[test]
    fn methods_of_different_self_types_group_and_report_their_types() {
        let src = format!(
            "struct A;\nstruct B;\nimpl A {{ fn run{BODY} }}\nimpl B {{ fn run{BODY} }}\ntrait T {{ fn go{BODY} }}\n"
        );
        let report = Dejadoc::default().functions().run_targets(
            "",
            &[target_from("c", "src/lib.rs", &[], &src)],
            &|_f, _i| None,
        );
        let group = report
            .groups
            .iter()
            .find(|g| g.kind == Kind::Function)
            .unwrap();
        let sites: Vec<(&str, Option<&str>)> = group
            .sites
            .iter()
            .map(|s| (s.item.as_str(), s.self_type.as_deref()))
            .collect();
        assert_eq!(
            sites,
            [
                ("c::A::run", Some("A")),
                ("c::B::run", Some("B")),
                ("c::T::go", Some("T"))
            ]
        );
    }

    #[test]
    fn signature_and_test_attributes_keep_functions_apart() {
        let wide = BODY.replace("u32", "u64");
        for src in [
            format!("fn one{BODY}\nfn two{wide}\n"),
            format!("#[test]\nfn one{BODY}\nfn two{BODY}\n"),
            format!("#[test]\n#[should_panic]\nfn one{BODY}\n#[test]\nfn two{BODY}\n"),
            format!("#[test]\n#[ignore]\nfn one{BODY}\n#[test]\nfn two{BODY}\n"),
        ] {
            assert_eq!(
                fn_groups(Dejadoc::default(), &src),
                Vec::<Vec<String>>::new(),
                "{src}"
            );
        }
    }

    #[test]
    fn the_function_floor_counts_the_body_not_the_signature() {
        // diesel's `&'a str::from_sql` pair: 30 tokens as whole functions, 6 in the body.
        let src = "struct S;\ntrait A {}\ntrait B {}\nimpl A for S {\nfn from_sql(value: &mut PgValue<'_>) -> deserialize::Result<Self> { Ok(core::str::from_utf8(value.as_bytes())?) }\n}\nimpl B for S {\nfn from_sql(value: &mut PgValue<'_>) -> deserialize::Result<Self> { Ok(core::str::from_utf8(value.as_bytes())?) }\n}\n";
        assert_eq!(
            fn_groups(Dejadoc::default(), src),
            Vec::<Vec<String>>::new()
        );
        assert_eq!(fn_groups(Dejadoc::default().fn_min_tokens(6), src).len(), 1);
        assert_eq!(fn_groups(Dejadoc::default().fn_min_tokens(7), src).len(), 0);
    }

    #[test]
    fn the_function_floor_defaults_to_30_tokens_and_is_settable() {
        let src = "fn one() -> u8 { 1 + 2 }\nfn two() -> u8 { 1 + 2 }\n";
        assert_eq!(
            fn_groups(Dejadoc::default(), src),
            Vec::<Vec<String>>::new()
        );
        assert_eq!(
            fn_groups(Dejadoc::default().fn_min_tokens(0), src),
            [["c::one", "c::two"]]
        );
        let src = format!("fn one{BODY}\nfn two{BODY}\n");
        assert_eq!(
            fn_groups(Dejadoc::default().fn_min_tokens(1000), &src),
            Vec::<Vec<String>>::new()
        );
    }

    #[test]
    fn an_allow_comment_above_a_function_drops_its_site() {
        let src = format!("fn one{BODY}\n// dejadoc: allow\n#[inline]\nfn two{BODY}\n");
        assert_eq!(
            fn_groups(Dejadoc::default(), &src),
            Vec::<Vec<String>>::new()
        );
        let src = format!("fn one{BODY}\n// dejadoc: allow\n\nfn two{BODY}\n");
        assert_eq!(fn_groups(Dejadoc::default(), &src), [["c::one", "c::two"]]);
    }

    #[test]
    fn nested_functions_are_not_sites() {
        let src = format!("fn outer() {{ fn one{BODY}\nfn two{BODY} }}\n");
        assert_eq!(
            fn_groups(Dejadoc::default(), &src),
            Vec::<Vec<String>>::new()
        );
    }

    #[test]
    fn a_function_site_spans_its_attributes_and_counts_apart() {
        let src = format!("/// One.\n#[inline]\nfn one{BODY}\n\nfn two{BODY}\n");
        let report = Dejadoc::default().functions().run_targets(
            "",
            &[target_from("c", "src/lib.rs", &[], &src)],
            &|_f, _i| None,
        );
        assert_eq!((report.total, report.unique), (0, 0));
        assert_eq!((report.functions, report.unique_functions), (2, 1));
        let sites = &report.groups[0].sites;
        assert_eq!((sites[0].line, sites[0].end), (1, Some(3)));
        assert_eq!((sites[1].line, sites[1].end), (5, Some(5)));
        assert_eq!(sites[0].code, format!("/// One.\n#[inline]\nfn one{BODY}"));
    }

    #[test]
    fn functions_in_a_file_rustdoc_skips_still_count() {
        let mut target = target_from(
            "c",
            "src/lib.rs",
            &[],
            &format!("/// ```\n/// let x = 1;\n/// ```\nfn one{BODY}\nfn two{BODY}\n"),
        );
        target.files[0].rustdoc = false;
        let report = Dejadoc::default()
            .functions()
            .run_targets("", &[target], &|_f, _i| None);
        assert_eq!(report.total, 0);
        assert_eq!(report.groups.len(), 1);
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
