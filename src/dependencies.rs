//! Source-only dependency traversal and persistent indexing without consumer writes.

#[cfg(test)]
use crate::SourceFile;
use crate::TargetScan;
use crate::context_cache::{self as cache, Load, Store};
use crate::context_index::IndexedContext;
use crate::reuse::{
    ContextAccess, DependencyCandidate, DependencyCoverage, DependencySearch, PackageSource,
    ReuseAction, ReuseGroup, ReuseReport,
};
use crate::source_archive::{ArchiveKind, extract_archive};
use alloc::borrow::ToOwned;
use alloc::collections::{BTreeMap, BTreeSet, VecDeque};
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec;
use alloc::vec::Vec;
use cargo_metadata::cargo_platform::{Cfg, Platform};
use cargo_metadata::{Dependency, DependencyKind, Metadata, TargetKind};
use core::str::FromStr;
use semver::Version;
use std::eprintln;
use std::path::{Path, PathBuf};

/// The crates.io index source identity cargo reports.
const CRATES_IO_INDEX: &str = "https://github.com/rust-lang/crates.io-index";
const CRATES_IO_SOURCE: &str = "registry+https://github.com/rust-lang/crates.io-index";
/// Default registry API base for latest-release checks.
pub(crate) const REGISTRY_API: &str = "https://crates.io/api/v1/crates";
/// Default registry archive base for source downloads.
pub(crate) const REGISTRY_CDN: &str = "https://static.crates.io/crates";

/// Registry endpoints the latest-release check talks to.
#[derive(Clone, Debug)]
pub(crate) struct Registry {
    pub(crate) api: String,
    pub(crate) cdn: String,
}

impl Default for Registry {
    fn default() -> Self {
        Self {
            api: REGISTRY_API.to_owned(),
            cdn: REGISTRY_CDN.to_owned(),
        }
    }
}

/// The effective search parameters after option and configuration fallback.
#[derive(Clone, Debug)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "Cargo selection and reporting flags are independent"
)]
struct Effective {
    depth: usize,
    include_dev: bool,
    include_build: bool,
    all_optional: bool,
    features: Vec<String>,
    all_features: bool,
    no_default_features: bool,
    target: Option<String>,
    check_latest: bool,
    fail_on_findings: bool,
    scan_generated: bool,
    markers: Vec<String>,
}

fn effective(options: &DependencySearch) -> Effective {
    Effective {
        depth: options.depth.unwrap_or(0),
        include_dev: options.include_dev.unwrap_or(false),
        include_build: options.include_build.unwrap_or(false),
        all_optional: options.all_optional.unwrap_or(false),
        features: options.features.clone().unwrap_or_default(),
        all_features: options.all_features.unwrap_or(false),
        no_default_features: options.no_default_features.unwrap_or(false),
        target: options.target.clone(),
        check_latest: options.check_latest.unwrap_or(true),
        fail_on_findings: options.fail_on_findings.unwrap_or(false),
        scan_generated: false,
        markers: Vec::new(),
    }
}

fn index_fingerprint(eff: &Effective) -> String {
    let mut hash = blake3::Hasher::new();
    hash.update(cache::canonicalization_fingerprint().as_bytes());
    hash.update(&[u8::from(eff.scan_generated)]);
    for marker in &eff.markers {
        hash.update(&(marker.len() as u64).to_le_bytes());
        hash.update(marker.as_bytes());
    }
    hash.finalize().to_hex().to_string()
}

/// One package of the traversal graph.
#[derive(Clone, Debug)]
struct Package {
    id: String,
    name: String,
    version: String,
    /// Source identity exactly as cargo reports it, `None` for path sources.
    source: Option<String>,
    /// Directory holding the package manifest.
    manifest_dir: PathBuf,
    /// Manifest dependency edges, targeted tables included.
    deps: Vec<ManifestDep>,
    /// Package-relative library source file, when the package has one.
    lib_rel: Option<String>,
    /// Library target name, `[lib].name` when declared.
    lib_name: Option<String>,
    /// Repository field of the package manifest, when declared.
    repository: Option<String>,
    /// Whether the package exposes an ordinary Rust-linkable library.
    library: bool,
    /// Present in the resolved graph.
    resolved: bool,
}

/// One manifest dependency edge.
#[derive(Clone, Debug)]
struct ManifestDep {
    name: String,
    source: Option<String>,
    req: String,
    kind: DependencyKind,
    optional: bool,
    target: Option<Platform>,
    /// Manifest directory of a path dependency.
    path: Option<PathBuf>,
}

/// The configured dependency graph plus its manifest edges.
#[derive(Debug)]
struct Graph {
    packages: BTreeMap<String, Package>,
    /// Enabled edges from the resolved graph, `from` to `(kind, to)`.
    configured: BTreeMap<String, Vec<(DependencyKind, String)>>,
    src_to_member: BTreeMap<String, String>,
}

/// An on-disk package source and its cache entry.
struct Source {
    /// Directory the package source is rooted at.
    root: PathBuf,
    /// Whether the source can change under the same cache key.
    mutable: bool,
    /// Store subdirectory, `public/crates.io` or `private/<key>`.
    dir: String,
    /// Entry name, `name-version` for immutable entries, `name` otherwise.
    name: String,
    tree: Option<crate::discover::ModuleTree>,
}

/// A dependency context record awaiting grouping.
struct DepRec {
    name: String,
    version: String,
    source: String,
    depth: usize,
    root: PathBuf,
    repository: Option<String>,
    latest_only: bool,
    record: IndexedContext,
}

/// A physical upstream range merged across versions and module aliases.
struct Merged {
    name: String,
    version: String,
    source: String,
    depth: usize,
    root: PathBuf,
    repository: Option<String>,
    site: crate::ContextSite,
    access: ContextAccess,
    api_path: Option<String>,
    conditions: Vec<String>,
    notes: Vec<String>,
    has_resolved: bool,
}

/// Traversal and indexing state for one run.
struct Discovery {
    graph: Graph,
    eff: Effective,
    store: Option<Store>,
    store_root: PathBuf,
    registry: Registry,
    index_fingerprint: String,
    coverage: DependencyCoverage,
    /// The targets discovery resolves for, empty when none could be established.
    effective_targets: Vec<String>,
    /// rustc cfg predicates per target triple, lazily acquired.
    cfgs: BTreeMap<String, Vec<Cfg>>,
    /// Latest published version per package, when the check succeeded.
    latest_known: BTreeMap<String, String>,
}

impl Discovery {
    fn fail(&mut self, message: &str) {
        if !self.coverage.failures.iter().any(|f| f == message) {
            self.coverage.failures.push(message.to_owned());
        }
        self.coverage.complete = false;
    }

    fn warn(&mut self, message: &str) {
        if !self.coverage.warnings.iter().any(|w| w == message) {
            self.coverage.warnings.push(message.to_owned());
        }
    }

    fn kind_allowed(&self, kind: DependencyKind) -> bool {
        match kind {
            DependencyKind::Normal => true,
            DependencyKind::Development => self.eff.include_dev,
            DependencyKind::Build => self.eff.include_build,
            _ => false,
        }
    }

    fn cfgs_for(&mut self, target: &str) -> Option<&Vec<Cfg>> {
        if self.cfgs.contains_key(target) {
            return self.cfgs.get(target);
        }
        let id = next_proc_id();
        let args = vec![
            "--print".to_owned(),
            "cfg".to_owned(),
            "--target".to_owned(),
            target.to_owned(),
        ];
        let mut acquired = Vec::new();
        match run_bounded(
            std::process::Command::new("rustc").args(&args),
            FAST_PROCESS_DEADLINE,
            &id,
        ) {
            Ok(out) if out.status.success() => {
                let text = String::from_utf8_lossy(&out.stdout);
                acquired = text
                    .lines()
                    .filter_map(|line| Cfg::from_str(line).ok())
                    .collect();
            }
            Ok(_) | Err(_) => {
                self.fail(&format!(
                    "dependency discovery: platform conditions could not be evaluated for {target} (rustc cfg unavailable)"
                ));
            }
        }
        self.cfgs.insert(target.to_owned(), acquired);
        self.cfgs.get(target)
    }

    fn platform_matches(&mut self, dep: &ManifestDep) -> bool {
        let Some(platform) = &dep.target else {
            return true;
        };
        if self.effective_targets.is_empty() {
            return true;
        }
        let platform = platform.clone();
        self.effective_targets
            .clone()
            .iter()
            .any(|target| match &platform {
                Platform::Name(name) => name == target,
                Platform::Cfg(expr) => self.cfgs_for(target).is_some_and(|cfgs| expr.matches(cfgs)),
            })
    }
}

/// Group source-only reuse findings with requested-analysis coverage.
pub(crate) fn analyze(
    root: &Path,
    targets: &[TargetScan],
    local: &[&IndexedContext],
    options: &DependencySearch,
    floor: usize,
    scan_generated: bool,
    generated_markers: &[String],
) -> ReuseReport {
    let mut eff = effective(options);
    eff.scan_generated = scan_generated;
    eff.markers = generated_markers.to_vec();
    let fail_on_findings = eff.fail_on_findings;
    if eff.depth == 0 {
        return ReuseReport {
            groups: Vec::new(),
            coverage: DependencyCoverage::default(),
            fail_on_findings,
        };
    }
    let cache_dir = options
        .cache_dir
        .clone()
        .unwrap_or_else(|| cache::Store::default_root().to_string_lossy().into_owned());
    let store_root = PathBuf::from(&cache_dir);
    let store = match cache::Store::open(&store_root) {
        Ok(store) => Some(store),
        Err(err) => {
            eprintln!("dejadoc: dependency index is unavailable: {err}");
            None
        }
    };
    let mut d = Discovery {
        graph: Graph {
            packages: BTreeMap::new(),
            configured: BTreeMap::new(),
            src_to_member: BTreeMap::new(),
        },
        index_fingerprint: index_fingerprint(&eff),
        eff,
        store,
        store_root,
        registry: Registry::default(),
        coverage: DependencyCoverage::default(),
        effective_targets: Vec::new(),
        cfgs: BTreeMap::new(),
        latest_known: BTreeMap::new(),
    };
    d.effective_targets = establish_targets(root, d.eff.target.as_deref());
    if d.effective_targets.is_empty() {
        d.fail(
            "dependency discovery: could not establish the effective target, platform filtering skipped",
        );
    }
    let (metadata, targeted) = match run_metadata(root, &d.eff, &d.effective_targets) {
        Ok(value) => value,
        Err(err) => {
            d.fail(&format!("dependency discovery: {err}"));
            return ReuseReport {
                groups: Vec::new(),
                coverage: d.coverage,
                fail_on_findings,
            };
        }
    };
    d.graph = build_graph(&metadata, &targeted);
    analyze_discovery(&mut d, root, targets, local, floor)
}

/// `analyze` over a ready graph, for tests with a controlled workspace.
fn analyze_discovery(
    d: &mut Discovery,
    root: &Path,
    targets: &[TargetScan],
    local: &[&IndexedContext],
    floor: usize,
) -> ReuseReport {
    let fail_on_findings = d.eff.fail_on_findings;
    let roots = d.roots(root, local, targets);
    if roots.is_empty() {
        return ReuseReport {
            groups: Vec::new(),
            coverage: core::mem::take(&mut d.coverage),
            fail_on_findings,
        };
    }
    let reach = d.traverse(&roots);
    d.coverage.packages = reach.len();
    let mut by_form: BTreeMap<String, Vec<DepRec>> = BTreeMap::new();
    let mut source_hashes: BTreeMap<(String, String, String), BTreeMap<String, String>> =
        BTreeMap::new();
    let packages = core::mem::take(&mut d.graph.packages);
    for (id, depth) in &reach {
        let Some(pkg) = packages.get(id) else {
            continue;
        };
        let Some(mut src) = d.source_root(pkg) else {
            continue;
        };
        let Some(lib_rel) = pkg.lib_rel.as_deref() else {
            continue;
        };
        let (records, hashes) = d.index_package(pkg, lib_rel, &mut src);
        source_hashes.insert(
            (
                pkg.name.clone(),
                pkg.version.clone(),
                pkg.source.clone().unwrap_or_default(),
            ),
            hashes,
        );
        for record in records {
            by_form
                .entry(record.form.clone())
                .or_default()
                .push(DepRec {
                    name: pkg.name.clone(),
                    version: pkg.version.clone(),
                    source: pkg.source.clone().unwrap_or_default(),
                    depth: *depth,
                    root: src.root.clone(),
                    repository: pkg.repository.clone(),
                    latest_only: false,
                    record,
                });
        }
        if d.eff.check_latest && pkg.source.as_deref() == Some(CRATES_IO_SOURCE) {
            index_latest(d, pkg, *depth, &mut by_form, &mut source_hashes);
        }
    }
    d.graph.packages = packages;
    let (mut groups, side) = group(local, &by_form, root, floor);
    enrich_groups(d, &mut groups, &side, &mut source_hashes);
    ReuseReport {
        groups,
        coverage: core::mem::take(&mut d.coverage),
        fail_on_findings,
    }
}

type SourceHashes = BTreeMap<(String, String, String), BTreeMap<String, String>>;

fn index_latest(
    d: &mut Discovery,
    pkg: &Package,
    depth: usize,
    by_form: &mut BTreeMap<String, Vec<DepRec>>,
    source_hashes: &mut SourceHashes,
) {
    let Some(latest) = d.latest_version(pkg) else {
        return;
    };
    let Ok(resolved) = Version::parse(&pkg.version) else {
        d.warn(&format!(
            "newer-version availability unknown for {} (unparseable version {})",
            pkg.name, pkg.version
        ));
        return;
    };
    if latest <= resolved {
        return;
    }
    let version = latest.to_string();
    let Some(latest_root) = d.crates_io_source(&pkg.name, &version) else {
        return;
    };
    let manifest = latest_root.join("Cargo.toml");
    let Ok(text) = std::fs::read_to_string(&manifest) else {
        d.fail(&format!(
            "newer version {}-{} manifest is unreadable",
            pkg.name, version
        ));
        return;
    };
    let Ok(value) = toml::from_str::<toml::Value>(&text) else {
        d.fail(&format!(
            "newer version {}-{} manifest is invalid",
            pkg.name, version
        ));
        return;
    };
    let Some((lib_rel, lib_name, library)) = manifest_library_info(&value, &latest_root) else {
        d.warn(&format!(
            "newer version {}-{} exposes no library",
            pkg.name, version
        ));
        return;
    };
    let mut latest_pkg = pkg.clone();
    latest_pkg.version.clone_from(&version);
    latest_pkg.lib_name = lib_name;
    latest_pkg.library = library;
    let mut source = Source {
        root: latest_root,
        mutable: false,
        dir: "public/crates.io".to_owned(),
        name: format!("{}-{version}", pkg.name),
        tree: None,
    };
    let (records, hashes) = d.index_package(&latest_pkg, &lib_rel, &mut source);
    source_hashes.insert(
        (
            pkg.name.clone(),
            version.clone(),
            pkg.source.clone().unwrap_or_default(),
        ),
        hashes,
    );
    for record in records {
        by_form
            .entry(record.form.clone())
            .or_default()
            .push(DepRec {
                name: pkg.name.clone(),
                version: version.clone(),
                source: pkg.source.clone().unwrap_or_default(),
                depth,
                root: source.root.clone(),
                repository: pkg.repository.clone(),
                latest_only: true,
                record,
            });
    }
}

struct EnrichmentBatch {
    indices: Vec<(usize, usize)>,
    repository: Option<String>,
}

fn enrich_groups(
    d: &mut Discovery,
    groups: &mut [ReuseGroup],
    origins: &[Vec<CandidateOrigin>],
    hashes: &mut SourceHashes,
) {
    let mut upstream = crate::context_upstream::Upstream::new(&d.store_root);
    let mut batches: BTreeMap<(String, String, String, PathBuf), EnrichmentBatch> = BTreeMap::new();
    for (g, (group, rows)) in groups.iter().zip(origins).enumerate() {
        for (i, row) in rows.iter().enumerate() {
            let candidate = &group.candidates[i];
            let key = (
                candidate.package.clone(),
                candidate.version.clone(),
                candidate.source.clone(),
                row.root.clone(),
            );
            batches
                .entry(key)
                .or_insert_with(|| EnrichmentBatch {
                    indices: Vec::new(),
                    repository: row.repository.clone(),
                })
                .indices
                .push((g, i));
        }
    }
    for ((name, version, source, root), batch) in batches {
        let source_hashes = hashes
            .remove(&(name.clone(), version.clone(), source.clone()))
            .unwrap_or_default();
        let package = PackageSource {
            name,
            version,
            source,
            root,
            repository: batch.repository,
            source_hashes,
        };
        let mut candidates: Vec<&mut DependencyCandidate> = groups
            .iter_mut()
            .enumerate()
            .flat_map(|(g, group)| {
                let indices = &batch.indices;
                group
                    .candidates
                    .iter_mut()
                    .enumerate()
                    .filter_map(move |(i, candidate)| {
                        indices.binary_search(&(g, i)).ok().map(|_| candidate)
                    })
            })
            .collect();
        for condition in upstream.enrich(&package, &mut candidates) {
            d.warn(&condition);
        }
    }
}

impl Discovery {
    /// The crates that hold selected local contexts, keyed to graph packages.
    fn roots(
        &mut self,
        root: &Path,
        local: &[&IndexedContext],
        targets: &[TargetScan],
    ) -> BTreeSet<String> {
        let mut out = BTreeSet::new();
        for record in local {
            let key = canonical(&root.join(&record.crate_root))
                .to_string_lossy()
                .into_owned();
            let Some(id) = self.graph.src_to_member.get(&key) else {
                let target_name = targets
                    .iter()
                    .find(|target| {
                        target
                            .files
                            .iter()
                            .any(|file| canonical(Path::new(&file.path)).to_string_lossy() == key)
                    })
                    .map(|target| target.name.clone());
                self.fail(&format!(
                    "dependency discovery: selected crate {} could not be located in the workspace metadata{}",
                    record.crate_name,
                    target_name.map_or_else(String::new, |name| format!(" (target {name})"))
                ));
                continue;
            };
            out.insert(id.clone());
        }
        out
    }

    /// Shortest-path reach from `roots` over the configured edges, plus optional
    /// edges when requested.
    fn traverse(&mut self, roots: &BTreeSet<String>) -> BTreeMap<String, usize> {
        let mut depth: BTreeMap<String, usize> = roots.iter().map(|id| (id.clone(), 0)).collect();
        let mut queue: VecDeque<String> = VecDeque::new();
        queue.extend(roots.iter().cloned());
        while let Some(id) = queue.pop_front() {
            let d = depth.get(&id).copied().unwrap_or(0);
            if d >= self.eff.depth {
                continue;
            }
            for (kind, to) in self.outgoing(&id) {
                if !self.kind_allowed(kind) || depth.contains_key(&to) {
                    continue;
                }
                depth.insert(to.clone(), d + 1);
                queue.push_back(to);
            }
        }
        depth
    }

    /// Outgoing edges of a package, configured edges when resolved and manifest
    /// optional edges when exploration is requested.
    fn outgoing(&mut self, id: &str) -> Vec<(DependencyKind, String)> {
        let Some(pkg) = self.graph.packages.get(id).cloned() else {
            return Vec::new();
        };
        let mut out = Vec::new();
        if pkg.resolved {
            if let Some(edges) = self.graph.configured.get(&pkg.id) {
                out.extend(edges.iter().map(|(kind, to)| (*kind, to.clone())));
            }
        } else {
            for dep in &pkg.deps {
                if dep.optional || !self.platform_matches(dep) {
                    continue;
                }
                if let Some(to) = self.resolve_target(dep) {
                    out.push((dep.kind, to));
                }
            }
        }
        if self.eff.all_optional {
            for dep in &pkg.deps {
                if !dep.optional || !self.platform_matches(dep) {
                    continue;
                }
                if let Some(to) = self.resolve_target(dep) {
                    out.push((dep.kind, to));
                }
            }
        }
        out
    }

    /// The graph package a manifest edge points at, discovered on disk or from
    /// the registry when absent.
    fn resolve_target(&mut self, dep: &ManifestDep) -> Option<String> {
        let mut best: Option<(String, Version)> = None;
        for (id, pkg) in &self.graph.packages {
            if pkg.name != dep.name || pkg.source != dep.source {
                continue;
            }
            if !version_satisfies(&pkg.version, &dep.req) {
                continue;
            }
            let Ok(version) = Version::parse(&pkg.version) else {
                continue;
            };
            match &best {
                None => best = Some((id.clone(), version)),
                Some((_, held)) if version > *held => best = Some((id.clone(), version)),
                _ => {}
            }
        }
        if let Some((id, _)) = best {
            return Some(id);
        }
        if let Some(dir) = &dep.path {
            return self.discover_path(dep, dir);
        }
        if dep.source.is_none()
            || dep
                .source
                .as_deref()
                .is_some_and(|s| s.starts_with("registry+"))
        {
            if dep.source.is_none() || dep.source.as_deref() == Some(CRATES_IO_SOURCE) {
                return self.discover_registry(dep);
            }
            self.fail(&format!(
                "dependency discovery: source for {} is unavailable offline (registry {})",
                dep.name,
                dep.source.as_deref().unwrap_or_default()
            ));
        } else {
            self.fail(&format!(
                "dependency discovery: source for {} is unavailable offline (source {})",
                dep.name,
                dep.source.as_deref().unwrap_or_default()
            ));
        }
        None
    }

