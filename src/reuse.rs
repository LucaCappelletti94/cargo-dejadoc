//! Dependency context findings and search parameters.

use crate::ContextSite;
use alloc::string::String;
use alloc::vec::Vec;

/// Parameters for dependency context discovery.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(default, deny_unknown_fields, rename_all = "kebab-case")]
pub struct DependencySearch {
    /// Maximum shortest-path dependency distance, default zero.
    pub depth: Option<usize>,
    /// Include development dependency edges.
    pub include_dev: Option<bool>,
    /// Include build dependency edges.
    pub include_build: Option<bool>,
    /// Explore optional dependency paths outside the selected feature configuration.
    pub all_optional: Option<bool>,
    /// Cargo features enabled for discovery.
    pub features: Option<Vec<String>>,
    /// Enable all workspace features for discovery.
    pub all_features: Option<bool>,
    /// Disable default workspace features for discovery.
    pub no_default_features: Option<bool>,
    /// Target triple used for dependency discovery.
    pub target: Option<String>,
    /// Persistent index directory, defaulting to the user cache directory.
    pub cache_dir: Option<String>,
    /// Check the latest published release, default true.
    pub check_latest: Option<bool>,
    /// Make dependency findings fail the duplicate gate, default false.
    pub fail_on_findings: Option<bool>,
}

impl DependencySearch {
    #[cfg(feature = "std")]
    pub(crate) fn fill_from(mut self, defaults: &Self) -> Self {
        self.depth = self.depth.or(defaults.depth);
        self.include_dev = self.include_dev.or(defaults.include_dev);
        self.include_build = self.include_build.or(defaults.include_build);
        self.all_optional = self.all_optional.or(defaults.all_optional);
        self.features = self.features.or_else(|| defaults.features.clone());
        self.all_features = self.all_features.or(defaults.all_features);
        self.no_default_features = self.no_default_features.or(defaults.no_default_features);
        self.target = self.target.or_else(|| defaults.target.clone());
        self.cache_dir = self.cache_dir.or_else(|| defaults.cache_dir.clone());
        self.check_latest = self.check_latest.or(defaults.check_latest);
        self.fail_on_findings = self.fail_on_findings.or(defaults.fail_on_findings);
        self
    }
}

/// Source-established accessibility of a lexical context.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, serde::Serialize, serde::Deserialize,
)]
#[serde(rename_all = "kebab-case")]
pub enum ContextAccess {
    /// A whole function with an established public path.
    PublicFunction,
    /// A whole method with an established public receiver and item path.
    PublicMethod,
    /// A whole function whose declaration is internal.
    InternalFunction,
    /// A whole method whose declaration is internal.
    InternalMethod,
    /// An embedded lexical context requiring extraction.
    Embedded,
    /// Source analysis does not establish external accessibility.
    Unresolved,
}

/// The reuse path suggested by source evidence.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, serde::Serialize, serde::Deserialize,
)]
#[serde(rename_all = "kebab-case")]
pub enum ReuseAction {
    /// Review an accessible API in the resolved version.
    ReuseApi,
    /// Review an accessible API in a newer published release.
    UpgradeApi,
    /// Request extraction or exposure of an internal context.
    RequestExtraction,
    /// Establish accessibility before choosing a reuse path.
    Unresolved,
}

/// An existing tagged upstream extraction request.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ExistingIssue {
    /// Full GitHub issue URL.
    pub url: String,
    /// Current upstream issue state.
    pub state: String,
}

/// An upstream lexical context matching selected local code.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct DependencyCandidate {
    /// Dependency package name.
    pub package: String,
    /// Indexed package version.
    pub version: String,
    /// Resolved dependency source identity.
    pub source: String,
    /// Shortest dependency distance from selected local crates.
    pub depth: usize,
    /// Original package-relative source range.
    pub site: ContextSite,
    /// Source-established accessibility.
    pub access: ContextAccess,
    /// An established externally accessible function or method path.
    pub api_path: Option<String>,
    /// Conditional-compilation requirements retained from source.
    pub conditions: Vec<String>,
    /// Source-established extraction constraints and unresolved facts.
    pub notes: Vec<String>,
    /// Suggested reuse path requiring human compatibility review.
    pub action: ReuseAction,
    /// GitHub repository explicitly declared by the upstream package.
    pub repository: Option<String>,
    /// Verified commit-pinned upstream source range.
    pub source_url: Option<String>,
    /// Prefilled extraction request URL when no existing request was established.
    pub issue_url: Option<String>,
    /// Existing tagged request for the same source URL.
    pub existing_issue: Option<ExistingIssue>,
    /// Unresolved source-link or issue-lookup condition.
    pub lookup_warning: Option<String>,
}

/// Dependency reuse alternatives for one selected local context.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ReuseGroup {
    /// Canonical-form hash used to associate local and dependency contexts.
    pub hash: String,
    /// Weighted canonical token count.
    pub tokens: usize,
    /// Selected local source range.
    pub local: ContextSite,
    /// Upstream alternatives ordered with accessible APIs first.
    pub candidates: Vec<DependencyCandidate>,
}

/// Coverage and failures of a requested dependency search.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct DependencyCoverage {
    /// Whether every requested dependency analysis job completed.
    pub complete: bool,
    /// Distinct dependency source packages examined.
    pub packages: usize,
    /// Source files examined through the persistent index.
    pub indexed_files: usize,
    /// Source files loaded from a valid persistent index.
    pub cached_files: usize,
    /// Failures that leave requested analysis incomplete.
    pub failures: Vec<String>,
    /// Non-fatal provenance, lookup or cache diagnostics.
    pub warnings: Vec<String>,
}

impl Default for DependencyCoverage {
    fn default() -> Self {
        Self {
            complete: true,
            packages: 0,
            indexed_files: 0,
            cached_files: 0,
            failures: Vec::new(),
            warnings: Vec::new(),
        }
    }
}

/// Grouped dependency findings and requested-analysis coverage.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ReuseReport {
    /// Reuse opportunities grouped by selected local context.
    pub groups: Vec<ReuseGroup>,
    /// Completed jobs and missing analysis coverage.
    pub coverage: DependencyCoverage,
    /// Whether findings participate in the duplicate gate.
    #[serde(skip)]
    pub fail_on_findings: bool,
}

#[cfg(feature = "std")]
pub(crate) struct PackageSource {
    pub(crate) name: String,
    pub(crate) version: String,
    pub(crate) source: String,
    pub(crate) root: std::path::PathBuf,
    pub(crate) repository: Option<String>,
    pub(crate) source_hashes: alloc::collections::BTreeMap<String, String>,
}