    /// A path dependency whose manifest is on disk.
    fn discover_path(&mut self, dep: &ManifestDep, dir: &Path) -> Option<String> {
        let dir = dir.canonicalize().unwrap_or_else(|_| dir.to_path_buf());
        let manifest = dir.join("Cargo.toml");
        let Ok(text) = std::fs::read_to_string(&manifest) else {
            self.fail(&format!(
                "dependency discovery: manifest for {} at {} is unreadable",
                dep.name,
                manifest.display()
            ));
            return None;
        };
        let Ok(value) = toml::from_str::<toml::Value>(&text) else {
            self.fail(&format!(
                "dependency discovery: manifest for {} at {} is invalid",
                dep.name,
                manifest.display()
            ));
            return None;
        };
        let Some(name) = value
            .get("package")
            .and_then(|package| package.get("name"))
            .and_then(toml::Value::as_str)
        else {
            self.fail(&format!(
                "dependency discovery: manifest for {} at {} has no package name",
                dep.name,
                manifest.display()
            ));
            return None;
        };
        if name != dep.name {
            self.fail(&format!(
                "dependency discovery: manifest for {} at {} declares {name}",
                dep.name,
                manifest.display()
            ));
            return None;
        }
        let version = value
            .get("package")
            .and_then(|package| package.get("version"))
            .and_then(toml::Value::as_str)
            .unwrap_or("0.0.0")
            .to_owned();
        let id = format!("path+file://{}#{version}", dir.display());
        if self.graph.packages.contains_key(&id) {
            return Some(id);
        }
        let (lib_rel, lib_name, library) = manifest_library_info(&value, &dir)?;
        let deps = manifest_deps_from_toml(&value, &dir);
        let repository = value
            .get("package")
            .and_then(|package| package.get("repository"))
            .and_then(toml::Value::as_str)
            .map(str::to_owned);
        self.graph.packages.insert(
            id.clone(),
            Package {
                id: id.clone(),
                name: name.to_owned(),
                version,
                source: None,
                manifest_dir: dir.clone(),
                deps,
                lib_rel: Some(lib_rel),
                lib_name,
                repository,
                library,
                resolved: false,
            },
        );
        Some(id)
    }

    /// A crates.io dependency not in the resolved graph, resolved to the newest
    /// stable version that matches the requirement.
    fn discover_registry(&mut self, dep: &ManifestDep) -> Option<String> {
        let url = format!("{}/{}", self.registry.api, dep.name);
        let value = match crate::context_network::json(&url) {
            Ok(value) => value,
            Err(err) => {
                self.fail(&format!(
                    "dependency discovery: version of {} could not be resolved: {err}",
                    dep.name
                ));
                return None;
            }
        };
        let Ok(req) = semver::VersionReq::parse(&dep.req) else {
            self.fail(&format!(
                "dependency discovery: requirement {} for {} is unparseable",
                dep.req, dep.name
            ));
            return None;
        };
        let Some(versions) = value.get("versions").and_then(serde_json::Value::as_array) else {
            self.fail(&format!(
                "dependency discovery: version of {} could not be resolved (no versions listed)",
                dep.name
            ));
            return None;
        };
        let Some(version) = versions
            .iter()
            .filter_map(|entry| entry.get("num").and_then(serde_json::Value::as_str))
            .filter_map(|num| semver::Version::parse(num).ok())
            .filter(|v| v.pre.is_empty() && req.matches(v))
            .max()
        else {
            self.fail(&format!(
                "dependency discovery: no stable version of {} matches {}",
                dep.name, dep.req
            ));
            return None;
        };
        let version = version.to_string();
        let root = self.crates_io_source(&dep.name, &version)?;
        let Ok(text) = std::fs::read_to_string(root.join("Cargo.toml")) else {
            self.fail(&format!(
                "dependency discovery: manifest for {} {version} in the downloaded source is unreadable",
                dep.name
            ));
            return None;
        };
        let Ok(value) = toml::from_str::<toml::Value>(&text) else {
            self.fail(&format!(
                "dependency discovery: manifest for {} {version} in the downloaded source is invalid",
                dep.name
            ));
            return None;
        };
        let name = value
            .get("package")
            .and_then(|package| package.get("name"))
            .and_then(toml::Value::as_str)
            .unwrap_or_default();
        if name != dep.name {
            self.fail(&format!(
                "dependency discovery: manifest for {} {version} in the downloaded source declares {name}",
                dep.name
            ));
            return None;
        }
        let id = format!("{CRATES_IO_SOURCE}#{name}/{version}");
        if self.graph.packages.contains_key(&id) {
            return Some(id);
        }
        let (lib_rel, lib_name, library) = manifest_library_info(&value, &root)?;
        let deps = manifest_deps_from_toml(&value, &root);
        let repository = value
            .get("package")
            .and_then(|package| package.get("repository"))
            .and_then(toml::Value::as_str)
            .map(str::to_owned);
        self.graph.packages.insert(
            id.clone(),
            Package {
                id: id.clone(),
                name: name.to_owned(),
                version,
                source: Some(CRATES_IO_SOURCE.to_owned()),
                manifest_dir: root.clone(),
                deps,
                lib_rel: Some(lib_rel),
                lib_name,
                repository,
                library,
                resolved: false,
            },
        );
        Some(id)
    }

    /// Resolve actual source bytes and establish their cache privacy.
    fn source_root(&mut self, pkg: &Package) -> Option<Source> {
        let source = pkg.source.as_deref().unwrap_or_default();
        if source.is_empty() {
            return Some(Source {
                root: pkg.manifest_dir.clone(),
                mutable: true,
                dir: format!(
                    "private/{}",
                    source_key("path", &pkg.manifest_dir.to_string_lossy())
                ),
                name: pkg.name.clone(),
                tree: None,
            });
        }
        if let Some(url) = source.strip_prefix("git+") {
            if !pkg.manifest_dir.is_dir() {
                self.fail(&format!("dependency source for {} is missing", pkg.name));
                return None;
            }
            return Some(Source {
                root: pkg.manifest_dir.clone(),
                mutable: true,
                dir: format!("private/{}", source_key("git", url)),
                name: pkg.name.clone(),
                tree: None,
            });
        }
        if let Some(index) = source.strip_prefix("registry+") {
            if index == CRATES_IO_INDEX {
                return self.crates_io_root(pkg);
            }
            if pkg.manifest_dir.is_dir() {
                return Some(private_registry_source(pkg));
            }
            self.fail(&format!(
                "dependency source for {} {} is unavailable at registry {index}",
                pkg.name, pkg.version
            ));
            return None;
        }
        if pkg.manifest_dir.is_dir() {
            return Some(Source {
                root: pkg.manifest_dir.clone(),
                mutable: true,
                dir: format!("private/{}", source_key("source", source)),
                name: pkg.name.clone(),
                tree: None,
            });
        }
        self.fail(&format!(
            "dependency source for {} {} is unavailable",
            pkg.name, pkg.version
        ));
        None
    }

    fn crates_io_root(&mut self, pkg: &Package) -> Option<Source> {
        let available = pkg.manifest_dir.is_dir();
        let mut actual = private_registry_source(pkg);
        if available && pkg.lib_rel.is_none() {
            return Some(actual);
        }
        let published = match self.acquire_crates_io_source(&pkg.name, &pkg.version) {
            Ok(root) => root,
            Err(error) if available => {
                self.warn(&format!(
                    "{} remains private because its published source is unavailable {error}",
                    pkg.name
                ));
                return Some(actual);
            }
            Err(error) => {
                self.fail(&error);
                return None;
            }
        };
        let mut tree = None;
        if available {
            let manifests_match = match (
                std::fs::read(actual.root.join("Cargo.toml")),
                std::fs::read(published.join("Cargo.toml")),
            ) {
                (Ok(a), Ok(b)) => a == b,
                _ => false,
            };
            if !manifests_match {
                return Some(actual);
            }
            let lib_rel = pkg.lib_rel.as_deref()?;
            let original_tree = package_tree(pkg, lib_rel, &actual.root, false);
            let published_tree = package_tree(pkg, lib_rel, &published, true);
            if !same_sources(&original_tree, &actual.root, &published_tree, &published) {
                actual.tree = Some(original_tree);
                return Some(actual);
            }
            tree = Some(published_tree);
        }
        Some(Source {
            root: published,
            mutable: false,
            dir: "public/crates.io".to_owned(),
            name: format!("{}-{}", pkg.name, pkg.version),
            tree,
        })
    }

    /// Acquire a published crates.io artifact or record missing source coverage.
    fn crates_io_source(&mut self, name: &str, version: &str) -> Option<PathBuf> {
        match self.acquire_crates_io_source(name, version) {
            Ok(root) => Some(root),
            Err(error) => {
                self.fail(&error);
                None
            }
        }
    }

    fn acquire_crates_io_source(&self, name: &str, version: &str) -> Result<PathBuf, String> {
        let name_version = format!("{name}-{version}");
        let base = self.store_root.join("public").join("crates.io");
        let archives = base.join("archives");
        std::fs::create_dir_all(&archives).map_err(|error| {
            format!("dependency source for {name_version} is unavailable {error}")
        })?;
        let archive = archives.join(format!("{name_version}.crate"));
        if !archive.exists() {
            let url = format!("{}/{name}/{name_version}.crate", self.registry.cdn);
            crate::context_network::download(&url, &archive)?;
        }
        let dest = base.join("sources").join(&name_version);
        if !dest.is_dir() {
            extract_archive(&archive, &dest, ArchiveKind::Crate).map_err(|error| {
                format!("dependency source for {name_version} could not be extracted {error}")
            })?;
        }
        dest.canonicalize().map_err(|error| {
            format!("dependency source for {name_version} cannot be resolved {error}")
        })
    }

    /// The latest published version of a crates.io package, when checkable.
    fn latest_version(&mut self, pkg: &Package) -> Option<Version> {
        let url = format!("{}/{}", self.registry.api, pkg.name);
        let value = match crate::context_network::json(&url) {
            Ok(value) => value,
            Err(err) => {
                self.fail(&format!(
                    "latest release of {} could not be checked: {err}",
                    pkg.name
                ));
                return None;
            }
        };
        let Some(latest) = value
            .get("crate")
            .and_then(|crate_| crate_.get("max_stable_version"))
            .and_then(serde_json::Value::as_str)
        else {
            self.warn(&format!(
                "latest release of {} is unknown (no max_stable_version)",
                pkg.name
            ));
            return None;
        };
        let Ok(version) = Version::parse(latest) else {
            self.warn(&format!(
                "latest release of {} is unknown (unparseable version {latest})",
                pkg.name
            ));
            return None;
        };
        self.latest_known
            .insert(pkg.name.clone(), latest.to_owned());
        Some(version)
    }

    /// Reuse or build a package index while recording incomplete source coverage.
    fn index_package(
        &mut self,
        pkg: &Package,
        lib_rel: &str,
        src: &mut Source,
    ) -> (Vec<IndexedContext>, BTreeMap<String, String>) {
        let tree = src.tree.take().unwrap_or_else(|| {
            package_tree(pkg, lib_rel, &src.root, src.dir == "public/crates.io")
        });
        for failure in &tree.failures {
            let module = failure.module.join("::");
            let where_ = if module.is_empty() {
                String::new()
            } else {
                format!("in {module} ")
            };
            self.fail(&format!(
                "dependency source for {}: {} {where_}({}).",
                pkg.name, failure.file, failure.message
            ));
        }
        let mut complete = tree.failures.is_empty();
        let file_count = tree.files.len();
        let mut fingerprint: BTreeMap<String, String> = tree
            .files
            .iter()
            .map(|file| {
                (
                    relative(&file.path, &src.root),
                    blake3::hash(file.text.as_bytes()).to_hex().to_string(),
                )
            })
            .collect();
        if src.mutable {
            match std::fs::read_to_string(src.root.join("Cargo.toml")) {
                Ok(text) => {
                    fingerprint.insert(
                        "Cargo.toml".to_owned(),
                        blake3::hash(text.as_bytes()).to_hex().to_string(),
                    );
                }
                Err(err) => {
                    complete = false;
                    self.fail(&format!(
                        "dependency source for {} has an unreadable manifest {err}",
                        pkg.name
                    ));
                }
            }
        }
        if let Some(store) = &self.store {
            match store.load(&src.dir, &src.name) {
                Load::Stored(stored)
                    if complete
                        && stored.schema == cache::SCHEMA
                        && stored.canonicalization == self.index_fingerprint
                        && (!src.mutable || stored.files == fingerprint) =>
                {
                    self.coverage.cached_files += file_count;
                    self.coverage.indexed_files += file_count;
                    return (stored.records, stored.files);
                }
                Load::Stored(_) | Load::Missing => {}
                Load::Corrupt => self.warn(&format!(
                    "dependency index for {} is corrupt, rebuilt",
                    src.name
                )),
                Load::Io(err) => self.warn(&format!(
                    "dependency index for {} is unavailable: {err}",
                    src.name
                )),
            }
        }
        let scan = TargetScan {
            name: pkg.lib_name.clone().unwrap_or_else(|| pkg.name.clone()),
            files: tree.files,
            library: pkg.library,
        };
        let root = src.root.to_string_lossy().into_owned();
        let records =
            crate::context_index::extract(&scan, &root, self.eff.scan_generated, &self.eff.markers);
        self.coverage.indexed_files += file_count;
        if complete
            && let Some(store) = &self.store
            && let Err(err) = store.save(
                &src.dir,
                &src.name,
                &self.index_fingerprint,
                &fingerprint,
                &records,
            )
        {
            self.warn(&format!(
                "dependency index for {} could not be saved: {err}",
                src.name
            ));
        }
        (records, fingerprint)
    }
}

fn private_registry_source(pkg: &Package) -> Source {
    let root = pkg
        .manifest_dir
        .canonicalize()
        .unwrap_or_else(|_| pkg.manifest_dir.clone());
    Source {
        dir: format!(
            "private/{}",
            source_key("registry", &root.to_string_lossy())
        ),
        root,
        mutable: true,
        name: format!("{}-{}", pkg.name, pkg.version),
        tree: None,
    }
}

fn package_tree(
    pkg: &Package,
    lib_rel: &str,
    root: &Path,
    public: bool,
) -> crate::discover::ModuleTree {
    let target = crate::discover::Target {
        name: pkg.lib_name.clone().unwrap_or_else(|| pkg.name.clone()),
        package: pkg.name.clone(),
        src: root.join(lib_rel),
        doc: true,
        library: pkg.library,
    };
    crate::discover::module_tree(&target, public.then_some(root))
}

fn same_sources(
    actual: &crate::discover::ModuleTree,
    actual_root: &Path,
    published: &crate::discover::ModuleTree,
    published_root: &Path,
) -> bool {
    if !actual.failures.is_empty()
        || !published.failures.is_empty()
        || actual.files.len() != published.files.len()
    {
        return false;
    }
    let files: BTreeMap<_, _> = published
        .files
        .iter()
        .filter_map(|file| {
            Some((
                Path::new(&file.path).strip_prefix(published_root).ok()?,
                file.text.as_str(),
            ))
        })
        .collect();
    actual.files.iter().all(|file| {
        Path::new(&file.path)
            .strip_prefix(actual_root)
            .ok()
            .and_then(|relative| files.get(&relative))
            .is_some_and(|text| *text == file.text.as_str())
    })
}

/// The effective targets for discovery, the explicit option first, then every Cargo
/// `build.target`, then the rustc host.
fn establish_targets(root: &Path, explicit: Option<&str>) -> Vec<String> {
    if let Some(target) = explicit {
        return vec![target.to_owned()];
    }
    let configured = cargo_config2::Config::load_with_cwd(root)
        .and_then(|config| config.build_target_for_cli(core::iter::empty::<&str>()))
        .unwrap_or_default();
    if configured.is_empty() {
        host_target().into_iter().collect()
    } else {
        configured
    }
}

/// The rustc host triple, the fallback effective target.
fn host_target() -> Option<String> {
    let id = next_proc_id();
    let args = vec!["-vV".to_owned()];
    let Ok(out) = run_bounded(
        std::process::Command::new("rustc").args(&args),
        FAST_PROCESS_DEADLINE,
        &id,
    ) else {
        return None;
    };
    let text = String::from_utf8_lossy(&out.stdout);
    text.lines()
        .find_map(|line| line.strip_prefix("host: "))
        .map(str::to_owned)
}

/// Run `cargo metadata` for the configured feature and target graph, in a
/// temporary manifest mirror, so the consumer's lockfile is never written.
fn run_metadata(
    root: &Path,
    eff: &Effective,
    targets: &[String],
) -> Result<(Metadata, BTreeMap<String, Vec<ManifestDep>>), String> {
    let members = discover_members(root)?;
    let workspace = crate::context_manifest::ManifestWorkspace::create(root, &members)
        .map_err(|err| format!("prepare dependency mirror: {err}"))?;
    let mut args: Vec<String> = vec![
        "metadata".to_owned(),
        "--format-version".to_owned(),
        "1".to_owned(),
        "--manifest-path".to_owned(),
        workspace.manifest_path().to_string_lossy().into_owned(),
    ];
    if eff.no_default_features {
        args.push("--no-default-features".to_owned());
    }
    if eff.all_features {
        args.push("--all-features".to_owned());
    }
    if !eff.features.is_empty() {
        args.push("--features".to_owned());
        args.push(eff.features.join(","));
    }
    for target in targets {
        args.push("--filter-platform".to_owned());
        args.push(target.clone());
    }
    // The working directory stays at the original root, so the original cargo
    // config and rustup toolchain are honored.
    let id = next_proc_id();
    let out = run_bounded(
        std::process::Command::new("cargo")
            .args(&args)
            .current_dir(root),
        METADATA_PROCESS_DEADLINE,
        &id,
    )
    .map_err(|error| error.to_string())?;
    if !out.status.success() {
        return Err(format!(
            "cargo metadata failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    let value = serde_json::from_slice::<serde_json::Value>(&out.stdout)
        .map_err(|err| format!("parse cargo metadata: {err}"))?;
    let (temp_root, original_root) = workspace.remap();
    let value = crate::context_manifest::remap_value(value, &temp_root, &original_root);
    let mut targeted: BTreeMap<String, Vec<ManifestDep>> = BTreeMap::new();
    if let Some(packages) = value.get("packages").and_then(serde_json::Value::as_array) {
        for package in packages {
            let Some(id) = package.get("id").and_then(serde_json::Value::as_str) else {
                continue;
            };
            if let Some(deps) = package.get("targeted_dependencies") {
                let Ok(deps) = serde_json::from_value::<Vec<Dependency>>(deps.clone()) else {
                    continue;
                };
                targeted.insert(id.to_owned(), deps.iter().map(manifest_dep).collect());
            }
        }
    }
    let metadata: Metadata =
        serde_json::from_value(value).map_err(|err| format!("parse cargo metadata: {err}"))?;
    Ok((metadata, targeted))
}

/// The actual workspace members and their targets, from a bounded `--no-deps`
/// metadata run, which does not write the lockfile.
fn discover_members(root: &Path) -> Result<Vec<crate::context_manifest::MemberInfo>, String> {
    let manifest = root.join("Cargo.toml");
    let args = vec![
        "metadata".to_owned(),
        "--no-deps".to_owned(),
        "--format-version".to_owned(),
        "1".to_owned(),
        "--manifest-path".to_owned(),
        manifest.to_string_lossy().into_owned(),
    ];
    let id = next_proc_id();
    let out = run_bounded(
        std::process::Command::new("cargo")
            .args(&args)
            .current_dir(root),
        FAST_PROCESS_DEADLINE,
        &id,
    )
    .map_err(|error| error.to_string())?;
    if !out.status.success() {
        return Err(format!(
            "cargo metadata --no-deps failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    let value = serde_json::from_slice::<serde_json::Value>(&out.stdout)
        .map_err(|err| format!("parse no-deps metadata: {err}"))?;
    let Some(packages) = value.get("packages").and_then(serde_json::Value::as_array) else {
        return Err("no-deps metadata lists no packages".to_owned());
    };
    let mut members = Vec::new();
    for package in packages {
        let Some(manifest) = package
            .get("manifest_path")
            .and_then(serde_json::Value::as_str)
        else {
            continue;
        };
        let targets = package
            .get("targets")
            .and_then(serde_json::Value::as_array)
            .map_or_else(Vec::new, |targets| {
                targets
                    .iter()
                    .filter_map(|target| {
                        Some(crate::context_manifest::TargetInfo {
                            name: target.get("name")?.as_str()?.to_owned(),
                            kind: string_array(target, "kind"),
                            crate_types: string_array(target, "crate_types"),
                            src_path: PathBuf::from(target.get("src_path")?.as_str()?),
                            edition: target
                                .get("edition")
                                .and_then(serde_json::Value::as_str)
                                .map(str::to_owned),
                            required_features: string_array(target, "required-features"),
                        })
                    })
                    .collect()
            });
        members.push(crate::context_manifest::MemberInfo {
            manifest: PathBuf::from(manifest),
            targets,
        });
    }
    Ok(members)
}

/// A string array field of a metadata object.
fn string_array(value: &serde_json::Value, field: &str) -> Vec<String> {
    value
        .get(field)
        .and_then(serde_json::Value::as_array)
        .map_or_else(Vec::new, |items| {
            items
                .iter()
                .filter_map(|item| item.as_str().map(str::to_owned))
                .collect()
        })
}

/// Deadline for fast cargo and rustc lookups.
const FAST_PROCESS_DEADLINE: std::time::Duration = std::time::Duration::from_secs(30);
/// Deadline for cargo metadata, which may fetch the registry index.
const METADATA_PROCESS_DEADLINE: std::time::Duration = std::time::Duration::from_secs(300);

static PROC_SEQ: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// A correlated identifier for one subprocess request.
pub(crate) fn next_proc_id() -> String {
    let seq = PROC_SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    format!("djd.proc.{}-{seq:04}", std::process::id())
}

/// Deadline for reaping a killed subprocess.
const REAP_DEADLINE: std::time::Duration = std::time::Duration::from_secs(10);
/// The maximum captured subprocess output before a capture is rejected.
const OUTPUT_LIMIT: u64 = 16 * 1024 * 1024;

/// The captured output of one bounded subprocess.
#[derive(Debug)]
pub(crate) struct ProcOutput {
    pub(crate) status: std::process::ExitStatus,
    pub(crate) stdout: Vec<u8>,
    pub(crate) stderr: Vec<u8>,
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum ProcessError {
    #[error("{id} {operation} {program} {source}")]
    Io {
        id: String,
        program: String,
        operation: &'static str,
        #[source]
        source: std::io::Error,
    },
    #[error("{id} {program} breached its {deadline:?} deadline")]
    Deadline {
        id: String,
        program: String,
        deadline: std::time::Duration,
    },
    #[error("{id} {program} output exceeds the {OUTPUT_LIMIT} byte limit")]
    OutputLimit { id: String, program: String },
}

impl ProcessError {
    fn io(id: &str, program: &str, operation: &'static str, source: std::io::Error) -> Self {
        Self::Io {
            id: id.to_owned(),
            program: program.to_owned(),
            operation,
            source,
        }
    }
}

struct Process(std::process::Child);

impl Drop for Process {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = reap_bounded(&mut self.0, REAP_DEADLINE);
    }
}

/// Run a correlated subprocess with bounded completion, capture and cleanup.
pub(crate) fn run_bounded(
    command: &mut std::process::Command,
    deadline: std::time::Duration,
    id: &str,
) -> Result<ProcOutput, ProcessError> {
    let program = command.get_program().to_string_lossy().into_owned();
    let name = program.as_str();
    let io = |operation: &'static str| move |error| ProcessError::io(id, name, operation, error);
    let dir = tempfile::tempdir().map_err(io("tempdir"))?;
    let stdout_path = dir.path().join("stdout");
    let stderr_path = dir.path().join("stderr");
    let stdout = std::fs::File::create(&stdout_path).map_err(io("stdout file"))?;
    let stderr = std::fs::File::create(&stderr_path).map_err(io("stderr file"))?;
    command.stdout(stdout).stderr(stderr);
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt as _;
        command.process_group(0);
    }
    let mut child = Process(command.spawn().map_err(io("spawn"))?);
    let started = std::time::Instant::now();
    loop {
        let outcome = match child.0.try_wait().map_err(io("wait")) {
            Ok(Some(status)) => {
                let stdout = capture_output(&stdout_path, &program, id)?;
                let stderr = capture_output(&stderr_path, &program, id)?;
                return Ok(ProcOutput {
                    status,
                    stdout,
                    stderr,
                });
            }
            Ok(None) if started.elapsed() < deadline => None,
            Ok(None) => Some(ProcessError::Deadline {
                id: id.to_owned(),
                program: program.clone(),
                deadline,
            }),
            Err(error) => Some(error),
        };
        if let Some(error) = outcome {
            let group = child.0.id();
            let _ = child.0.kill();
            kill_process_group(group);
            let _ = reap_bounded(&mut child.0, REAP_DEADLINE);
            return Err(error);
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
}

/// Read one captured output file, refusing one longer than the limit.
fn capture_output(path: &Path, program: &str, id: &str) -> Result<Vec<u8>, ProcessError> {
    use std::io::Read as _;
    let mut bytes = Vec::new();
    std::fs::File::open(path)
        .and_then(|file| file.take(OUTPUT_LIMIT + 1).read_to_end(&mut bytes))
        .map_err(|error| ProcessError::io(id, program, "read output", error))?;
    if !u64::try_from(bytes.len()).is_ok_and(|length| length <= OUTPUT_LIMIT) {
        return Err(ProcessError::OutputLimit {
            id: id.to_owned(),
            program: program.to_owned(),
        });
    }
    Ok(bytes)
}

/// Terminate the process group of a killed child, best effort.
#[cfg(unix)]
fn kill_process_group(pid: u32) {
    if let Some(group) = signalable_group(pid) {
        let _ = rustix::process::kill_process_group(group, rustix::process::Signal::KILL);
    }
}

/// The group of a spawned child, never 0 or 1, whose negation would signal the caller's own
/// group or every process of the user.
#[cfg(unix)]
fn signalable_group(pid: u32) -> Option<rustix::process::Pid> {
    let raw = i32::try_from(pid).ok().filter(|raw| *raw > 1)?;
    rustix::process::Pid::from_raw(raw)
}

/// No process-group termination without unix.
#[cfg(not(unix))]
fn kill_process_group(_pid: u32) {}

/// Poll child completion within a monotonic cleanup deadline.
fn reap_bounded(child: &mut std::process::Child, deadline: std::time::Duration) -> bool {
    let started = std::time::Instant::now();
    while started.elapsed() < deadline {
        match child.try_wait() {
            Ok(Some(_)) => return true,
            Ok(None) => std::thread::sleep(std::time::Duration::from_millis(20)),
            Err(_) => return false,
        }
    }
    false
}

/// Build the traversal graph from resolved metadata.
fn build_graph(metadata: &Metadata, targeted: &BTreeMap<String, Vec<ManifestDep>>) -> Graph {
    let mut packages = BTreeMap::new();
    let mut src_to_member = BTreeMap::new();
    for pkg in &metadata.packages {
        let id = pkg.id.to_string();
        let manifest_dir = pkg
            .manifest_path
            .parent()
            .map_or_else(PathBuf::new, |dir| dir.as_std_path().to_path_buf());
        let linkable = pkg.targets.iter().find(|target| {
            target
                .kind
                .iter()
                .any(|kind| matches!(kind, TargetKind::Lib | TargetKind::RLib | TargetKind::DyLib))
        });
        let lib_target = linkable.or_else(|| {
            pkg.targets.iter().find(|target| {
                target.kind.iter().any(|kind| {
                    matches!(
                        kind,
                        TargetKind::ProcMacro | TargetKind::CDyLib | TargetKind::StaticLib
                    )
                })
            })
        });
        let lib = lib_target.map(|target| {
            let abs = target.src_path.as_std_path().to_path_buf();
            let rel = abs.strip_prefix(&manifest_dir).map_or_else(
                |_| "src/lib.rs".to_owned(),
                |rel| rel.to_string_lossy().into_owned(),
            );
            (rel, target.name.clone())
        });
        let mut deps: Vec<ManifestDep> = pkg.dependencies.iter().map(manifest_dep).collect();
        deps.extend(targeted.get(&id).cloned().unwrap_or_default());
        for target in &pkg.targets {
            src_to_member.insert(
                canonical(target.src_path.as_std_path())
                    .to_string_lossy()
                    .into_owned(),
                id.clone(),
            );
        }
        packages.insert(
            id.clone(),
            Package {
                id,
                name: pkg.name.to_string(),
                version: pkg.version.to_string(),
                source: pkg.source.as_ref().map(|source| source.repr.clone()),
                manifest_dir,
                deps,
                lib_rel: lib.as_ref().map(|(rel, _)| rel.clone()),
                lib_name: lib.as_ref().map(|(_, name)| name.clone()),
                repository: pkg.repository.clone(),
                library: linkable.is_some(),
                resolved: false,
            },
        );
    }
    let mut resolved = BTreeSet::new();
    let mut configured: BTreeMap<String, Vec<(DependencyKind, String)>> = BTreeMap::new();
    if let Some(resolve) = &metadata.resolve {
        for node in &resolve.nodes {
            resolved.insert(node.id.to_string());
            for dep in &node.deps {
                for kind in &dep.dep_kinds {
                    configured
                        .entry(node.id.to_string())
                        .or_default()
                        .push((kind.kind, dep.pkg.to_string()));
                }
            }
        }
    }
    for (id, pkg) in &mut packages {
        pkg.resolved = resolved.contains(id);
    }
    Graph {
        packages,
        configured,
        src_to_member,
    }
}

/// A manifest dependency edge as stored on a package.
fn manifest_dep(dep: &Dependency) -> ManifestDep {
    ManifestDep {
        name: dep.name.clone(),
        source: dep.source.as_ref().map(|source| source.repr.clone()),
        req: dep.req.to_string(),
        kind: dep.kind,
        optional: dep.optional,
        target: dep.target.clone(),
        path: dep
            .path
            .as_ref()
            .map(|path| path.as_std_path().to_path_buf()),
    }
}

/// Library target facts of a raw manifest, `None` when the package has no
/// library source file.
fn manifest_library_info(
    value: &toml::Value,
    base: &Path,
) -> Option<(String, Option<String>, bool)> {
    let lib = value.get("lib");
    let path = lib
        .and_then(|lib| lib.get("path"))
        .and_then(toml::Value::as_str)
        .unwrap_or("src/lib.rs");
    if !base.join(path).is_file() {
        return None;
    }
    let name = lib
        .and_then(|lib| lib.get("name"))
        .and_then(toml::Value::as_str)
        .map(str::to_owned);
    let mut library = true;
    if let Some(lib) = lib {
        if lib
            .get("proc-macro")
            .and_then(toml::Value::as_bool)
            .unwrap_or(false)
        {
            library = false;
        } else if let Some(crate_type) = lib.get("crate-type").and_then(toml::Value::as_array) {
            library = crate_type
                .iter()
                .any(|t| matches!(t.as_str(), Some("lib" | "rlib" | "dylib")));
        }
    }
    Some((path.to_owned(), name, library))
}

/// Manifest dependency edges of a package discovered from a raw manifest.
fn manifest_deps_from_toml(value: &toml::Value, base: &Path) -> Vec<ManifestDep> {
    let mut out = Vec::new();
    for (table, kind) in [
        ("dependencies", DependencyKind::Normal),
        ("dev-dependencies", DependencyKind::Development),
        ("build-dependencies", DependencyKind::Build),
    ] {
        if let Some(deps) = value.get(table).and_then(toml::Value::as_table) {
            out.extend(dep_entries(deps, kind, None, base));
        }
    }
    if let Some(targets) = value.get("target").and_then(toml::Value::as_table) {
        for (key, table) in targets {
            let platform = Platform::from_str(key).ok();
            if let Some(table) = table.as_table() {
                for (sub, kind) in [
                    ("dependencies", DependencyKind::Normal),
                    ("dev-dependencies", DependencyKind::Development),
                    ("build-dependencies", DependencyKind::Build),
                ] {
                    if let Some(deps) = table.get(sub).and_then(toml::Value::as_table) {
                        out.extend(dep_entries(deps, kind, platform.as_ref(), base));
                    }
                }
            }
        }
    }
    out
}

/// One dependency table of a raw manifest.
fn dep_entries(
    table: &toml::Table,
    kind: DependencyKind,
    platform: Option<&Platform>,
    base: &Path,
) -> Vec<ManifestDep> {
    table
        .iter()
        .filter_map(|(name, value)| {
            let (source, req, optional, path) = if let Some(req) = value.as_str() {
                (None, req.to_owned(), false, None)
            } else {
                let table = value.as_table()?;
                let req = table
                    .get("version")
                    .and_then(toml::Value::as_str)
                    .unwrap_or("*")
                    .to_owned();
                let source = table
                    .get("git")
                    .and_then(toml::Value::as_str)
                    .map(|url| format!("git+{url}"))
                    .or_else(|| {
                        table
                            .get("registry")
                            .and_then(toml::Value::as_str)
                            .map(|url| format!("registry+{url}"))
                    });
                let optional = table
                    .get("optional")
                    .and_then(toml::Value::as_bool)
                    .unwrap_or(false);
                let path = table.get("path").and_then(toml::Value::as_str).map(|path| {
                    let path = Path::new(path);
                    if path.is_absolute() {
                        path.to_path_buf()
                    } else {
                        base.join(path)
                    }
                });
                (source, req, optional, path)
            };
            Some(ManifestDep {
                name: name.clone(),
                source,
                req,
                kind,
                optional,
                target: platform.cloned(),
                path,
            })
        })
        .collect()
}

/// Whether `version` satisfies the requirement `req`.
fn version_satisfies(version: &str, req: &str) -> bool {
    match (
        semver::Version::parse(version),
        semver::VersionReq::parse(req),
    ) {
        (Ok(version), Ok(req)) => req.matches(&version),
        _ => true,
    }
}

fn reuse_action(access: ContextAccess, resolved: bool) -> ReuseAction {
    match access {
        ContextAccess::PublicFunction | ContextAccess::PublicMethod if resolved => {
            ReuseAction::ReuseApi
        }
        ContextAccess::PublicFunction | ContextAccess::PublicMethod => ReuseAction::UpgradeApi,
        ContextAccess::InternalFunction
        | ContextAccess::InternalMethod
        | ContextAccess::Embedded => ReuseAction::RequestExtraction,
        ContextAccess::Unresolved => ReuseAction::Unresolved,
    }
}

struct CandidateOrigin {
    root: PathBuf,
    repository: Option<String>,
}

/// Group selected local contexts against dependency contexts.
fn group(
    local: &[&IndexedContext],
    by_form: &BTreeMap<String, Vec<DepRec>>,
    root: &Path,
    floor: usize,
) -> (Vec<ReuseGroup>, Vec<Vec<CandidateOrigin>>) {
    let mut anchors: Vec<&IndexedContext> = local
        .iter()
        .copied()
        .filter(|record| record.tokens >= floor)
        .collect();
    anchors.sort_by(|a, b| {
        record_priority(a.access, a.api_path.as_deref())
            .cmp(&record_priority(b.access, b.api_path.as_deref()))
            .then_with(|| a.site.file.cmp(&b.site.file))
            .then_with(|| a.site.line.cmp(&b.site.line))
            .then_with(|| a.site.column.cmp(&b.site.column))
    });
    let mut seen: BTreeSet<(String, u32, usize)> = BTreeSet::new();
    let mut groups = Vec::new();
    let mut side = Vec::new();
    for anchor in &anchors {
        let key = (
            anchor.site.file.clone(),
            anchor.site.line,
            anchor.site.column,
        );
        if !seen.insert(key) {
            continue;
        }
        let Some(recs) = by_form.get(&anchor.form) else {
            continue;
        };
        let merged = merge_records(recs);
        let mut candidates = Vec::new();
        for m in merged.into_values() {
            if m.name == anchor.crate_name {
                continue;
            }
            if same_site(
                &m.root,
                &m.site.file,
                m.site.line,
                root,
                &anchor.site.file,
                anchor.site.line,
            ) {
                continue;
            }
            let action = reuse_action(m.access, m.has_resolved);
            candidates.push((
                DependencyCandidate {
                    package: m.name,
                    version: m.version,
                    source: m.source,
                    depth: m.depth,
                    site: m.site,
                    access: m.access,
                    api_path: m.api_path,
                    conditions: m.conditions,
                    notes: m.notes,
                    action,
                    repository: None,
                    source_url: None,
                    issue_url: None,
                    existing_issue: None,
                    lookup_warning: None,
                },
                CandidateOrigin {
                    root: m.root,
                    repository: m.repository,
                },
            ));
        }
        if candidates.is_empty() {
            continue;
        }
        candidates.sort_by(|a, b| candidate_order(&a.0).cmp(&candidate_order(&b.0)));
        let (candidates, rows) = candidates.into_iter().unzip();
        side.push(rows);
        groups.push(ReuseGroup {
            hash: blake3::hash(anchor.form.as_bytes()).to_hex().to_string(),
            tokens: anchor.tokens,
            local: anchor.site.clone(),
            candidates,
        });
    }
    (groups, side)
}

#[derive(PartialEq, Eq, PartialOrd, Ord)]
struct MergeKey {
    package: String,
    source: String,
    path_root: Option<PathBuf>,
    file: String,
    line: u32,
    column: usize,
}

fn merge_records(recs: &[DepRec]) -> BTreeMap<MergeKey, Merged> {
    let mut merged: BTreeMap<MergeKey, Merged> = BTreeMap::new();
    for rec in recs {
        let record = &rec.record;
        let key = MergeKey {
            package: rec.name.clone(),
            source: rec.source.clone(),
            path_root: rec.source.is_empty().then(|| rec.root.clone()),
            file: record.site.file.clone(),
            line: record.site.line,
            column: record.site.column,
        };
        let priority = record_priority(record.access, record.api_path.as_deref());
        let resolved_api = !rec.latest_only
            && matches!(
                record.access,
                ContextAccess::PublicFunction | ContextAccess::PublicMethod
            );
        match merged.get_mut(&key) {
            None => {
                merged.insert(
                    key,
                    Merged {
                        name: rec.name.clone(),
                        version: rec.version.clone(),
                        source: rec.source.clone(),
                        depth: rec.depth,
                        root: rec.root.clone(),
                        repository: rec.repository.clone(),
                        site: record.site.clone(),
                        access: record.access,
                        api_path: record.api_path.clone(),
                        conditions: record.conditions.clone(),
                        notes: record.extraction_notes.clone(),
                        has_resolved: resolved_api,
                    },
                );
            }
            Some(m) if merged_priority(m) > priority => {
                m.version.clone_from(&rec.version);
                m.source.clone_from(&rec.source);
                m.depth = rec.depth;
                m.root.clone_from(&rec.root);
                m.site.clone_from(&record.site);
                m.access = record.access;
                m.api_path.clone_from(&record.api_path);
                m.conditions.clone_from(&record.conditions);
                m.notes.clone_from(&record.extraction_notes);
                m.has_resolved |= resolved_api;
            }
            Some(m) => m.has_resolved |= resolved_api,
        }
    }
    merged
}

/// Sort order of an accessibility classification, established routes first.
fn record_priority(access: ContextAccess, api_path: Option<&str>) -> (u8, bool) {
    (access_rank(access), api_path.is_none())
}

/// Whether an established accessibility wins the merge of one range.
fn merged_priority(m: &Merged) -> (u8, bool) {
    (access_rank(m.access), m.api_path.is_none())
}

/// The rank of an accessibility classification for display and merging.
fn access_rank(access: ContextAccess) -> u8 {
    match access {
        ContextAccess::PublicFunction | ContextAccess::PublicMethod => 0,
        ContextAccess::InternalFunction | ContextAccess::InternalMethod => 1,
        ContextAccess::Embedded => 2,
        ContextAccess::Unresolved => 3,
    }
}

/// The report order of candidates in one group, preferred action first.
fn candidate_order(candidate: &DependencyCandidate) -> (u8, usize, &str, &str, &str, u32) {
    (
        action_rank(candidate.action),
        candidate.depth,
        &candidate.package,
        &candidate.version,
        &candidate.site.file,
        candidate.site.line,
    )
}

/// The display rank of a reuse path, existing APIs first.
fn action_rank(action: ReuseAction) -> u8 {
    match action {
        ReuseAction::ReuseApi => 0,
        ReuseAction::UpgradeApi => 1,
        ReuseAction::RequestExtraction => 2,
        ReuseAction::Unresolved => 3,
    }
}

/// A path canonicalized when it resolves, unchanged otherwise.
fn canonical(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

/// Whether two package-relative sites are the same physical range.
fn same_site(
    root_a: &Path,
    file_a: &str,
    line_a: u32,
    root_b: &Path,
    file_b: &str,
    line_b: u32,
) -> bool {
    let path_a = std::fs::canonicalize(root_a.join(file_a)).unwrap_or_else(|_| root_a.join(file_a));
    let path_b = std::fs::canonicalize(root_b.join(file_b)).unwrap_or_else(|_| root_b.join(file_b));
    path_a == path_b && line_a == line_b
}

/// A stable cache key fragment for a mutable source identity.
fn source_key(kind: &str, value: &str) -> String {
    let digest = blake3::hash(value.as_bytes()).to_hex();
    format!("{kind}-{}", &digest[..16])
}

/// A package-relative site path against `root`.
fn relative(path: &str, root: &Path) -> String {
    Path::new(path).strip_prefix(root).map_or_else(
        |_| path.to_owned(),
        |rel| rel.to_string_lossy().into_owned(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fmt::Write as _;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::sync::Arc;

    const MATCHING_BODY: &str = "let total = value + 1u32; total * 2u32";

    fn put(root: &Path, rel: &str, text: &str) {
        let path = root.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }

    /// A workspace whose dependencies cover every edge kind and platform.
    fn fixture(root: &Path) {
        put(
            root,
            "Cargo.toml",
            "[workspace]\nresolver = \"3\"\nmembers = [\"app\", \"other\"]\nexclude = [\"deps/direct\", \"deps/transitive\", \"deps/development\", \"deps/optional\", \"deps/opt2\", \"deps/winonly\", \"deps/winopt\"]\n",
        );
        put(
            root,
            "app/Cargo.toml",
            "[package]\nname = \"app\"\nversion = \"0.1.0\"\nedition = \"2024\"\n[dependencies]\ndirect = { path = \"../deps/direct\" }\noptional = { path = \"../deps/optional\", optional = true }\n[dev-dependencies]\ndevelopment = { path = \"../deps/development\" }\n[features]\nwith-optional = [\"dep:optional\"]\n[target.'x86_64-pc-windows-gnu'.dependencies]\nwinonly = { path = \"../deps/winonly\" }\nwinopt = { path = \"../deps/winopt\", optional = true }\n",
        );
        put(
            root,
            "app/src/lib.rs",
            &format!("pub mod shared;\npub fn anchor(value: u32) -> u32 {{ {MATCHING_BODY} }}\n"),
        );
        put(
            root,
            "app/src/shared.rs",
            &format!("pub fn shared(value: u32) -> u32 {{ {MATCHING_BODY} }}\n"),
        );
        put(
            root,
            "other/Cargo.toml",
            "[package]\nname = \"other\"\nversion = \"0.1.0\"\nedition = \"2024\"\n[dependencies]\napp = { path = \"../app\" }\n",
        );
        put(
            root,
            "other/src/lib.rs",
            "pub fn anchor(input: u32) -> u32 { let result = input + 1u32; result * 2u32 }\n",
        );
        put(
            root,
            "deps/direct/Cargo.toml",
            "[package]\nname = \"direct\"\nversion = \"0.1.0\"\nedition = \"2024\"\n[dependencies]\ntransitive = { path = \"../transitive\" }\nopt2 = { path = \"../opt2\", optional = true }\n[workspace]\n",
        );
        put(
            root,
            "deps/direct/src/lib.rs",
            "pub fn helper(input: u32) -> u32 { let result = input + 1u32; result * 2u32 }\n",
        );
        for (name, increment) in [
            ("transitive", 7),
            ("development", 11),
            ("optional", 13),
            ("opt2", 17),
            ("winonly", 19),
            ("winopt", 23),
        ] {
            put(
                root,
                &format!("deps/{name}/Cargo.toml"),
                &format!(
                    "[package]\nname = \"{name}\"\nversion = \"0.1.0\"\nedition = \"2024\"\n[workspace]\n"
                ),
            );
            put(
                root,
                &format!("deps/{name}/src/lib.rs"),
                &format!("pub fn helper(input: u32) -> u32 {{ input + {increment}u32 }}\n"),
            );
            let local_path = root.join("app/src/lib.rs");
            let mut local_source = std::fs::read_to_string(&local_path).unwrap();
            writeln!(
                local_source,
                "pub fn {name}_anchor(input: u32) -> u32 {{ input + {increment}u32 }}"
            )
            .unwrap();
            std::fs::write(local_path, local_source).unwrap();
        }
    }

    /// Local records of one crate, extracted the same way the scan does.
    fn local_records(
        dir: &Path,
        crate_name: &str,
        files: &[(&str, Vec<&str>)],
    ) -> Vec<IndexedContext> {
        let mut sources = Vec::new();
        for (rel, segments) in files {
            let text = std::fs::read_to_string(dir.join(rel)).unwrap();
            sources.push(SourceFile {
                path: dir.join(rel).to_string_lossy().into_owned(),
                segments: segments
                    .iter()
                    .map(|segment| (*segment).to_owned())
                    .collect(),
                cfg: Vec::new(),
                parsed: syn::parse_file(&text).unwrap(),
                text,
                rustdoc: true,
            });
        }
        let scan = TargetScan {
            name: crate_name.to_owned(),
            files: sources,
            library: true,
        };
        crate::context_index::extract(&scan, &dir.to_string_lossy(), false, &[])
    }

    fn app_targets(dir: &Path) -> Vec<TargetScan> {
        let text = std::fs::read_to_string(dir.join("app/src/lib.rs")).unwrap();
        let shared = std::fs::read_to_string(dir.join("app/src/shared.rs")).unwrap();
        vec![TargetScan {
            name: "app".to_owned(),
            files: vec![
                SourceFile {
                    path: dir.join("app/src/lib.rs").to_string_lossy().into_owned(),
                    segments: Vec::new(),
                    cfg: Vec::new(),
                    parsed: syn::parse_file(&text).unwrap(),
                    text,
                    rustdoc: true,
                },
                SourceFile {
                    path: dir.join("app/src/shared.rs").to_string_lossy().into_owned(),
                    segments: vec!["shared".to_owned()],
                    cfg: Vec::new(),
                    parsed: syn::parse_file(&shared).unwrap(),
                    text: shared,
                    rustdoc: true,
                },
            ],
            library: true,
        }]
    }

    fn run(
        dir: &Path,
        local: &[IndexedContext],
        targets: &[TargetScan],
        options: &DependencySearch,
    ) -> ReuseReport {
        let refs: Vec<&IndexedContext> = local.iter().collect();
        analyze(dir, targets, &refs, options, 0, false, &[])
    }

    fn packages(report: &ReuseReport) -> Vec<String> {
        let mut names: Vec<String> = report
            .groups
            .iter()
            .flat_map(|group| group.candidates.iter())
            .map(|candidate| candidate.package.clone())
            .collect();
        names.sort();
        names.dedup();
        names
    }

    fn app_options(_dir: &Path, cache: &Path, depth: usize) -> DependencySearch {
        DependencySearch {
            depth: Some(depth),
            check_latest: Some(false),
            cache_dir: Some(cache.to_string_lossy().into_owned()),
            ..DependencySearch::default()
        }
    }

    fn app_records(dir: &Path) -> Vec<IndexedContext> {
        local_records(
            dir,
            "app",
            &[
                ("app/src/lib.rs", vec![]),
                ("app/src/shared.rs", vec!["shared"]),
            ],
        )
    }

    #[test]
    fn candidate_ranking_preserves_verified_repository_links() {
        let dir = tempfile::tempdir().unwrap();
        fixture(dir.path());
        let local = app_records(dir.path());
        let refs: Vec<&IndexedContext> = local.iter().collect();
        let server = crate::context_network::tests::Fixture::start();
        let sha = "a".repeat(40);
        let mut forms: BTreeMap<String, Vec<DepRec>> = BTreeMap::new();
        let mut hashes = BTreeMap::new();
        for (name, visibility) in [("aaa", ""), ("zzz", "pub ")] {
            let root = dir.path().join(name);
            let text = format!("{visibility}fn helper(value: u32) -> u32 {{ {MATCHING_BODY} }}\n");
            put(&root, "src/lib.rs", &text);
            put(
                &root,
                ".cargo_vcs_info.json",
                &format!("{{\"git\":{{\"sha1\":\"{sha}\"}},\"path_in_vcs\":\"\"}}"),
            );
            server.route(
                &format!("/raw/o/{name}/{sha}/src/lib.rs"),
                200,
                text.as_bytes(),
            );
            hashes.insert(name, blake3::hash(text.as_bytes()).to_hex().to_string());
            for record in local_records(&root, name, &[("src/lib.rs", vec![])]) {
                forms.entry(record.form.clone()).or_default().push(DepRec {
                    name: name.to_owned(),
                    version: "1.0.0".to_owned(),
                    source: CRATES_IO_SOURCE.to_owned(),
                    depth: 1,
                    root: root.clone(),
                    repository: Some(format!("https://github.com/o/{name}")),
                    latest_only: false,
                    record,
                });
            }
        }
        server.route(
            "/api/search/issues",
            200,
            br#"{"total_count":0,"items":[]}"#,
        );
        let (mut groups, origins) = group(&refs, &forms, dir.path(), 0);
        let mut upstream = crate::context_upstream::Upstream::with_base(
            &dir.path().join("index"),
            &server.api_base(),
            &server.raw_base(),
        );
        assert_eq!(groups[0].candidates[0].package, "zzz");
        for (group, origins) in groups.iter_mut().zip(origins) {
            for (candidate, origin) in group.candidates.iter_mut().zip(origins) {
                let package = PackageSource {
                    name: candidate.package.clone(),
                    version: candidate.version.clone(),
                    source: candidate.source.clone(),
                    root: origin.root,
                    repository: origin.repository,
                    source_hashes: BTreeMap::from([(
                        "src/lib.rs".to_owned(),
                        hashes[candidate.package.as_str()].clone(),
                    )]),
                };
                upstream.enrich(&package, &mut [candidate]);
                assert_eq!(
                    candidate.source_url,
                    Some(format!(
                        "https://github.com/o/{}/blob/{sha}/src/lib.rs#L1-L1",
                        candidate.package
                    ))
                );
            }
        }
    }

    #[test]
    fn depth_and_kind_control_reach() {
        let dir = tempfile::tempdir().unwrap();
        let cache = dir.path().join("index");
        fixture(dir.path());
        let local = app_records(dir.path());
        let targets = app_targets(dir.path());
        let mut options = app_options(dir.path(), &cache, 1);
        let report = run(dir.path(), &local, &targets, &options);
        assert_eq!(packages(&report), vec!["direct".to_owned()]);
        options.depth = Some(2);
        let report = run(dir.path(), &local, &targets, &options);
        assert_eq!(
            packages(&report),
            vec!["direct".to_owned(), "transitive".to_owned()]
        );
        options.include_dev = Some(true);
        options.depth = Some(1);
        let report = run(dir.path(), &local, &targets, &options);
        assert_eq!(
            packages(&report),
            vec!["development".to_owned(), "direct".to_owned()]
        );
        options.include_dev = None;
        options.features = Some(vec!["with-optional".to_owned()]);
        let report = run(dir.path(), &local, &targets, &options);
        assert_eq!(
            packages(&report),
            vec!["direct".to_owned(), "optional".to_owned()]
        );
    }

    #[test]
    fn distinct_path_packages_retain_each_resolved_api() {
        let dir = tempfile::tempdir().unwrap();
        fixture(dir.path());
        let root_path = dir.path().join("Cargo.toml");
        let mut root: toml::Value =
            toml::from_str(&std::fs::read_to_string(&root_path).unwrap()).unwrap();
        let excluded = root["workspace"]["exclude"].as_array_mut().unwrap();
        for (folder, version) in [("one", "1.0.0"), ("two", "2.0.0")] {
            excluded.push(toml::Value::String(folder.to_owned()));
            put(
                dir.path(),
                &format!("{folder}/Cargo.toml"),
                &format!(
                    "[package]\nname=\"shared\"\nversion=\"{version}\"\nedition=\"2024\"\n[workspace]\n"
                ),
            );
            put(
                dir.path(),
                &format!("{folder}/src/lib.rs"),
                &format!("pub fn helper(value: u32) -> u32 {{ {MATCHING_BODY} }}"),
            );
        }
        std::fs::write(root_path, toml::to_string(&root).unwrap()).unwrap();
        let path = dir.path().join("app/Cargo.toml");
        let mut app = std::fs::read_to_string(&path).unwrap();
        app.push_str("\n[dependencies.first]\npackage=\"shared\"\npath=\"../one\"\n[dependencies.second]\npackage=\"shared\"\npath=\"../two\"\n");
        std::fs::write(path, app).unwrap();
        let report = run(
            dir.path(),
            &app_records(dir.path()),
            &app_targets(dir.path()),
            &app_options(dir.path(), &dir.path().join("index"), 1),
        );
        let versions: BTreeSet<&str> = report
            .groups
            .iter()
            .flat_map(|group| &group.candidates)
            .filter(|candidate| candidate.package == "shared")
            .map(|candidate| candidate.version.as_str())
            .collect();
        assert_eq!(versions, BTreeSet::from(["1.0.0", "2.0.0"]));
    }

    #[test]
    fn build_dependencies_require_explicit_inclusion() {
        let dir = tempfile::tempdir().unwrap();
        fixture(dir.path());
        put(
            dir.path(),
            "building/Cargo.toml",
            "[package]\nname=\"building\"\nversion=\"0.1.0\"\nedition=\"2024\"\n",
        );
        put(
            dir.path(),
            "building/src/lib.rs",
            &format!("pub fn helper(value: u32) -> u32 {{ {MATCHING_BODY} }}"),
        );
        let path = dir.path().join("app/Cargo.toml");
        let mut manifest = std::fs::read_to_string(&path).unwrap();
        manifest.push_str("\n[build-dependencies]\nbuilding = { path = \"../building\" }\n");
        std::fs::write(path, manifest).unwrap();
        let local = app_records(dir.path());
        let targets = app_targets(dir.path());
        let mut options = app_options(dir.path(), &dir.path().join("index"), 1);
        assert_eq!(
            packages(&run(dir.path(), &local, &targets, &options)),
            ["direct"]
        );
        options.include_build = Some(true);
        assert_eq!(
            packages(&run(dir.path(), &local, &targets, &options)),
            ["building", "direct"]
        );
    }

    #[test]
    fn default_and_all_features_control_optional_reuse() {
        let dir = tempfile::tempdir().unwrap();
        fixture(dir.path());
        let root_path = dir.path().join("Cargo.toml");
        let mut root: toml::Value =
            toml::from_str(&std::fs::read_to_string(&root_path).unwrap()).unwrap();
        root["workspace"]["members"] =
            toml::Value::Array(vec![toml::Value::String("app".to_owned())]);
        std::fs::write(root_path, toml::to_string(&root).unwrap()).unwrap();
        let app_path = dir.path().join("app/Cargo.toml");
        let mut app: toml::Value =
            toml::from_str(&std::fs::read_to_string(&app_path).unwrap()).unwrap();
        app["features"].as_table_mut().unwrap().insert(
            "default".to_owned(),
            toml::Value::Array(vec![toml::Value::String("with-optional".to_owned())]),
        );
        std::fs::write(app_path, toml::to_string(&app).unwrap()).unwrap();
        let local = app_records(dir.path());
        let targets = app_targets(dir.path());
        let mut options = app_options(dir.path(), &dir.path().join("index"), 1);
        assert_eq!(
            packages(&run(dir.path(), &local, &targets, &options)),
            ["direct", "optional"]
        );
        options.no_default_features = Some(true);
        assert_eq!(
            packages(&run(dir.path(), &local, &targets, &options)),
            ["direct"]
        );
        options.all_features = Some(true);
        assert_eq!(
            packages(&run(dir.path(), &local, &targets, &options)),
            ["direct", "optional"]
        );
    }

    #[test]
    #[cfg(unix)]
    fn subprocess_deadline_kills_descendants_and_quotes_request() {
        let dir = tempfile::tempdir().unwrap();
        let id = next_proc_id();
        let args = ["-c", "sleep 0.5; printf leaked > escaped"].map(str::to_owned);
        let started = std::time::Instant::now();
        let result = run_bounded(
            std::process::Command::new("sh")
                .args(&args)
                .current_dir(dir.path()),
            std::time::Duration::from_millis(50),
            &id,
        );
        let error = result.err().unwrap().to_string();
        assert!(error.contains(&id));
        assert!(error.contains("deadline"));
        assert!(started.elapsed() < std::time::Duration::from_secs(15));
        std::thread::sleep(std::time::Duration::from_millis(600));
        assert!(!dir.path().join("escaped").exists());
    }

    #[test]
    fn all_optional_explores_disabled_edges() {
        let dir = tempfile::tempdir().unwrap();
        let cache = dir.path().join("index");
        fixture(dir.path());
        let local = app_records(dir.path());
        let targets = app_targets(dir.path());
        let mut options = app_options(dir.path(), &cache, 2);
        options.all_optional = Some(true);
        let report = run(dir.path(), &local, &targets, &options);
        assert_eq!(
            packages(&report),
            vec![
                "direct".to_owned(),
                "opt2".to_owned(),
                "optional".to_owned(),
                "transitive".to_owned(),
            ]
        );
    }

    #[test]
    fn target_specific_edges_follow_the_effective_target() {
        let dir = tempfile::tempdir().unwrap();
        let cache = dir.path().join("index");
        fixture(dir.path());
        let local = app_records(dir.path());
        let targets = app_targets(dir.path());
        let mut options = app_options(dir.path(), &cache, 1);
        options.all_optional = Some(true);
        options.target = Some("x86_64-pc-windows-gnu".to_owned());
        let report = run(dir.path(), &local, &targets, &options);
        let names = packages(&report);
        assert!(
            names.iter().any(|name| name == "winonly"),
            "windows target must include the windows-only dependency, got {names:?}"
        );
        assert!(
            names.iter().any(|name| name == "winopt"),
            "windows target must explore the windows-only optional dependency, got {names:?}"
        );
        options.target = None;
        let report = run(dir.path(), &local, &targets, &options);
        let names = packages(&report);
        assert!(!names.iter().any(|name| name == "winonly"), "{names:?}");
        assert!(!names.iter().any(|name| name == "winopt"), "{names:?}");
    }

    #[test]
    fn the_configured_build_targets_select_target_specific_edges() {
        let dir = tempfile::tempdir().unwrap();
        let cache = dir.path().join("index");
        fixture(dir.path());
        put(
            dir.path(),
            ".cargo/config.toml",
            "[build]\ntarget = [\"x86_64-unknown-linux-gnu\", \"x86_64-pc-windows-gnu\"]\n",
        );
        let local = app_records(dir.path());
        let targets = app_targets(dir.path());
        let report = run(
            dir.path(),
            &local,
            &targets,
            &app_options(dir.path(), &cache, 1),
        );
        let names = packages(&report);
        assert!(names.iter().any(|name| name == "winonly"), "{names:?}");
        assert!(report.coverage.complete, "{:?}", report.coverage.failures);
    }

    #[test]
    fn cross_member_candidates_keep_other_crate_only() {
        let dir = tempfile::tempdir().unwrap();
        let cache = dir.path().join("index");
        fixture(dir.path());
        let app = app_records(dir.path());
        let other = local_records(dir.path(), "other", &[("other/src/lib.rs", vec![])]);
        let mut local = app.clone();
        local.extend(other);
        let mut targets = app_targets(dir.path());
        let other_text = std::fs::read_to_string(dir.path().join("other/src/lib.rs")).unwrap();
        targets.push(TargetScan {
            name: "other".to_owned(),
            files: vec![SourceFile {
                path: dir
                    .path()
                    .join("other/src/lib.rs")
                    .to_string_lossy()
                    .into_owned(),
                segments: Vec::new(),
                cfg: Vec::new(),
                parsed: syn::parse_file(&other_text).unwrap(),
                text: other_text,
                rustdoc: true,
            }],
            library: true,
        });
        let options = app_options(dir.path(), &cache, 1);
        let report = run(dir.path(), &local, &targets, &options);
        let groups: Vec<_> = report
            .groups
            .iter()
            .filter(|group| group.local.file == "other/src/lib.rs")
            .collect();
        assert_eq!(groups.len(), 1, "the other crate anchor must group once");
        let names: Vec<String> = groups[0]
            .candidates
            .iter()
            .map(|c| c.package.clone())
            .collect();
        assert!(
            names.iter().any(|name| name == "app"),
            "the app candidate must reach the other crate anchor, got {names:?}"
        );
        let app_groups: Vec<_> = report
            .groups
            .iter()
            .filter(|group| group.local.file == "app/src/lib.rs")
            .collect();
        for group in &app_groups {
            for candidate in &group.candidates {
                assert_ne!(
                    candidate.package, "app",
                    "a self-crate candidate must not surface as a dependency finding"
                );
            }
        }
    }

    #[test]
    fn lockfile_is_not_created_or_touched() {
        let dir = tempfile::tempdir().unwrap();
        let cache = dir.path().join("index");
        fixture(dir.path());
        assert!(!dir.path().join("Cargo.lock").exists());
        let local = app_records(dir.path());
        let targets = app_targets(dir.path());
        let options = app_options(dir.path(), &cache, 1);
        run(dir.path(), &local, &targets, &options);
        assert!(!dir.path().join("Cargo.lock").exists());
        put(
            dir.path(),
            "Cargo.lock",
            "# This file is automatically generated by Cargo.\n# It is not intended for manual editing.\nversion = 4\n",
        );
        let before = std::fs::read(dir.path().join("Cargo.lock")).unwrap();
        run(dir.path(), &local, &targets, &options);
        let after = std::fs::read(dir.path().join("Cargo.lock")).unwrap();
        assert_eq!(
            before, after,
            "a present lockfile must come back byte-identical"
        );
    }

    #[test]
    fn mutable_source_cache_reuses_then_invalidates() {
        let dir = tempfile::tempdir().unwrap();
        let cache = dir.path().join("index");
        fixture(dir.path());
        let local = app_records(dir.path());
        let targets = app_targets(dir.path());
        let options = app_options(dir.path(), &cache, 1);
        let first = run(dir.path(), &local, &targets, &options);
        assert_eq!(packages(&first), vec!["direct".to_owned()]);
        assert_eq!(first.coverage.cached_files, 0);
        let second = run(dir.path(), &local, &targets, &options);
        assert_eq!(packages(&second), vec!["direct".to_owned()]);
        assert!(
            second.coverage.cached_files > 0,
            "the second run must load the direct index from the cache"
        );
        assert!(second.coverage.complete);
        assert_eq!(second.coverage.indexed_files, first.coverage.indexed_files);
        assert_eq!(second.coverage.cached_files, first.coverage.indexed_files);
        put(
            dir.path(),
            "deps/direct/src/lib.rs",
            "pub fn helper(input: u32) -> u32 { input + 101u32 }\n",
        );
        let third = run(dir.path(), &local, &targets, &options);
        assert!(
            packages(&third).is_empty(),
            "the changed source must no longer match"
        );
    }

    #[test]
    fn public_api_paths_use_the_library_target_name() {
        let dir = tempfile::tempdir().unwrap();
        let cache = dir.path().join("index");
        fixture(dir.path());
        put(
            dir.path(),
            "deps/direct/Cargo.toml",
            "[package]\nname = \"direct\"\nversion = \"0.1.0\"\nedition = \"2024\"\n[lib]\nname = \"actual_api\"\n[workspace]\n",
        );
        let local = app_records(dir.path());
        let targets = app_targets(dir.path());
        let options = app_options(dir.path(), &cache, 1);
        let report = run(dir.path(), &local, &targets, &options);
        assert!(report.coverage.complete, "{:?}", report.coverage);
        let candidate = report
            .groups
            .iter()
            .flat_map(|group| &group.candidates)
            .find(|candidate| candidate.package == "direct")
            .unwrap();
        assert_eq!(candidate.api_path.as_deref(), Some("actual_api::helper"));
    }

    #[test]
    fn generated_source_options_invalidate_cached_contexts() {
        let dir = tempfile::tempdir().unwrap();
        let cache = dir.path().join("index");
        fixture(dir.path());
        put(
            dir.path(),
            "deps/direct/src/lib.rs",
            "pub fn helper(input: u32) -> u32 { let result = input + 1u32; result * 2u32 }\n",
        );
        let local = app_records(dir.path());
        let targets = app_targets(dir.path());
        let options = app_options(dir.path(), &cache, 1);
        let refs: Vec<&IndexedContext> = local.iter().collect();
        let excluded = vec!["pub fn helper".to_owned()];
        let first = analyze(dir.path(), &targets, &refs, &options, 0, false, &excluded);
        assert_eq!(packages(&first), Vec::<String>::new());
        let second = analyze(dir.path(), &targets, &refs, &options, 0, true, &excluded);
        assert_eq!(packages(&second), vec!["direct".to_owned()]);
        let third = analyze(dir.path(), &targets, &refs, &options, 0, false, &[]);
        assert_eq!(packages(&third), vec!["direct".to_owned()]);
        let fourth = analyze(dir.path(), &targets, &refs, &options, 0, false, &excluded);
        assert_eq!(packages(&fourth), Vec::<String>::new());
    }

    #[test]
    fn unreadable_dependency_source_marks_incomplete_and_skips_cache() {
        let dir = tempfile::tempdir().unwrap();
        let cache = dir.path().join("index");
        fixture(dir.path());
        let local = app_records(dir.path());
        let targets = app_targets(dir.path());
        let options = app_options(dir.path(), &cache, 1);
        let lib = dir.path().join("deps/direct/src/lib.rs");
        std::fs::remove_file(&lib).unwrap();
        std::os::unix::fs::symlink("missing.rs", &lib).unwrap();
        let report = run(dir.path(), &local, &targets, &options);
        assert!(
            !report.coverage.complete,
            "a broken source must leave the analysis incomplete"
        );
        assert!(
            report
                .coverage
                .failures
                .iter()
                .any(|failure| failure.contains("direct")),
            "the failure must name the broken source, got {:?}",
            report.coverage.failures
        );
        assert!(
            packages(&report).iter().all(|name| name != "direct"),
            "the broken source must not yield candidates"
        );
        let manifest_dir = std::fs::canonicalize(dir.path().join("deps/direct")).unwrap();
        let key = source_key("path", &manifest_dir.to_string_lossy());
        let index = cache.join("private").join(key).join("direct.json");
        assert!(
            !index.is_file(),
            "an incomplete discovery must not persist an index entry"
        );
    }

    /// Socket I/O deadline for the test fixture.
    const SOCKET_IO_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);
    /// Poll interval for the fixture's nonblocking accept.
    const ACCEPT_POLL: std::time::Duration = std::time::Duration::from_millis(5);

    /// A single-threaded test HTTP server, with bounded socket I/O and a
    /// bounded shutdown.
    struct HttpServer {
        base: String,
        handle: std::thread::JoinHandle<()>,
        shutdown: Arc<std::sync::atomic::AtomicBool>,
    }

    impl HttpServer {
        fn start(routes: BTreeMap<String, Vec<u8>>) -> Self {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let port = listener.local_addr().unwrap().port();
            let shutdown = Arc::new(std::sync::atomic::AtomicBool::new(false));
            let control = Arc::clone(&shutdown);
            let handle = std::thread::spawn(move || {
                let listener = listener;
                let _ = listener.set_nonblocking(true);
                loop {
                    if control.load(std::sync::atomic::Ordering::Acquire) {
                        break;
                    }
                    match listener.accept() {
                        Ok((stream, _)) => serve_one(stream, &routes),
                        Err(_) => std::thread::sleep(ACCEPT_POLL),
                    }
                }
            });
            Self {
                base: format!("http://127.0.0.1:{port}"),
                handle,
                shutdown,
            }
        }

        fn stop(self) {
            self.shutdown
                .store(true, std::sync::atomic::Ordering::Release);
            let _ = self.handle.join();
        }
    }

    /// Serve one connection, with bounded socket reads and writes.
    fn serve_one(mut stream: std::net::TcpStream, routes: &BTreeMap<String, Vec<u8>>) {
        let _ = stream.set_read_timeout(Some(SOCKET_IO_TIMEOUT));
        let _ = stream.set_write_timeout(Some(SOCKET_IO_TIMEOUT));
        let mut head = Vec::new();
        let mut chunk = [0u8; 1024];
        loop {
            let Ok(n) = stream.read(&mut chunk) else {
                return;
            };
            if n == 0 {
                return;
            }
            head.extend_from_slice(&chunk[..n]);
            if head.windows(4).any(|window| window == b"\r\n\r\n") {
                break;
            }
        }
        let request = String::from_utf8_lossy(&head);
        let path = request
            .lines()
            .next()
            .and_then(|line| line.split_whitespace().nth(1))
            .unwrap_or_default()
            .to_owned();
        let (status, body) = routes
            .get(&path)
            .map_or(("404 Not Found", b"not found".as_slice()), |body| {
                ("200 OK", body.as_slice())
            });
        let response = format!(
            "HTTP/1.1 {status}\r\nContent-Type: application/octet-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            body.len()
        );
        let _ = stream.write_all(response.as_bytes());
        let _ = stream.write_all(body);
    }

    fn crate_archive(name: &str, version: &str, lib: &str) -> Vec<u8> {
        let manifest =
            format!("[package]\nname = \"{name}\"\nversion = \"{version}\"\nedition = \"2021\"\n");
        let top = format!("{name}-{version}");
        let mut bytes = Vec::new();
        let encoder = flate2::write::GzEncoder::new(&mut bytes, flate2::Compression::default());
        let mut tar = tar::Builder::new(encoder);
        for (file, text) in [
            (format!("{top}/Cargo.toml"), manifest.clone()),
            (format!("{top}/src/lib.rs"), lib.to_owned()),
        ] {
            let data = text.as_bytes();
            let mut header = tar::Header::new_gnu();
            // data.len is a byte count, lossless into u64
            header.set_size(data.len() as u64);
            header.set_mode(0o644);
            header.set_cksum();
            tar.append_data(&mut header, file, data).unwrap();
        }
        let encoder = tar.into_inner().unwrap();
        encoder.finish().unwrap();
        bytes
    }

    const RESOLVED_LIB: &str = "pub fn both(input: u32) -> u32 { let result = input + 1u32; result * 2u32 }\npub fn only_old(input: u32) -> u32 { input + 3u32 }\n";
    const LATEST_LIB: &str = "pub fn both(input: u32) -> u32 { let result = input + 1u32; result * 2u32 }\npub fn only_new(input: u32) -> u32 { input + 4u32 }\n";

    /// Registry metadata rooted at `ws` without invoking Cargo.
    fn registry_workspace_json(ws: &Path) -> String {
        let ws = ws.to_string_lossy();
        let value = serde_json::json!({
            "packages": [
                {
                    "name": "app",
                    "version": "0.1.0",
                    "id": "path+file:///ws/app#0.1.0",
                    "source": null,
                    "dependencies": [
                        {
                            "name": "upstream",
                            "source": "registry+https://github.com/rust-lang/crates.io-index",
                            "req": "^1.0",
                            "kind": null,
                            "optional": false,
                            "uses_default_features": true,
                            "features": [],
                            "target": null,
                            "rename": null,
                            "registry": null,
                            "path": null
                        }
                    ],
                    "targets": [
                        { "name": "app", "kind": ["lib"], "crate_types": ["lib"], "src_path": format!("{ws}/app/src/lib.rs") }
                    ],
                    "manifest_path": format!("{ws}/app/Cargo.toml"),
                    "metadata": null,
                    "features": {}
                },
                {
                    "name": "upstream",
                    "version": "1.0.0",
                    "id": "registry+https://github.com/rust-lang/crates.io-index#upstream/1.0.0",
                    "source": "registry+https://github.com/rust-lang/crates.io-index",
                    "dependencies": [],
                    "targets": [
                        { "name": "upstream", "kind": ["lib"], "crate_types": ["lib"], "src_path": format!("{ws}/vendor/upstream/src/lib.rs") }
                    ],
                    "manifest_path": format!("{ws}/vendor/upstream/Cargo.toml"),
                    "metadata": null,
                    "features": {}
                }
            ],
            "workspace_members": ["path+file:///ws/app#0.1.0"],
            "resolve": {
                "nodes": [
                    {
                        "id": "path+file:///ws/app#0.1.0",
                        "deps": [
                            {
                                "name": "upstream",
                                "pkg": "registry+https://github.com/rust-lang/crates.io-index#upstream/1.0.0",
                                "dep_kinds": [{ "kind": "normal" }]
                            }
                        ],
                        "dependencies": ["registry+https://github.com/rust-lang/crates.io-index#upstream/1.0.0"],
                        "features": []
                    },
                    {
                        "id": "registry+https://github.com/rust-lang/crates.io-index#upstream/1.0.0",
                        "deps": [],
                        "dependencies": [],
                        "features": []
                    }
                ]
            },
            "workspace_root": format!("{ws}"),
            "target_directory": format!("{ws}/target"),
            "metadata": null,
            "version": 1
        });
        serde_json::to_string(&value).unwrap()
    }

    fn registry_run(ws: &Path, cache: &Path, base: &str, check_latest: bool) -> ReuseReport {
        let local = local_records(ws, "app", &[("app/src/lib.rs", vec![])]);
        let refs: Vec<&IndexedContext> = local.iter().collect();
        let metadata: Metadata = serde_json::from_str(&registry_workspace_json(ws)).unwrap();
        let graph = build_graph(&metadata, &BTreeMap::new());
        let store = Store::open(cache).unwrap();
        let options = DependencySearch {
            depth: Some(1),
            check_latest: Some(check_latest),
            cache_dir: Some(cache.to_string_lossy().into_owned()),
            ..DependencySearch::default()
        };
        let eff = effective(&options);
        let mut d = Discovery {
            graph,
            index_fingerprint: index_fingerprint(&eff),
            eff,
            store: Some(store),
            store_root: cache.to_path_buf(),
            registry: Registry {
                api: format!("{base}/api/v1/crates"),
                cdn: format!("{base}/crates"),
            },
            coverage: DependencyCoverage::default(),
            effective_targets: Vec::new(),
            cfgs: BTreeMap::new(),
            latest_known: BTreeMap::new(),
        };
        let targets = single_lib_targets(ws);
        analyze_discovery(&mut d, ws, &targets, &refs, 0)
    }

    /// A single-library target scan of the registry workspace app.
    fn single_lib_targets(ws: &Path) -> Vec<TargetScan> {
        let text = std::fs::read_to_string(ws.join("app/src/lib.rs")).unwrap();
        vec![TargetScan {
            name: "app".to_owned(),
            files: vec![SourceFile {
                path: ws.join("app/src/lib.rs").to_string_lossy().into_owned(),
                segments: Vec::new(),
                cfg: Vec::new(),
                parsed: syn::parse_file(&text).unwrap(),
                text,
                rustdoc: true,
            }],
            library: true,
        }]
    }

    #[test]
    fn missing_manifest_prevents_private_index_persistence() {
        let dir = tempfile::tempdir().unwrap();
        let ws = dir.path().join("ws");
        let cache = dir.path().join("index");
        put(
            &ws,
            "app/src/lib.rs",
            &format!("pub fn local_match(value: u32) -> u32 {{ {MATCHING_BODY} }}\n"),
        );
        let mut routes = BTreeMap::new();
        routes.insert(
            "/crates/upstream/upstream-1.0.0.crate".to_owned(),
            crate_archive("upstream", "1.0.0", RESOLVED_LIB),
        );
        let server = HttpServer::start(routes);
        let report = registry_run(&ws, &cache, &server.base, false);
        server.stop();
        assert!(!report.coverage.complete);
        assert_eq!(packages(&report), vec!["upstream".to_owned()]);
        let key = source_key("path", &ws.join("app").to_string_lossy());
        assert!(!cache.join("private").join(key).join("app.json").exists());
    }

    #[test]
    fn registry_source_privacy_requires_published_bytes() {
        for identical in [false, true] {
            let dir = tempfile::tempdir().unwrap();
            let ws = dir.path().join("ws");
            let cache = dir.path().join("index");
            put(
                &ws,
                "app/Cargo.toml",
                "[package]\nname = \"app\"\nversion = \"0.1.0\"\n",
            );
            put(
                &ws,
                "app/src/lib.rs",
                &format!("pub fn local_match(value: u32) -> u32 {{ {MATCHING_BODY} }}\n"),
            );
            put(
                &ws,
                "vendor/upstream/Cargo.toml",
                "[package]\nname = \"upstream\"\nversion = \"1.0.0\"\nedition = \"2021\"\n",
            );
            let private_body =
                format!("pub fn private_helper(value: u32) -> u32 {{ {MATCHING_BODY} }}\n");
            put(
                &ws,
                "vendor/upstream/src/lib.rs",
                if identical {
                    RESOLVED_LIB
                } else {
                    &private_body
                },
            );
            let mut routes = BTreeMap::new();
            routes.insert(
                "/crates/upstream/upstream-1.0.0.crate".to_owned(),
                crate_archive("upstream", "1.0.0", RESOLVED_LIB),
            );
            let server = HttpServer::start(routes);
            let report = registry_run(&ws, &cache, &server.base, false);
            server.stop();
            assert!(report.coverage.complete, "{:?}", report.coverage);
            assert_eq!(packages(&report), vec!["upstream".to_owned()]);
            let public_index = cache.join("public/crates.io/upstream-1.0.0.json");
            assert_eq!(public_index.exists(), identical);
            let key = source_key("registry", &ws.join("vendor/upstream").to_string_lossy());
            let private_index = cache.join("private").join(key).join("upstream-1.0.0.json");
            assert_eq!(private_index.exists(), !identical);
            let candidate = &report.groups[0].candidates[0];
            assert_eq!(
                candidate.site.item,
                if identical {
                    "upstream::both"
                } else {
                    "upstream::private_helper"
                }
            );
        }
    }

    #[test]
    fn newly_public_helper_requires_upgrade() {
        let dir = tempfile::tempdir().unwrap();
        let ws = dir.path().join("ws");
        put(
            &ws,
            "app/Cargo.toml",
            "[package]\nname=\"app\"\nversion=\"0.1.0\"\nedition=\"2024\"\n",
        );
        put(
            &ws,
            "app/src/lib.rs",
            &format!("pub fn local_match(value: u32) -> u32 {{ {MATCHING_BODY} }}"),
        );
        let old = format!("    fn helper(value: u32) -> u32 {{ {MATCHING_BODY} }}");
        let latest = format!("pub fn helper(value: u32) -> u32 {{ {MATCHING_BODY} }}");
        let routes = BTreeMap::from([
            (
                "/api/v1/crates/upstream".to_owned(),
                br#"{"crate":{"max_stable_version":"1.2.0"}}"#.to_vec(),
            ),
            (
                "/crates/upstream/upstream-1.0.0.crate".to_owned(),
                crate_archive("upstream", "1.0.0", &old),
            ),
            (
                "/crates/upstream/upstream-1.2.0.crate".to_owned(),
                crate_archive("upstream", "1.2.0", &latest),
            ),
        ]);
        let server = HttpServer::start(routes);
        let report = registry_run(&ws, &dir.path().join("index"), &server.base, true);
        server.stop();
        let candidate = report
            .groups
            .iter()
            .flat_map(|group| &group.candidates)
            .find(|candidate| candidate.version == "1.2.0")
            .unwrap();
        assert_eq!(candidate.access, ContextAccess::PublicFunction);
        assert_eq!(candidate.action, ReuseAction::UpgradeApi);
    }

    fn assert_candidate(group: &ReuseGroup, package: &str, version: &str) -> ReuseAction {
        let candidate = group
            .candidates
            .iter()
            .find(|candidate| candidate.package == package && candidate.version == version);
        assert!(
            candidate.is_some(),
            "no {package} {version} candidate, got {:?}",
            group.candidates
        );
        candidate.unwrap().action
    }

    #[test]
    fn latest_release_classifies_upgrade_and_reuse() {
        let dir = tempfile::tempdir().unwrap();
        let ws = dir.path().join("ws");
        let cache = dir.path().join("index");
        put(
            &ws,
            "app/Cargo.toml",
            "[package]\nname = \"app\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
        );
        put(
            &ws,
            "app/src/lib.rs",
            &format!(
                "pub fn local_match(value: u32) -> u32 {{ {MATCHING_BODY} }}\npub fn local_new(value: u32) -> u32 {{ value + 4u32 }}\npub fn local_old(value: u32) -> u32 {{ value + 3u32 }}\n"
            ),
        );
        let mut routes = BTreeMap::new();
        routes.insert(
            "/api/v1/crates/upstream".to_owned(),
            br#"{"crate":{"max_stable_version":"1.2.0"}}"#.to_vec(),
        );
        routes.insert(
            "/crates/upstream/upstream-1.0.0.crate".to_owned(),
            crate_archive("upstream", "1.0.0", RESOLVED_LIB),
        );
        routes.insert(
            "/crates/upstream/upstream-1.2.0.crate".to_owned(),
            crate_archive("upstream", "1.2.0", LATEST_LIB),
        );
        let server = HttpServer::start(routes);
        let report = registry_run(&ws, &cache, &server.base, true);
        server.stop();
        let found = report
            .groups
            .iter()
            .any(|group| group.local.item.contains("local_match"));
        assert!(
            found,
            "no local_match group, groups {:?}, coverage {:?}",
            report.groups, report.coverage
        );
        let match_group = report
            .groups
            .iter()
            .find(|group| group.local.item.contains("local_match"))
            .unwrap();
        assert_eq!(
            assert_candidate(match_group, "upstream", "1.0.0"),
            ReuseAction::ReuseApi,
            "a match present at the resolved version must be a reuse"
        );
        assert!(
            !match_group
                .candidates
                .iter()
                .any(|candidate| candidate.version == "1.2.0"),
            "a match present at the resolved version must not duplicate as an upgrade, got {:?}",
            match_group.candidates
        );
        let new_group = report
            .groups
            .iter()
            .find(|group| group.local.item.contains("local_new"))
            .expect("a local_new group");
        assert_eq!(
            assert_candidate(new_group, "upstream", "1.2.0"),
            ReuseAction::UpgradeApi,
            "a match only at the latest release must be an upgrade"
        );
        let old_group = report
            .groups
            .iter()
            .find(|group| group.local.item.contains("local_old"))
            .expect("a local_old group");
        assert_eq!(
            assert_candidate(old_group, "upstream", "1.0.0"),
            ReuseAction::ReuseApi,
            "a match only at the resolved version must stay a reuse"
        );
        assert!(
            !old_group
                .candidates
                .iter()
                .any(|candidate| candidate.version == "1.2.0"),
            "a resolved-only match must not surface as a latest upgrade"
        );
        assert!(report.coverage.complete, "{:?}", report.coverage.failures);
        let archive = cache.join("public/crates.io/archives/upstream-1.0.0.crate");
        assert!(
            archive.is_file(),
            "the resolved source must land in the public cache"
        );
        let index = cache.join("public/crates.io/upstream-1.0.0.json");
        assert!(
            index.is_file(),
            "the resolved index must land in the public cache"
        );
    }

    #[test]
    fn an_unresolved_optional_registry_dependency_is_fetched_from_the_registry() {
        let dir = tempfile::tempdir().unwrap();
        let ws = dir.path().join("ws");
        let cache = dir.path().join("index");
        put(
            &ws,
            "app/Cargo.toml",
            "[package]\nname = \"app\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
        );
        put(
            &ws,
            "app/src/lib.rs",
            &format!("pub fn local_match(value: u32) -> u32 {{ {MATCHING_BODY} }}\n"),
        );
        let mut metadata: serde_json::Value =
            serde_json::from_str(&registry_workspace_json(&ws)).unwrap();
        metadata["packages"][0]["dependencies"][0]["optional"] = serde_json::json!(true);
        metadata["packages"].as_array_mut().unwrap().truncate(1);
        metadata["resolve"]["nodes"] = serde_json::json!([{
            "id": "path+file:///ws/app#0.1.0",
            "deps": [],
            "dependencies": [],
            "features": []
        }]);
        let mut routes = BTreeMap::new();
        routes.insert(
            "/api/v1/crates/upstream".to_owned(),
            br#"{"crate":{"max_stable_version":"1.0.0"},"versions":[{"num":"1.0.0"}]}"#.to_vec(),
        );
        routes.insert(
            "/crates/upstream/upstream-1.0.0.crate".to_owned(),
            crate_archive("upstream", "1.0.0", RESOLVED_LIB),
        );
        let server = HttpServer::start(routes);
        let local = local_records(&ws, "app", &[("app/src/lib.rs", vec![])]);
        let refs: Vec<&IndexedContext> = local.iter().collect();
        let metadata: Metadata = serde_json::from_value(metadata).unwrap();
        let options = DependencySearch {
            depth: Some(1),
            all_optional: Some(true),
            check_latest: Some(false),
            cache_dir: Some(cache.to_string_lossy().into_owned()),
            ..DependencySearch::default()
        };
        let eff = effective(&options);
        let mut d = Discovery {
            graph: build_graph(&metadata, &BTreeMap::new()),
            index_fingerprint: index_fingerprint(&eff),
            eff,
            store: Some(Store::open(&cache).unwrap()),
            store_root: cache.clone(),
            registry: Registry {
                api: format!("{}/api/v1/crates", server.base),
                cdn: format!("{}/crates", server.base),
            },
            coverage: DependencyCoverage::default(),
            effective_targets: Vec::new(),
            cfgs: BTreeMap::new(),
            latest_known: BTreeMap::new(),
        };
        let report = analyze_discovery(&mut d, &ws, &single_lib_targets(&ws), &refs, 0);
        server.stop();
        assert!(report.coverage.complete, "{:?}", report.coverage.failures);
        let group = report
            .groups
            .iter()
            .find(|group| group.local.item.contains("local_match"))
            .expect("a local_match group");
        assert_eq!(
            assert_candidate(group, "upstream", "1.0.0"),
            ReuseAction::ReuseApi
        );
    }

    #[test]
    fn latest_release_off_limits_to_the_resolved_version() {
        let dir = tempfile::tempdir().unwrap();
        let ws = dir.path().join("ws");
        let cache = dir.path().join("index");
        put(
            &ws,
            "app/Cargo.toml",
            "[package]\nname = \"app\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
        );
        put(
            &ws,
            "app/src/lib.rs",
            &format!(
                "pub fn local_new(value: u32) -> u32 {{ value + 4u32 }}\npub fn local_match(value: u32) -> u32 {{ {MATCHING_BODY} }}\n"
            ),
        );
        let mut routes = BTreeMap::new();
        routes.insert(
            "/api/v1/crates/upstream".to_owned(),
            br#"{"crate":{"max_stable_version":"1.2.0"}}"#.to_vec(),
        );
        routes.insert(
            "/crates/upstream/upstream-1.0.0.crate".to_owned(),
            crate_archive("upstream", "1.0.0", RESOLVED_LIB),
        );
        let server = HttpServer::start(routes);
        let report = registry_run(&ws, &cache, &server.base, false);
        server.stop();
        assert!(
            report.groups.iter().all(|group| {
                group
                    .candidates
                    .iter()
                    .all(|candidate| candidate.version != "1.2.0")
            }),
            "with the latest check off, no latest-version candidate may surface, got {:?}",
            report.groups
        );
        assert!(report.coverage.complete, "{:?}", report.coverage);
        assert!(
            !cache
                .join("public/crates.io/archives/upstream-1.2.0.crate")
                .exists(),
            "with the latest check off, the latest source must not be downloaded"
        );
    }

    #[test]
    fn latest_release_failure_keeps_resolved_findings_and_marks_incomplete() {
        let dir = tempfile::tempdir().unwrap();
        let ws = dir.path().join("ws");
        let cache = dir.path().join("index");
        put(
            &ws,
            "app/Cargo.toml",
            "[package]\nname = \"app\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
        );
        put(
            &ws,
            "app/src/lib.rs",
            &format!("pub fn local_match(value: u32) -> u32 {{ {MATCHING_BODY} }}\n"),
        );
        let mut routes = BTreeMap::new();
        routes.insert(
            "/crates/upstream/upstream-1.0.0.crate".to_owned(),
            crate_archive("upstream", "1.0.0", RESOLVED_LIB),
        );
        let server = HttpServer::start(routes);
        let report = registry_run(&ws, &cache, &server.base, true);
        server.stop();
        assert!(
            !report.coverage.complete,
            "a failed latest check must leave the analysis incomplete"
        );
        assert!(
            report
                .coverage
                .failures
                .iter()
                .any(|failure| failure.contains("upstream")),
            "the failure must name the package, got {:?}",
            report.coverage.failures
        );
        let names = packages(&report);
        assert!(
            names.iter().any(|name| name == "upstream"),
            "resolved-version findings must survive the latest-check failure, got {names:?}"
        );
    }

    /// Create the named pipe that holds an extraction inside archive I/O.
    #[cfg(unix)]
    fn hold_fifo(dir: &Path, name: &str) -> PathBuf {
        let fifo = dir.join(name);
        let status = std::process::Command::new("mkfifo")
            .arg(&fifo)
            .status()
            .unwrap();
        assert!(status.success(), "mkfifo for {name}");
        fifo
    }

    /// A held archive stream still delivers a complete source.
    #[test]
    #[cfg(unix)]
    fn held_archive_stream_delivers_complete_source() {
        const LIB: &str = "pub fn streamed(input: u32) -> u32 { input + 13u32 }\n";
        const TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("upstream-1.0.0");
        std::fs::create_dir_all(dest.parent().unwrap()).unwrap();
        let stream = crate_archive("upstream", "1.0.0", LIB);
        let fifo = hold_fifo(dir.path(), "stream.fifo");
        let id = "djd.archive.held-stream";
        let (tx, rx) = std::sync::mpsc::sync_channel(1);
        let (dest_thread, fifo_thread) = (dest.clone(), fifo.clone());
        std::thread::spawn(move || {
            let outcome = extract_archive(&fifo_thread, &dest_thread, ArchiveKind::Crate);
            tx.send((id, outcome)).unwrap();
        });
        let mut writer = std::fs::OpenOptions::new().write(true).open(&fifo).unwrap();
        // Split delivery proves the held stream reassembles into a full archive.
        writer.write_all(&stream[..16]).unwrap();
        writer.write_all(&stream[16..]).unwrap();
        drop(writer);
        let (got_id, outcome) = rx.recv_timeout(TIMEOUT).unwrap();
        assert_eq!(got_id, id);
        outcome.expect("a held stream must still extract");
        assert_eq!(
            std::fs::read_to_string(dest.join("src/lib.rs")).unwrap(),
            LIB
        );
    }

    /// A later extraction accepts the first published source.
    #[test]
    #[cfg(unix)]
    fn late_extraction_does_not_replace_published_source() {
        const PUBLISHED: &str = "pub fn published(input: u32) -> u32 { input + 11u32 }\n";
        const LATE: &str = "pub fn late(input: u32) -> u32 { input + 12u32 }\n";
        const TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("upstream-1.0.0");
        std::fs::create_dir_all(dest.parent().unwrap()).unwrap();
        let published_path = dir.path().join("published.crate");
        let published_bytes = crate_archive("upstream", "1.0.0", PUBLISHED);
        std::fs::write(&published_path, published_bytes).unwrap();
        let late_stream = crate_archive("upstream", "1.0.0", LATE);
        let fifo = hold_fifo(dir.path(), "late.fifo");
        let id = "djd.archive.late-extraction";
        let (tx, rx) = std::sync::mpsc::sync_channel(1);
        let (dest_thread, fifo_thread) = (dest.clone(), fifo.clone());
        std::thread::spawn(move || {
            let outcome = extract_archive(&fifo_thread, &dest_thread, ArchiveKind::Crate);
            tx.send((id, outcome)).unwrap();
        });
        let mut writer = std::fs::OpenOptions::new().write(true).open(&fifo).unwrap();
        // The late extraction blocks inside archive I/O on these bytes while the writer holds the rest.
        writer.write_all(&late_stream[..16]).unwrap();
        extract_archive(&published_path, &dest, ArchiveKind::Crate)
            .expect("the first extraction must publish");
        writer.write_all(&late_stream[16..]).unwrap();
        drop(writer);
        let (got_id, outcome) = rx.recv_timeout(TIMEOUT).unwrap();
        assert_eq!(got_id, id);
        outcome.expect("a late extraction must accept the published source");
        assert!(dest.is_dir());
        assert_eq!(
            std::fs::read_to_string(dest.join("src/lib.rs")).unwrap(),
            PUBLISHED,
            "a late extraction must not replace the first published source"
        );
    }
    /// Files for a local app crate rooted at `ws/app`.
    fn app_files(ws: &Path, lib: &str) {
        put(
            ws,
            "app/Cargo.toml",
            "[package]\nname = \"app\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
        );
        put(ws, "app/src/lib.rs", lib);
    }

    /// Files for one dependency crate rooted at `ws/name`.
    fn pkg_files(ws: &Path, name: &str, lib: &str) {
        put(
            ws,
            &format!("{name}/Cargo.toml"),
            &format!("[package]\nname = \"{name}\"\nversion = \"0.1.0\"\nedition = \"2024\"\n"),
        );
        put(ws, &format!("{name}/src/lib.rs"), lib);
    }

    /// A crafted metadata package entry rooted at `ws/name`.
    fn json_pkg(
        name: &str,
        version: &str,
        ws: &Path,
        source: Option<&str>,
        kind: &str,
        deps: &[serde_json::Value],
    ) -> serde_json::Value {
        let id = source.map_or_else(
            || format!("path+file://{}#{}", ws.join(name).display(), version),
            |source| format!("{source}#{name}/{version}"),
        );
        serde_json::json!({
            "name": name,
            "version": version,
            "id": id,
            "source": source,
            "dependencies": deps,
            "targets": [
                {
                    "name": name,
                    "kind": [kind],
                    "crate_types": [kind],
                    "src_path": format!("{}/src/lib.rs", ws.join(name).display()),
                }
            ],
            "manifest_path": format!("{}/Cargo.toml", ws.join(name).display()),
            "features": {}
        })
    }

    /// A crafted manifest dependency entry.
    fn json_dep(
        name: &str,
        source: Option<&str>,
        req: &str,
        optional: bool,
        target: Option<&str>,
        path: Option<&str>,
    ) -> serde_json::Value {
        serde_json::json!({
            "name": name,
            "source": source,
            "req": req,
            "kind": null,
            "optional": optional,
            "uses_default_features": true,
            "features": [],
            "target": target,
            "rename": null,
            "registry": null,
            "path": path
        })
    }

    /// A crafted metadata document for an app with the given edges and packages.
    fn crafted_meta(
        ws: &Path,
        app_deps: &[serde_json::Value],
        packages: &[serde_json::Value],
        resolve: Option<serde_json::Value>,
    ) -> serde_json::Value {
        let app_id = format!("path+file://{}#0.1.0", ws.display());
        let mut all: Vec<serde_json::Value> = Vec::new();
        all.push(serde_json::json!({
            "name": "app",
            "version": "0.1.0",
            "id": app_id,
            "source": null,
            "dependencies": app_deps,
            "targets": [
                {
                    "name": "app",
                    "kind": ["lib"],
                    "crate_types": ["lib"],
                    "src_path": format!("{}/app/src/lib.rs", ws.display()),
                }
            ],
            "manifest_path": format!("{}/app/Cargo.toml", ws.display()),
            "features": {}
        }));
        all.extend_from_slice(packages);
        serde_json::json!({
            "packages": all,
            "workspace_members": [app_id],
            "resolve": resolve.unwrap_or(serde_json::json!({ "nodes": [] })),
            "workspace_root": ws.to_string_lossy(),
            "target_directory": format!("{}/target", ws.display()),
            "metadata": null,
            "version": 1
        })
    }

    /// A crafted metadata run, bypassing cargo.
    fn crafted_run(
        ws: &Path,
        cache: &Path,
        base: Option<&str>,
        options: &DependencySearch,
        json: serde_json::Value,
    ) -> ReuseReport {
        let local = local_records(ws, "app", &[("app/src/lib.rs", vec![])]);
        let refs: Vec<&IndexedContext> = local.iter().collect();
        let metadata: Metadata = serde_json::from_value(json).unwrap();
        let graph = build_graph(&metadata, &BTreeMap::new());
        let store = Store::open(cache).unwrap();
        let eff = effective(options);
        let mut d = Discovery {
            graph,
            index_fingerprint: index_fingerprint(&eff),
            eff,
            store: Some(store),
            store_root: cache.to_path_buf(),
            registry: match base {
                Some(base) => Registry {
                    api: format!("{base}/api/v1/crates"),
                    cdn: format!("{base}/crates"),
                },
                None => Registry::default(),
            },
            coverage: DependencyCoverage::default(),
            effective_targets: Vec::new(),
            cfgs: BTreeMap::new(),
            latest_known: BTreeMap::new(),
        };
        let targets = single_lib_targets(ws);
        analyze_discovery(&mut d, ws, &targets, &refs, 0)
    }

    /// One run of crafted discovery, all-optional at depth `depth`.
    fn crafted(
        ws: &Path,
        cache: &Path,
        base: Option<&str>,
        depth: usize,
        json: serde_json::Value,
    ) -> ReuseReport {
        let options = DependencySearch {
            depth: Some(depth),
            all_optional: Some(true),
            check_latest: Some(false),
            ..DependencySearch::default()
        };
        crafted_run(ws, cache, base, &options, json)
    }

    /// A discovery state without a store, rooted at `dir`.
    fn discovery(dir: &Path) -> Discovery {
        Discovery {
            graph: Graph {
                packages: BTreeMap::new(),
                configured: BTreeMap::new(),
                src_to_member: BTreeMap::new(),
            },
            index_fingerprint: String::new(),
            eff: effective(&DependencySearch::default()),
            store: None,
            store_root: dir.to_path_buf(),
            registry: Registry::default(),
            coverage: DependencyCoverage::default(),
            effective_targets: Vec::new(),
            cfgs: BTreeMap::new(),
            latest_known: BTreeMap::new(),
        }
    }

    #[test]
    fn zero_depth_skips_discovery_entirely() {
        let dir = tempfile::tempdir().unwrap();
        let options = DependencySearch {
            depth: None,
            check_latest: Some(false),
            cache_dir: Some(dir.path().join("index").to_string_lossy().into_owned()),
            fail_on_findings: Some(true),
            ..DependencySearch::default()
        };
        let report = analyze(dir.path(), &[], &[], &options, 0, false, &[]);
        assert!(report.groups.is_empty(), "{:?}", report.groups);
        assert!(report.coverage.complete, "{:?}", report.coverage);
        assert!(report.coverage.failures.is_empty(), "{:?}", report.coverage);
        assert!(report.fail_on_findings);
    }

    #[test]
    fn an_unknown_dependency_kind_is_not_followed() {
        let dir = tempfile::tempdir().unwrap();
        let ws = dir.path().join("ws");
        let cache = dir.path().join("index");
        app_files(
            &ws,
            &format!("pub fn local_match(value: u32) -> u32 {{ {MATCHING_BODY} }}\n"),
        );
        pkg_files(
            &ws,
            "kept",
            &format!("pub fn kept_fn(value: u32) -> u32 {{ {MATCHING_BODY} }}\n"),
        );
        pkg_files(
            &ws,
            "dropped",
            &format!("pub fn dropped_fn(value: u32) -> u32 {{ {MATCHING_BODY} }}\n"),
        );
        let app_id = format!("path+file://{}#0.1.0", ws.display());
        let json = crafted_meta(
            &ws,
            &[],
            &[
                json_pkg("kept", "0.1.0", &ws, None, "lib", &[]),
                json_pkg("dropped", "0.1.0", &ws, None, "lib", &[]),
            ],
            Some(serde_json::json!({
                "nodes": [
                    {
                        "id": app_id,
                        "deps": [
                            { "name": "kept", "pkg": format!("path+file://{}#0.1.0", ws.join("kept").display()), "dep_kinds": [{ "kind": "normal" }] },
                            { "name": "dropped", "pkg": format!("path+file://{}#0.1.0", ws.join("dropped").display()), "dep_kinds": [{ "kind": "proc" }] }
                        ],
                        "dependencies": [],
                        "features": []
                    }
                ]
            })),
        );
        let report = crafted(&ws, &cache, None, 1, json);
        assert_eq!(packages(&report), vec!["kept".to_owned()]);
        assert!(report.coverage.complete, "{:?}", report.coverage);
    }

    #[test]
    fn a_graph_without_a_resolve_section_follows_manifest_edges() {
        let dir = tempfile::tempdir().unwrap();
        let ws = dir.path().join("ws");
        let cache = dir.path().join("index");
        app_files(
            &ws,
            &format!("pub fn local_match(value: u32) -> u32 {{ {MATCHING_BODY} }}\n"),
        );
        pkg_files(
            &ws,
            "side",
            &format!("pub fn side_fn(value: u32) -> u32 {{ {MATCHING_BODY} }}\n"),
        );
        let json = crafted_meta(
            &ws,
            &[json_dep(
                "side",
                None,
                "*",
                true,
                None,
                Some(&ws.join("side").to_string_lossy()),
            )],
            &[json_pkg("side", "0.1.0", &ws, None, "lib", &[])],
            None,
        );
        let report = crafted(&ws, &cache, None, 1, json);
        assert_eq!(packages(&report), vec!["side".to_owned()]);
        assert!(report.coverage.complete, "{:?}", report.coverage);
    }

    #[test]
    fn a_null_resolve_section_is_treated_as_absent() {
        let dir = tempfile::tempdir().unwrap();
        let ws = dir.path().join("ws");
        let cache = dir.path().join("index");
        app_files(
            &ws,
            &format!("pub fn local_match(value: u32) -> u32 {{ {MATCHING_BODY} }}\n"),
        );
        let mut json = crafted_meta(&ws, &[], &[], None);
        json["resolve"] = serde_json::Value::Null;
        let report = crafted(&ws, &cache, None, 1, json);
        assert!(report.coverage.complete, "{:?}", report.coverage);
        assert!(report.groups.is_empty(), "{:?}", report.groups);
    }

    #[test]
    fn an_edge_to_an_unknown_package_is_skipped() {
        let dir = tempfile::tempdir().unwrap();
        let ws = dir.path().join("ws");
        let cache = dir.path().join("index");
        app_files(
            &ws,
            &format!("pub fn local_match(value: u32) -> u32 {{ {MATCHING_BODY} }}\n"),
        );
        pkg_files(
            &ws,
            "real",
            &format!("pub fn real_fn(value: u32) -> u32 {{ {MATCHING_BODY} }}\n"),
        );
        let app_id = format!("path+file://{}#0.1.0", ws.display());
        let json = crafted_meta(
            &ws,
            &[json_dep(
                "real",
                None,
                "*",
                true,
                None,
                Some(&ws.join("real").to_string_lossy()),
            )],
            &[json_pkg("real", "0.1.0", &ws, None, "lib", &[])],
            Some(serde_json::json!({
                "nodes": [
                    {
                        "id": app_id,
                        "deps": [
                            { "name": "ghost", "pkg": format!("path+file://{}#9.9.9", ws.join("ghost").display()), "dep_kinds": [{ "kind": "normal" }] }
                        ],
                        "dependencies": [],
                        "features": []
                    }
                ]
            })),
        );
        let report = crafted(&ws, &cache, None, 1, json);
        assert_eq!(packages(&report), vec!["real".to_owned()]);
        assert!(report.coverage.complete, "{:?}", report.coverage);
    }

    /// The member and path packages the unresolved-graph test resolves from disk.
    fn unresolved_fixture(ws: &Path) {
        app_files(
            ws,
            &format!("pub fn local_match(value: u32) -> u32 {{ {MATCHING_BODY} }}\n"),
        );
        for name in ["side", "inner", "weird", "dup"] {
            pkg_files(
                ws,
                name,
                &format!("pub fn {name}_fn(value: u32) -> u32 {{ {MATCHING_BODY} }}\n"),
            );
        }
        pkg_files(
            ws,
            "inner2",
            "pub fn inner2_fn(value: u32) -> u32 { value + 1u32 }\n",
        );
        put(
            ws,
            "badreq/Cargo.toml",
            "[package]\nname = \"badreq\"\nversion = \"0.1.0\"\nedition = \"2024\"\n[dependencies]\nweird = { version = \"not-semver\" }\n",
        );
        put(
            ws,
            "badreq/src/lib.rs",
            "pub fn badreq_fn(value: u32) -> u32 { value + 1u32 }\n",
        );
    }

    #[test]
    fn unresolved_packages_follow_their_manifest_edges() {
        let dir = tempfile::tempdir().unwrap();
        let ws = dir.path().join("ws");
        let cache = dir.path().join("index");
        unresolved_fixture(&ws);
        let routes = BTreeMap::new();
        let server = HttpServer::start(routes);
        let side_deps = [
            json_dep("inner", None, "^1.0", false, None, None),
            json_dep("inner2", None, "^9.0", false, None, None),
        ];
        let side = json_pkg("side", "0.1.0", &ws, None, "lib", &side_deps);
        let mut pm = json_pkg("pm", "0.1.0", &ws, None, "proc-macro", &[]);
        pm["targets"][0]["src_path"] =
            serde_json::Value::String(format!("{}/pm-else/src/lib.rs", ws.display()));
        let json = crafted_meta(
            &ws,
            &[
                json_dep(
                    "side",
                    None,
                    "*",
                    true,
                    None,
                    Some(&ws.join("side").to_string_lossy()),
                ),
                json_dep(
                    "badreq",
                    None,
                    "*",
                    true,
                    None,
                    Some(&ws.join("badreq").to_string_lossy()),
                ),
                json_dep("dup", None, "*", true, None, None),
            ],
            &[
                side,
                json_pkg("inner", "1.0.0", &ws, None, "lib", &[]),
                json_pkg("inner2", "1.0.0", &ws, None, "lib", &[]),
                json_pkg("weird", "1.0.0", &ws, None, "lib", &[]),
                json_pkg("dup", "1.0.0", &ws, None, "lib", &[]),
                json_pkg("dup", "1.5.0", &ws, None, "lib", &[]),
                json_pkg("dup", "2.0.0", &ws, None, "lib", &[]),
                json_pkg("dup", "10.0.0", &ws, None, "lib", &[]),
                pm,
            ],
            Some(serde_json::json!({
                "nodes": [
                    {
                        "id": format!("path+file://{}#0.1.0", ws.display()),
                        "deps": [],
                        "dependencies": [],
                        "features": []
                    }
                ]
            })),
        );
        let report = crafted(&ws, &cache, Some(&server.base), 2, json);
        server.stop();
        assert_eq!(
            packages(&report),
            vec![
                "dup".to_owned(),
                "inner".to_owned(),
                "side".to_owned(),
                "weird".to_owned()
            ]
        );
        let dup = report
            .groups
            .iter()
            .flat_map(|group| group.candidates.iter())
            .find(|candidate| candidate.package == "dup")
            .expect("the dup candidate is present");
        assert_eq!(dup.version, "10.0.0");
        let failures = report.coverage.failures.join("\n");
        assert!(
            failures.contains("version of inner2 could not be resolved"),
            "{failures}"
        );
        assert!(!report.coverage.complete, "{:?}", report.coverage);
    }

    #[test]
    fn platform_matches_allows_targets_without_effective_targets() {
        let mut d = discovery(Path::new("/tmp"));
        let named = ManifestDep {
            name: "x".to_owned(),
            source: None,
            req: "*".to_owned(),
            kind: DependencyKind::Normal,
            optional: true,
            target: Some(Platform::Name("x86_64-unknown-linux-gnu".to_owned())),
            path: None,
        };
        assert!(d.platform_matches(&named));
        let plain = ManifestDep {
            name: "y".to_owned(),
            source: None,
            req: "*".to_owned(),
            kind: DependencyKind::Normal,
            optional: false,
            target: None,
            path: None,
        };
        assert!(d.platform_matches(&plain));
    }

    #[test]
    fn manifest_library_info_requires_a_library_source() {
        let dir = tempfile::tempdir().unwrap();
        put(dir.path(), "src/lib.rs", "pub fn f() {}\n");
        let value: toml::Value =
            toml::from_str("[package]\nname = \"p\"\nversion = \"0.1.0\"\n").unwrap();
        assert_eq!(
            manifest_library_info(&value, dir.path()),
            Some((String::from("src/lib.rs"), None, true))
        );
        let value: toml::Value = toml::from_str("[lib]\npath = \"src/absent.rs\"\n").unwrap();
        assert_eq!(manifest_library_info(&value, dir.path()), None);
        let value: toml::Value = toml::from_str("[lib]\nproc-macro = true\n").unwrap();
        assert_eq!(
            manifest_library_info(&value, dir.path()),
            Some((String::from("src/lib.rs"), None, false))
        );
        let value: toml::Value = toml::from_str("[lib]\ncrate-type = [\"cdylib\"]\n").unwrap();
        assert_eq!(
            manifest_library_info(&value, dir.path()),
            Some((String::from("src/lib.rs"), None, false))
        );
        let value: toml::Value = toml::from_str("[lib]\ncrate-type = [\"dylib\"]\n").unwrap();
        assert_eq!(
            manifest_library_info(&value, dir.path()),
            Some((String::from("src/lib.rs"), None, true))
        );
        let value: toml::Value = toml::from_str("[lib]\nname = \"renamed\"\n").unwrap();
        assert_eq!(
            manifest_library_info(&value, dir.path()),
            Some((String::from("src/lib.rs"), Some("renamed".to_owned()), true))
        );
    }

    #[test]
    fn manifest_deps_parse_every_table_shape() {
        let dir = tempfile::tempdir().unwrap();
        let value: toml::Value = toml::from_str(
            r#"
[dependencies]
alpha = "1.0"
beta = { version = "2", optional = true }
gamma = { git = "https://example.com/gamma", version = "3" }
delta = { registry = "https://example.com/idx" }
epsilon = { path = "rel/epsilon" }
zeta = { path = "/abs/zeta" }
ignored = 5
[dev-dependencies]
eta = "0.1"
[build-dependencies]
theta = "0.2"
[target]
stray = "oops"
[target.'cfg(unix)'.dependencies]
iota = { version = "4", optional = true }
"#,
        )
        .unwrap();
        let deps = manifest_deps_from_toml(&value, dir.path());
        assert_eq!(deps.len(), 9);
        let by_name: BTreeMap<&str, &ManifestDep> =
            deps.iter().map(|dep| (dep.name.as_str(), dep)).collect();
        let alpha = by_name["alpha"];
        assert_eq!(alpha.req, "1.0");
        assert!(!alpha.optional);
        assert_eq!(alpha.kind, DependencyKind::Normal);
        assert!(alpha.source.is_none());
        assert!(alpha.path.is_none());
        assert!(alpha.target.is_none());
        assert!(by_name["beta"].optional);
        assert_eq!(by_name["beta"].req, "2");
        assert_eq!(
            by_name["gamma"].source.as_deref(),
            Some("git+https://example.com/gamma")
        );
        assert_eq!(
            by_name["delta"].source.as_deref(),
            Some("registry+https://example.com/idx")
        );
        assert_eq!(
            by_name["epsilon"].path.as_deref(),
            Some(dir.path().join("rel/epsilon").as_path())
        );
        assert_eq!(
            by_name["zeta"].path.as_deref(),
            Some(Path::new("/abs/zeta"))
        );
        assert_eq!(by_name["eta"].kind, DependencyKind::Development);
        assert_eq!(by_name["theta"].kind, DependencyKind::Build);
        assert!(matches!(by_name["iota"].target, Some(Platform::Cfg(_))));
        assert!(by_name["iota"].optional);
    }

    #[test]
    fn relative_keeps_paths_outside_the_root() {
        assert_eq!(
            relative("/other/file.rs", Path::new("/root")),
            "/other/file.rs"
        );
        assert_eq!(relative("/root/a/b.rs", Path::new("/root")), "a/b.rs");
    }

    #[test]
    fn oversized_subprocess_output_is_rejected() {
        let id = next_proc_id();
        let args = ["-c", "head -c 17000000 /dev/zero"].map(str::to_owned);
        let error = run_bounded(
            std::process::Command::new("sh").args(&args),
            std::time::Duration::from_secs(30),
            &id,
        )
        .expect_err("oversized output must be rejected");
        assert!(matches!(error, ProcessError::OutputLimit { .. }));
    }

    #[test]
    fn output_at_the_limit_is_kept() {
        let id = next_proc_id();
        let args = ["-c".to_owned(), format!("head -c {OUTPUT_LIMIT} /dev/zero")];
        let output = run_bounded(
            std::process::Command::new("sh").args(&args),
            std::time::Duration::from_secs(30),
            &id,
        )
        .expect("output at the limit is kept");
        assert_eq!(u64::try_from(output.stdout.len()).unwrap(), OUTPUT_LIMIT);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn a_capture_file_the_child_removes_or_replaces_is_a_read_failure() {
        for script in [
            r#"rm "$(readlink /proc/$$/fd/1)""#,
            r#"out="$(readlink /proc/$$/fd/1)"; rm "$out"; mkdir "$out""#,
        ] {
            let id = next_proc_id();
            let args = ["-c", script].map(str::to_owned);
            let error = run_bounded(
                std::process::Command::new("sh").args(&args),
                std::time::Duration::from_secs(30),
                &id,
            )
            .expect_err(script);
            assert!(
                matches!(
                    &error,
                    ProcessError::Io {
                        operation: "read output",
                        ..
                    }
                ),
                "{script}: {error}"
            );
        }
    }

    #[test]
    fn a_live_child_is_not_reaped_within_a_short_window() {
        let mut child = std::process::Command::new("sleep")
            .arg("5")
            .spawn()
            .unwrap();
        assert!(!reap_bounded(
            &mut child,
            std::time::Duration::from_millis(100)
        ));
        child.kill().unwrap();
        let _ = child.wait();
    }

    #[test]
    fn a_breached_deadline_is_reported_with_the_process_killed() {
        let id = next_proc_id();
        let args = ["30".to_owned()];
        let error = run_bounded(
            std::process::Command::new("sleep").args(&args),
            std::time::Duration::from_millis(100),
            &id,
        )
        .expect_err("a breached deadline must be reported");
        assert!(matches!(error, ProcessError::Deadline { .. }));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn a_breached_deadline_kills_the_descendants_of_the_process() {
        let dir = tempfile::tempdir().unwrap();
        let pid_file = dir.path().join("pid");
        let args = [
            "-c".to_owned(),
            "sleep 30 & echo $! > \"$1\"; wait".to_owned(),
            "sh".to_owned(),
            pid_file.to_string_lossy().into_owned(),
        ];
        let id = next_proc_id();
        let error = run_bounded(
            std::process::Command::new("sh").args(&args),
            std::time::Duration::from_millis(500),
            &id,
        )
        .expect_err("a breached deadline must be reported");
        assert!(matches!(error, ProcessError::Deadline { .. }));
        let pid = std::fs::read_to_string(&pid_file).unwrap();
        let stat = format!("/proc/{}/stat", pid.trim());
        let gone = (0..250).any(|_| {
            std::thread::sleep(std::time::Duration::from_millis(20));
            !std::fs::read_to_string(&stat).is_ok_and(|line| !line.contains(") Z "))
        });
        assert!(gone, "grandchild {} survived the group kill", pid.trim());
    }

    #[cfg(unix)]
    #[test]
    fn only_child_process_groups_are_signalable() {
        for pid in [0, 1, u32::MAX] {
            assert!(signalable_group(pid).is_none(), "{pid}");
        }
        let group = signalable_group(2).expect("group 2 is signalable");
        assert_eq!(group.as_raw_nonzero().get(), 2);
    }

    #[test]
    fn acquire_reports_missing_store_layout_and_bad_archives() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("public")).unwrap();
        std::fs::write(dir.path().join("public/crates.io"), "a file").unwrap();
        let d = discovery(dir.path());
        let error = d.acquire_crates_io_source("n", "1.0.0").unwrap_err();
        assert!(error.contains("is unavailable"), "{error}");
        let dir = tempfile::tempdir().unwrap();
        let archives = dir.path().join("public/crates.io/archives");
        std::fs::create_dir_all(&archives).unwrap();
        std::fs::write(archives.join("n-2.0.0.crate"), "not a crate").unwrap();
        let d = discovery(dir.path());
        let error = d.acquire_crates_io_source("n", "2.0.0").unwrap_err();
        assert!(error.contains("could not be extracted"), "{error}");
    }

    #[cfg(unix)]
    #[test]
    fn serve_one_ends_on_closed_and_idle_peers() {
        use std::io::Write as _;
        let routes = BTreeMap::new();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let mut client = std::net::TcpStream::connect(addr).unwrap();
        client.write_all(b"GET /x HTTP/1.1\r\n").unwrap();
        let (stream, _) = listener.accept().unwrap();
        drop(client);
        serve_one(stream, &routes);
        let client = std::net::TcpStream::connect(addr).unwrap();
        let (stream, _) = listener.accept().unwrap();
        serve_one(stream, &routes);
        drop(client);
    }

    #[test]
    fn a_corrupt_index_entry_is_rebuilt_with_a_warning() {
        let dir = tempfile::tempdir().unwrap();
        let cache = dir.path().join("index");
        fixture(dir.path());
        let manifest_dir = std::fs::canonicalize(dir.path().join("deps/direct")).unwrap();
        let key = source_key("path", &manifest_dir.to_string_lossy());
        let entry = cache.join("private").join(&key).join("direct.json");
        std::fs::create_dir_all(entry.parent().unwrap()).unwrap();
        std::fs::write(&entry, "not json").unwrap();
        let local = app_records(dir.path());
        let targets = app_targets(dir.path());
        let report = run(
            dir.path(),
            &local,
            &targets,
            &app_options(dir.path(), &cache, 1),
        );
        let warnings = report.coverage.warnings.join("\n");
        assert!(
            warnings.contains("dependency index for direct is corrupt, rebuilt"),
            "{warnings}"
        );
        assert_eq!(packages(&report), vec!["direct".to_owned()]);
        let rebuilt: serde_json::Value =
            serde_json::from_reader(std::fs::File::open(&entry).unwrap()).unwrap();
        assert_eq!(
            rebuilt.get("schema").and_then(serde_json::Value::as_u64),
            Some(u64::from(cache::SCHEMA))
        );
    }

    #[test]
    fn an_unwritable_store_keeps_the_findings_and_warns() {
        let dir = tempfile::tempdir().unwrap();
        let cache = dir.path().join("index");
        fixture(dir.path());
        std::fs::create_dir_all(&cache).unwrap();
        std::fs::write(cache.join("private"), "blocked").unwrap();
        let local = app_records(dir.path());
        let targets = app_targets(dir.path());
        let report = run(
            dir.path(),
            &local,
            &targets,
            &app_options(dir.path(), &cache, 1),
        );
        let warnings = report.coverage.warnings.join("\n");
        assert!(
            warnings.contains("dependency index for direct is unavailable"),
            "{warnings}"
        );
        assert!(
            warnings.contains("dependency index for direct could not be saved"),
            "{warnings}"
        );
        assert_eq!(packages(&report), vec!["direct".to_owned()]);
    }

    #[test]
    fn an_unopenable_index_dir_still_reports_findings() {
        let dir = tempfile::tempdir().unwrap();
        let cache = dir.path().join("index");
        fixture(dir.path());
        std::fs::write(&cache, "a file").unwrap();
        let local = app_records(dir.path());
        let targets = app_targets(dir.path());
        let report = run(
            dir.path(),
            &local,
            &targets,
            &app_options(dir.path(), &cache, 1),
        );
        assert_eq!(packages(&report), vec!["direct".to_owned()]);
        assert!(report.coverage.complete, "{:?}", report.coverage);
        assert_eq!(std::fs::read_to_string(&cache).unwrap(), "a file");
    }

    #[test]
    fn an_unlocated_selected_crate_leaves_the_analysis_incomplete() {
        let dir = tempfile::tempdir().unwrap();
        let cache = dir.path().join("index");
        fixture(dir.path());
        put(
            dir.path(),
            "stray/src/lib.rs",
            &format!("pub fn stray_fn(value: u32) -> u32 {{ {MATCHING_BODY} }}\n"),
        );
        let local = local_records(dir.path(), "stray", &[("stray/src/lib.rs", vec![])]);
        let file = dir.path().join("stray/src/lib.rs");
        let text = std::fs::read_to_string(&file).unwrap();
        let targets = vec![TargetScan {
            name: "stray".to_owned(),
            files: vec![SourceFile {
                path: file.to_string_lossy().into_owned(),
                segments: Vec::new(),
                cfg: Vec::new(),
                parsed: syn::parse_file(&text).unwrap(),
                text,
                rustdoc: true,
            }],
            library: true,
        }];
        let options = app_options(dir.path(), &cache, 1);
        let report = run(dir.path(), &local, &targets, &options);
        let failures = report.coverage.failures.join("\n");
        assert!(
            failures.contains(
                "selected crate stray could not be located in the workspace metadata (target stray)"
            ),
            "{failures}"
        );
        assert!(!report.coverage.complete, "{:?}", report.coverage);
    }

    #[test]
    fn cfg_gated_optional_dependencies_follow_the_host_conditions() {
        let dir = tempfile::tempdir().unwrap();
        let cache = dir.path().join("index");
        fixture(dir.path());
        let root = dir.path().join("Cargo.toml");
        let mut root_manifest = std::fs::read_to_string(&root).unwrap();
        root_manifest = root_manifest.replacen(
            "\"deps/winonly\", \"deps/winopt\"]",
            "\"deps/winonly\", \"deps/winopt\", \"deps/cfgdep\", \"deps/cfgdep2\"]",
            1,
        );
        std::fs::write(&root, root_manifest).unwrap();
        put(
            dir.path(),
            "deps/cfgdep/Cargo.toml",
            "[package]\nname = \"cfgdep\"\nversion = \"0.1.0\"\nedition = \"2024\"\n[workspace]\n",
        );
        put(
            dir.path(),
            "deps/cfgdep/src/lib.rs",
            &format!("pub fn helper(value: u32) -> u32 {{ {MATCHING_BODY} }}\n"),
        );
        put(
            dir.path(),
            "deps/cfgdep2/Cargo.toml",
            "[package]\nname = \"cfgdep2\"\nversion = \"0.1.0\"\nedition = \"2024\"\n[workspace]\n",
        );
        put(
            dir.path(),
            "deps/cfgdep2/src/lib.rs",
            &format!("pub fn helper(value: u32) -> u32 {{ {MATCHING_BODY} }}\n"),
        );
        let path = dir.path().join("app/Cargo.toml");
        let mut manifest = std::fs::read_to_string(&path).unwrap();
        manifest.push_str(
            "[target.'cfg(unix)'.dependencies]\ncfgdep = { path = \"../deps/cfgdep\", optional = true }\ncfgdep2 = { path = \"../deps/cfgdep2\", optional = true }\n",
        );
        std::fs::write(&path, manifest).unwrap();
        let local = app_records(dir.path());
        let targets = app_targets(dir.path());
        let mut options = app_options(dir.path(), &cache, 2);
        options.all_optional = Some(true);
        let report = run(dir.path(), &local, &targets, &options);
        assert_eq!(
            packages(&report),
            vec![
                "cfgdep".to_owned(),
                "cfgdep2".to_owned(),
                "direct".to_owned(),
                "opt2".to_owned(),
                "optional".to_owned(),
                "transitive".to_owned()
            ]
        );
    }

    #[test]
    fn unparseable_dependency_sources_are_reported_with_their_modules() {
        let dir = tempfile::tempdir().unwrap();
        let cache = dir.path().join("index");
        fixture(dir.path());
        put(
            dir.path(),
            "deps/badsrc/Cargo.toml",
            "[package]\nname = \"badsrc\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
        );
        put(dir.path(), "deps/badsrc/src/lib.rs", "fn broken( {\n");
        put(
            dir.path(),
            "deps/badsub/Cargo.toml",
            "[package]\nname = \"badsub\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
        );
        put(
            dir.path(),
            "deps/badsub/src/lib.rs",
            &format!("mod sub;\npub fn helper(value: u32) -> u32 {{ {MATCHING_BODY} }}\n"),
        );
        put(dir.path(), "deps/badsub/src/sub.rs", "not rust at all\n");
        let path = dir.path().join("app/Cargo.toml");
        let mut app: toml::Value =
            toml::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        let deps = app["dependencies"].as_table_mut().unwrap();
        deps.insert(
            "badsrc".to_owned(),
            toml::Value::try_from(serde_json::json!({ "path": "../deps/badsrc" })).unwrap(),
        );
        deps.insert(
            "badsub".to_owned(),
            toml::Value::try_from(serde_json::json!({ "path": "../deps/badsub" })).unwrap(),
        );
        std::fs::write(&path, toml::to_string(&app).unwrap()).unwrap();
        let local = app_records(dir.path());
        let targets = app_targets(dir.path());
        let report = run(
            dir.path(),
            &local,
            &targets,
            &app_options(dir.path(), &cache, 1),
        );
        let failures = report.coverage.failures.join("\n");
        assert!(
            failures.contains("dependency source for badsrc"),
            "{failures}"
        );
        assert!(failures.contains("in sub"), "{failures}");
        assert_eq!(
            packages(&report),
            vec!["badsub".to_owned(), "direct".to_owned()]
        );
        assert!(!report.coverage.complete, "{:?}", report.coverage);
    }

    #[test]
    fn unavailable_sources_are_reported_with_their_identity() {
        let dir = tempfile::tempdir().unwrap();
        let ws = dir.path().join("ws");
        let cache = dir.path().join("index");
        app_files(
            &ws,
            &format!("pub fn local_match(value: u32) -> u32 {{ {MATCHING_BODY} }}\n"),
        );
        let json = crafted_meta(
            &ws,
            &[
                json_dep(
                    "privreg",
                    Some("registry+https://example.com/idx"),
                    "*",
                    true,
                    None,
                    None,
                ),
                json_dep(
                    "gitdep",
                    Some("git+https://example.com/repo"),
                    "*",
                    true,
                    None,
                    None,
                ),
            ],
            &[],
            Some(serde_json::json!({
                "nodes": [
                    { "id": format!("path+file://{}#0.1.0", ws.display()), "deps": [], "dependencies": [], "features": [] }
                ]
            })),
        );
        let report = crafted(&ws, &cache, None, 1, json);
        let failures = report.coverage.failures.join("\n");
        assert!(
            failures.contains("source for privreg is unavailable offline (registry registry+https://example.com/idx)"),
            "{failures}"
        );
        assert!(
            failures.contains(
                "source for gitdep is unavailable offline (source git+https://example.com/repo)"
            ),
            "{failures}"
        );
        assert!(report.groups.is_empty(), "{:?}", report.groups);
        assert!(!report.coverage.complete, "{:?}", report.coverage);
    }

    #[test]
    fn broken_path_manifests_are_reported_per_dependency() {
        let dir = tempfile::tempdir().unwrap();
        let ws = dir.path().join("ws");
        let cache = dir.path().join("index");
        app_files(
            &ws,
            &format!("pub fn local_match(value: u32) -> u32 {{ {MATCHING_BODY} }}\n"),
        );
        put(&ws, "ghost2/Cargo.toml", "this is not toml");
        put(&ws, "ghost3/Cargo.toml", "[workspace]\n");
        put(&ws, "ghost4/Cargo.toml", "[package]\nname = \"other\"\n");
        pkg_files(
            &ws,
            "good",
            &format!("pub fn good_fn(value: u32) -> u32 {{ {MATCHING_BODY} }}\n"),
        );
        let json = crafted_meta(
            &ws,
            &[
                json_dep(
                    "bad1",
                    None,
                    "*",
                    true,
                    None,
                    Some(&ws.join("ghost1").to_string_lossy()),
                ),
                json_dep(
                    "bad2",
                    None,
                    "*",
                    true,
                    None,
                    Some(&ws.join("ghost2").to_string_lossy()),
                ),
                json_dep(
                    "bad3",
                    None,
                    "*",
                    true,
                    None,
                    Some(&ws.join("ghost3").to_string_lossy()),
                ),
                json_dep(
                    "bad4",
                    None,
                    "*",
                    true,
                    None,
                    Some(&ws.join("ghost4").to_string_lossy()),
                ),
                json_dep(
                    "good",
                    None,
                    "*",
                    true,
                    None,
                    Some(&ws.join("good").to_string_lossy()),
                ),
                json_dep(
                    "good2",
                    None,
                    "*",
                    true,
                    None,
                    Some(&ws.join("good").to_string_lossy()),
                ),
            ],
            &[],
            Some(serde_json::json!({
                "nodes": [
                    { "id": format!("path+file://{}#0.1.0", ws.display()), "deps": [], "dependencies": [], "features": [] }
                ]
            })),
        );
        let report = crafted(&ws, &cache, None, 1, json);
        let failures = report.coverage.failures.join("\n");
        assert!(failures.contains("manifest for bad1"), "{failures}");
        assert!(failures.contains("is unreadable"), "{failures}");
        assert!(failures.contains("manifest for bad2"), "{failures}");
        assert!(failures.contains("is invalid"), "{failures}");
        assert!(failures.contains("manifest for bad3"), "{failures}");
        assert!(failures.contains("has no package name"), "{failures}");
        assert!(failures.contains("manifest for bad4"), "{failures}");
        assert!(failures.contains("declares other"), "{failures}");
        assert_eq!(packages(&report), vec!["good".to_owned()]);
    }

    fn raw_crate(name: &str, version: &str, files: &[(&str, &str)]) -> Vec<u8> {
        let top = format!("{name}-{version}");
        let mut bytes = Vec::new();
        let encoder = flate2::write::GzEncoder::new(&mut bytes, flate2::Compression::default());
        let mut tar = tar::Builder::new(encoder);
        for (file, text) in files {
            let data = text.as_bytes();
            let mut header = tar::Header::new_gnu();
            header.set_size(u64::try_from(data.len()).unwrap());
            header.set_mode(0o644);
            header.set_cksum();
            tar.append_data(&mut header, format!("{top}/{file}"), data)
                .unwrap();
        }
        let encoder = tar.into_inner().unwrap();
        encoder.finish().unwrap();
        bytes
    }

    /// Registry routes that make each lookup of the failure test fail its own way.
    fn failing_registry_routes() -> BTreeMap<String, Vec<u8>> {
        let lib = format!("pub fn dup_fn(value: u32) -> u32 {{ {MATCHING_BODY} }}\n");
        let mut routes = BTreeMap::new();
        for name in [
            "badreq", "gnover", "gpre", "gnomani", "gbadmani", "gwrong", "gcdn404", "gdup",
        ] {
            let body = match name {
                "gnover" => "{}",
                "gpre" => r#"{"versions":[{"num":"1.0.0-beta"}]}"#,
                _ => r#"{"versions":[{"num":"1.0.0"}]}"#,
            };
            routes.insert(format!("/api/v1/crates/{name}"), body.as_bytes().to_vec());
        }
        routes.insert(
            "/crates/gnomani/gnomani-1.0.0.crate".to_owned(),
            raw_crate("gnomani", "1.0.0", &[("src/lib.rs", "pub fn f() {}\n")]),
        );
        routes.insert(
            "/crates/gbadmani/gbadmani-1.0.0.crate".to_owned(),
            raw_crate(
                "gbadmani",
                "1.0.0",
                &[
                    ("Cargo.toml", "not toml"),
                    ("src/lib.rs", "pub fn f() {}\n"),
                ],
            ),
        );
        routes.insert(
            "/crates/gwrong/gwrong-1.0.0.crate".to_owned(),
            raw_crate(
                "gwrong",
                "1.0.0",
                &[
                    (
                        "Cargo.toml",
                        "[package]\nname = \"other\"\nversion = \"1.0.0\"\nedition = \"2021\"\n",
                    ),
                    ("src/lib.rs", "pub fn f() {}\n"),
                ],
            ),
        );
        routes.insert(
            "/crates/gdup/gdup-1.0.0.crate".to_owned(),
            crate_archive("gdup", "1.0.0", &lib),
        );
        routes
    }

    #[test]
    fn registry_lookup_failures_are_reported_per_dependency() {
        let dir = tempfile::tempdir().unwrap();
        let ws = dir.path().join("ws");
        let cache = dir.path().join("index");
        app_files(
            &ws,
            &format!("pub fn local_match(value: u32) -> u32 {{ {MATCHING_BODY} }}\n"),
        );
        put(
            &ws,
            "badspec/Cargo.toml",
            "[package]\nname = \"badspec\"\nversion = \"0.1.0\"\nedition = \"2024\"\n[dependencies]\nbadreq = { version = \"^bad\" }\n",
        );
        put(
            &ws,
            "badspec/src/lib.rs",
            "pub fn badspec_fn(value: u32) -> u32 { value + 1u32 }\n",
        );
        let server = HttpServer::start(failing_registry_routes());
        let json = crafted_meta(
            &ws,
            &[
                json_dep("g404", None, "*", true, None, None),
                json_dep(
                    "badspec",
                    None,
                    "*",
                    true,
                    None,
                    Some(&ws.join("badspec").to_string_lossy()),
                ),
                json_dep("gnover", None, "*", true, None, None),
                json_dep("gpre", None, "*", true, None, None),
                json_dep("gnomani", None, "*", true, None, None),
                json_dep("gbadmani", None, "*", true, None, None),
                json_dep("gwrong", None, "*", true, None, None),
                json_dep("gcdn404", None, "*", true, None, None),
                json_dep("gdup", None, "1.0.0", true, None, None),
                json_dep("gdup", None, "1.0.0", true, None, None),
            ],
            &[],
            Some(serde_json::json!({
                "nodes": [
                    { "id": format!("path+file://{}#0.1.0", ws.display()), "deps": [], "dependencies": [], "features": [] }
                ]
            })),
        );
        let report = crafted(&ws, &cache, Some(&server.base), 2, json);
        server.stop();
        let failures = report.coverage.failures.join("\n");
        for needle in [
            "version of g404 could not be resolved",
            "requirement ^bad for badreq is unparseable",
            "version of gnover could not be resolved (no versions listed)",
            "no stable version of gpre matches *",
            "manifest for gnomani 1.0.0 in the downloaded source is unreadable",
            "manifest for gbadmani 1.0.0 in the downloaded source is invalid",
            "manifest for gwrong 1.0.0 in the downloaded source declares other",
        ] {
            assert!(failures.contains(needle), "{needle} missing in {failures}");
        }
        assert!(failures.contains("gcdn404"), "{failures}");
        assert_eq!(packages(&report), vec!["gdup".to_owned()]);
        assert!(!report.coverage.complete, "{:?}", report.coverage);
    }

    #[test]
    fn exotic_sources_resolve_or_fail_with_their_identity() {
        let dir = tempfile::tempdir().unwrap();
        let ws = dir.path().join("ws");
        let cache = dir.path().join("index");
        app_files(
            &ws,
            &format!("pub fn local_match(value: u32) -> u32 {{ {MATCHING_BODY} }}\n"),
        );
        for name in ["gsrc", "rsrc", "usrc"] {
            pkg_files(
                &ws,
                name,
                &format!("pub fn {name}_fn(value: u32) -> u32 {{ {MATCHING_BODY} }}\n"),
            );
        }
        let sources = [
            ("gsrc", "git+https://example.com/repo"),
            ("gsrc2", "git+https://example.com/repo2"),
            ("rsrc", "registry+https://example.com/idx"),
            ("rsrc2", "registry+https://example.com/idx2"),
            ("usrc", "https://example.com/dep"),
            ("usrc2", "https://example.com/dep2"),
        ];
        let deps: Vec<_> = sources
            .iter()
            .map(|(name, source)| json_dep(name, Some(source), "*", true, None, None))
            .collect();
        let pkgs: Vec<_> = sources
            .iter()
            .map(|(name, source)| json_pkg(name, "1.0.0", &ws, Some(source), "lib", &[]))
            .collect();
        let json = crafted_meta(
            &ws,
            &deps,
            &pkgs,
            Some(serde_json::json!({
                "nodes": [
                    { "id": format!("path+file://{}#0.1.0", ws.display()), "deps": [], "dependencies": [], "features": [] }
                ]
            })),
        );
        let report = crafted(&ws, &cache, None, 1, json);
        let failures = report.coverage.failures.join("\n");
        assert!(
            failures.contains("dependency source for gsrc2 is missing"),
            "{failures}"
        );
        assert!(
            failures.contains("dependency source for rsrc2 1.0.0 is unavailable at registry https://example.com/idx2"),
            "{failures}"
        );
        assert!(
            failures.contains("dependency source for usrc2 1.0.0 is unavailable"),
            "{failures}"
        );
        assert_eq!(
            packages(&report),
            vec!["gsrc".to_owned(), "rsrc".to_owned(), "usrc".to_owned()]
        );
    }

    /// Local sources and published archives that differ or match for the privacy test.
    fn privacy_fixture(ws: &Path) -> BTreeMap<String, Vec<u8>> {
        app_files(
            ws,
            &format!("pub fn local_match(value: u32) -> u32 {{ {MATCHING_BODY} }}\n"),
        );
        put(
            ws,
            "cprivdiff/Cargo.toml",
            "[package]\nname = \"cprivdiff\"\nversion = \"1.0.0\"\nedition = \"2021\"\n",
        );
        put(
            ws,
            "cprivdiff/src/lib.rs",
            &format!("pub mod extra;\npub fn cpriv_fn(value: u32) -> u32 {{ {MATCHING_BODY} }}\n"),
        );
        put(
            ws,
            "cprivdiff/src/extra.rs",
            "pub fn extra_fn(value: u32) -> u32 { value + 1u32 }\n",
        );
        pkg_files(
            ws,
            "cmanidiff",
            &format!("pub fn cmana_fn(value: u32) -> u32 {{ {MATCHING_BODY} }}\n"),
        );
        pkg_files(ws, "cnolib", "fn main() {}\n");
        std::fs::rename(ws.join("cnolib/src/lib.rs"), ws.join("cnolib/src/main.rs")).unwrap();
        pkg_files(
            ws,
            "cwarn",
            &format!("pub fn cwarn_fn(value: u32) -> u32 {{ {MATCHING_BODY} }}\n"),
        );
        put(
            ws,
            "creadfail/src/lib.rs",
            "pub fn cread_fn(value: u32) -> u32 { value + 1u32 }\n",
        );
        let mut routes = BTreeMap::new();
        for (name, body) in [
            (
                "cprivdiff",
                format!("pub fn cpriv_fn(value: u32) -> u32 {{ {MATCHING_BODY} }}\n"),
            ),
            (
                "cmanidiff",
                format!("pub fn cmana_fn(value: u32) -> u32 {{ {MATCHING_BODY} }}\n"),
            ),
            (
                "creadfail",
                "pub fn cread_fn(value: u32) -> u32 { value + 1u32 }\n".to_owned(),
            ),
        ] {
            routes.insert(
                format!("/crates/{name}/{name}-1.0.0.crate"),
                crate_archive(name, "1.0.0", &body),
            );
        }
        routes
    }

    #[test]
    fn private_and_published_bytes_decide_the_index_privacy() {
        let dir = tempfile::tempdir().unwrap();
        let ws = dir.path().join("ws");
        let cache = dir.path().join("index");
        let server = HttpServer::start(privacy_fixture(&ws));
        let kinds = [
            ("cprivdiff", "lib"),
            ("cmanidiff", "lib"),
            ("cnolib", "bin"),
            ("cwarn", "lib"),
            ("cfail", "lib"),
            ("creadfail", "lib"),
        ];
        let deps: Vec<_> = kinds
            .iter()
            .map(|(name, _)| json_dep(name, Some(CRATES_IO_SOURCE), "*", true, None, None))
            .collect();
        let pkgs: Vec<_> = kinds
            .iter()
            .map(|(name, kind)| json_pkg(name, "1.0.0", &ws, Some(CRATES_IO_SOURCE), kind, &[]))
            .collect();
        let json = crafted_meta(
            &ws,
            &deps,
            &pkgs,
            Some(serde_json::json!({
                "nodes": [
                    { "id": format!("path+file://{}#0.1.0", ws.display()), "deps": [], "dependencies": [], "features": [] }
                ]
            })),
        );
        let report = crafted(&ws, &cache, Some(&server.base), 1, json);
        server.stop();
        let warnings = report.coverage.warnings.join("\n");
        assert!(
            warnings.contains("cwarn remains private because its published source is unavailable"),
            "{warnings}"
        );
        let failures = report.coverage.failures.join("\n");
        assert!(failures.contains("cfail-1.0.0.crate"), "{failures}");
        assert!(
            failures.contains("dependency source for creadfail has an unreadable manifest"),
            "{failures}"
        );
        assert_eq!(
            packages(&report),
            vec![
                "cmanidiff".to_owned(),
                "cprivdiff".to_owned(),
                "cwarn".to_owned()
            ]
        );
        assert!(!report.coverage.complete, "{:?}", report.coverage);
    }

    /// Registry routes that make each latest-release check fail its own way.
    fn failing_latest_routes() -> BTreeMap<String, Vec<u8>> {
        let mut routes = BTreeMap::new();
        routes.insert("/api/v1/crates/u0".to_owned(), b"{}".to_vec());
        routes.insert(
            "/api/v1/crates/u1".to_owned(),
            br#"{"crate":{"max_stable_version":"not-a-version"}}"#.to_vec(),
        );
        for name in ["u2", "u3", "u4", "u5", "u6"] {
            routes.insert(
                format!("/api/v1/crates/{name}"),
                br#"{"crate":{"max_stable_version":"2.0.0"}}"#.to_vec(),
            );
        }
        routes.insert(
            "/crates/u4/u4-2.0.0.crate".to_owned(),
            raw_crate("u4", "2.0.0", &[("src/lib.rs", "pub fn f() {}\n")]),
        );
        routes.insert(
            "/crates/u5/u5-2.0.0.crate".to_owned(),
            raw_crate(
                "u5",
                "2.0.0",
                &[
                    ("Cargo.toml", "not toml"),
                    ("src/lib.rs", "pub fn f() {}\n"),
                ],
            ),
        );
        routes.insert(
            "/crates/u6/u6-2.0.0.crate".to_owned(),
            raw_crate(
                "u6",
                "2.0.0",
                &[
                    (
                        "Cargo.toml",
                        "[package]\nname = \"u6\"\nversion = \"2.0.0\"\nedition = \"2021\"\n",
                    ),
                    ("src/main.rs", "fn main() {}\n"),
                ],
            ),
        );
        routes
    }

    #[test]
    fn latest_release_failures_are_reported_per_package() {
        let dir = tempfile::tempdir().unwrap();
        let ws = dir.path().join("ws");
        let cache = dir.path().join("index");
        app_files(
            &ws,
            "pub fn local_other(value: u32) -> u32 { value + 9u32 }\n",
        );
        for name in ["u0", "u1", "u2", "u3", "u4", "u5", "u6"] {
            pkg_files(
                &ws,
                name,
                &format!("pub fn {name}_fn(value: u32) -> u32 {{ value + 1u32 }}\n"),
            );
        }
        let server = HttpServer::start(failing_latest_routes());
        let packages_json: Vec<serde_json::Value> = [
            ("u0", "1.0.0"),
            ("u1", "1.0.0"),
            ("u2", "2.0.0"),
            ("u3", "1.0.0"),
            ("u4", "1.0.0"),
            ("u5", "1.0.0"),
            ("u6", "1.0.0"),
        ]
        .iter()
        .map(|(name, version)| json_pkg(name, version, &ws, Some(CRATES_IO_SOURCE), "lib", &[]))
        .collect();
        let json = crafted_meta(
            &ws,
            &["u0", "u1", "u2", "u3", "u4", "u5", "u6"]
                .iter()
                .map(|name| json_dep(name, Some(CRATES_IO_SOURCE), "*", true, None, None))
                .collect::<Vec<_>>(),
            &packages_json,
            Some(serde_json::json!({
                "nodes": [
                    { "id": format!("path+file://{}#0.1.0", ws.display()), "deps": [], "dependencies": [], "features": [] }
                ]
            })),
        );
        let options = DependencySearch {
            depth: Some(1),
            all_optional: Some(true),
            check_latest: Some(true),
            ..DependencySearch::default()
        };
        let report = crafted_run(&ws, &cache, Some(&server.base), &options, json);
        server.stop();
        let warnings = report.coverage.warnings.join("\n");
        assert!(
            warnings.contains("latest release of u0 is unknown (no max_stable_version)"),
            "{warnings}"
        );
        assert!(
            warnings
                .contains("latest release of u1 is unknown (unparseable version not-a-version)"),
            "{warnings}"
        );
        assert!(
            warnings.contains("newer version u6-2.0.0 exposes no library"),
            "{warnings}"
        );
        assert!(!warnings.contains("newer version u2"), "{warnings}");
        assert!(!warnings.contains("latest release of u2"), "{warnings}");
        let failures = report.coverage.failures.join("\n");
        assert!(
            failures.contains("newer version u4-2.0.0 manifest is unreadable"),
            "{failures}"
        );
        assert!(
            failures.contains("newer version u5-2.0.0 manifest is invalid"),
            "{failures}"
        );
        assert!(failures.contains("u3-2.0.0.crate"), "{failures}");
        assert!(!report.coverage.complete, "{:?}", report.coverage);
    }

    /// One indexed context record for the group unit test.
    fn record(
        item: &str,
        file: &str,
        line: u32,
        crate_name: &str,
        access: ContextAccess,
    ) -> IndexedContext {
        IndexedContext {
            form: "F".to_owned(),
            tokens: 50,
            site: crate::ContextSite {
                file: file.to_owned(),
                line,
                column: 1,
                end: line + 1,
                end_column: 2,
                item: item.to_owned(),
                kind: crate::ContextKind::FunctionBody,
            },
            module: crate_name.to_owned(),
            crate_name: crate_name.to_owned(),
            crate_root: "app/src/lib.rs".to_owned(),
            access,
            api_path: None,
            conditions: Vec::new(),
            extraction_notes: Vec::new(),
        }
    }

    /// One dependency record for the group unit test.
    fn dep_rec(
        name: &str,
        source: &str,
        root: &Path,
        file: &str,
        line: u32,
        access: ContextAccess,
        latest_only: bool,
    ) -> DepRec {
        DepRec {
            name: name.to_owned(),
            version: "1.0.0".to_owned(),
            source: source.to_owned(),
            depth: 0,
            root: root.to_path_buf(),
            repository: None,
            latest_only,
            record: record(&format!("{name}::helper"), file, line, name, access),
        }
    }

    #[test]
    fn group_merges_candidates_by_action_rank() {
        let dir = tempfile::tempdir().unwrap();
        let ws = dir.path();
        let anchor = record(
            "app::anchor",
            "app/src/lib.rs",
            10,
            "app",
            ContextAccess::Embedded,
        );
        let recs: Vec<DepRec> = vec![
            dep_rec(
                "up",
                "",
                ws,
                "app/src/lib.rs",
                10,
                ContextAccess::PublicFunction,
                false,
            ),
            dep_rec(
                "plain",
                "",
                ws,
                "plain/src/lib.rs",
                60,
                ContextAccess::PublicFunction,
                false,
            ),
            dep_rec(
                "latest",
                "registry+x",
                ws,
                "latest/src/lib.rs",
                40,
                ContextAccess::PublicFunction,
                true,
            ),
            dep_rec(
                "intern",
                "",
                ws,
                "intern/src/lib.rs",
                20,
                ContextAccess::InternalFunction,
                false,
            ),
            dep_rec(
                "embed",
                "",
                ws,
                "embed/src/lib.rs",
                50,
                ContextAccess::Embedded,
                false,
            ),
            dep_rec(
                "unres",
                "",
                ws,
                "unres/src/lib.rs",
                30,
                ContextAccess::Unresolved,
                false,
            ),
        ];
        let mut by_form: BTreeMap<String, Vec<DepRec>> = BTreeMap::new();
        by_form.insert("F".to_owned(), recs);
        let anchors: Vec<&IndexedContext> = vec![&anchor, &anchor];
        let (groups, _) = group(&anchors, &by_form, ws, 0);
        assert_eq!(groups.len(), 1, "{groups:?}");
        let names: Vec<String> = groups[0]
            .candidates
            .iter()
            .map(|c| c.package.clone())
            .collect();
        assert_eq!(names, vec!["plain", "latest", "embed", "intern", "unres"]);
        let actions: Vec<ReuseAction> = groups[0].candidates.iter().map(|c| c.action).collect();
        assert_eq!(
            actions,
            vec![
                ReuseAction::ReuseApi,
                ReuseAction::UpgradeApi,
                ReuseAction::RequestExtraction,
                ReuseAction::RequestExtraction,
                ReuseAction::Unresolved
            ]
        );
    }
}
