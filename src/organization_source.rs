//! Acquisition of pinned GitHub organization snapshots.

use alloc::borrow::ToOwned;
use alloc::collections::BTreeSet;
use alloc::format;
use alloc::string::String;
use alloc::string::ToString;
use alloc::vec::Vec;
use core::fmt::Write as _;
use std::path::{Path, PathBuf};

use crate::organization_snapshot::SNAPSHOT_TREE;

/// The skipped-link report inside a commit directory, one coverage failure per line.
const LINK_REPORT: &str = "links";

/// The GitHub REST API.
const GITHUB_API: &str = "https://api.github.com";

/// Repositories per enumeration page.
const PAGE_SIZE: u64 = 100;

/// Enumeration pages one organization may span.
const MAX_PAGES: u32 = 10_000;

/// Deadline for reading the `origin` remote.
const GIT_DEADLINE: core::time::Duration = core::time::Duration::from_secs(10);

/// The organization snapshot corpus and its per-repository source roots.
pub(crate) struct Corpus {
    /// The organization report with repository, package and coverage records.
    pub(crate) report: crate::organization::OrganizationReport,
    /// The extracted snapshot root for each repository, aligned to `report.repositories`.
    pub(crate) roots: Vec<Option<PathBuf>>,
    /// The targets of each successfully discovered package.
    pub(crate) packages: Vec<PackageSource>,
}

/// The scan targets of one discovered package and its report index.
pub(crate) struct PackageSource {
    /// Index into `report.packages`.
    pub(crate) package: usize,
    /// The package's scan targets.
    pub(crate) targets: Vec<crate::discover::Target>,
}

/// One repository acquisition failure.
#[derive(Debug, thiserror::Error)]
pub(crate) enum SourceError {
    /// A GitHub API request failed.
    #[error("{0}")]
    Api(String),
    /// The repository archive could not be downloaded.
    #[error("{0}")]
    Download(String),
    /// The repository archive could not be extracted.
    #[error("{0}")]
    Extract(#[from] crate::source_archive::ArchiveError),
    /// The private cache storage could not be prepared.
    #[error("I/O: {0}")]
    Storage(#[source] std::io::Error),
}

/// Why the current repository's GitHub owner could not be determined.
#[derive(Debug, thiserror::Error)]
pub enum OwnerError {
    /// `git remote get-url origin` failed, for a directory outside a repository or one
    /// without an `origin` remote.
    #[error("the current repository has no readable origin remote, {0}")]
    NoOrigin(String),
    /// The `origin` remote is not a `github.com` repository URL.
    #[error("the origin remote {0} is not a GitHub repository")]
    NotGitHub(String),
}

/// The GitHub owner of the repository at `dir`, from `GITHUB_REPOSITORY_OWNER` when set and
/// non-empty, else from its `origin` remote.
///
/// # Errors
///
/// Fails when the variable is unset or empty and `origin` is unreadable or not a
/// `github.com` repository URL.
pub fn repository_owner(dir: impl AsRef<Path>) -> Result<String, OwnerError> {
    let variable = std::env::var("GITHUB_REPOSITORY_OWNER").ok();
    resolve_owner(variable.as_deref(), || origin_url(dir.as_ref()))
}

/// The owner named by `variable`, else by the `origin` URL `origin` reads.
fn resolve_owner(
    variable: Option<&str>,
    origin: impl FnOnce() -> Result<String, OwnerError>,
) -> Result<String, OwnerError> {
    if let Some(owner) = variable.filter(|owner| !owner.is_empty()) {
        return Ok(owner.to_owned());
    }
    let url = origin()?;
    github_owner(&url)
        .map(str::to_owned)
        .ok_or(OwnerError::NotGitHub(url))
}

/// The `origin` remote URL of the repository at `dir`, through a bounded `git` call.
fn origin_url(dir: &Path) -> Result<String, OwnerError> {
    let mut command = std::process::Command::new("git");
    command
        .arg("-C")
        .arg(dir)
        .args(["remote", "get-url", "origin"]);
    let id = crate::dependencies::next_proc_id();
    let output = crate::dependencies::run_bounded(&mut command, GIT_DEADLINE, &id)
        .map_err(|error| OwnerError::NoOrigin(error.to_string()))?;
    if !output.status.success() {
        return Err(OwnerError::NoOrigin(
            String::from_utf8_lossy(&output.stderr).trim().to_string(),
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

/// The owner of a `github.com` repository URL in HTTPS, SSH, scp or git form.
fn github_owner(url: &str) -> Option<&str> {
    let (authority, path) = match url.split_once("://") {
        Some((_, rest)) => rest.split_once('/')?,
        None => url.split_once(':')?,
    };
    let host = authority.rsplit('@').next()?;
    let host = host.split_once(':').map_or(host, |(host, _)| host);
    if !host.eq_ignore_ascii_case("github.com") {
        return None;
    }
    let owner = path.trim_start_matches('/').split('/').next()?;
    is_github_login(owner).then_some(owner)
}

/// Load every accessible repository of one organization as a pinned snapshot corpus.
///
/// Branches are pinned on every run. Snapshots are cached privately, keyed by repository and
/// full commit, and reused while immutable. Failed repositories are recorded in
/// `report.coverage.failures` with repository-qualified strings.
#[must_use = "Use the corpus as the organization scan input"]
pub(crate) fn load(organization: &str, cache_root: Option<&Path>, all_targets: bool) -> Corpus {
    let cache =
        cache_root.map_or_else(crate::context_cache::Store::default_root, Path::to_path_buf);
    load_with_bases(organization, GITHUB_API, &cache, all_targets)
}

/// The acquisition against one explicit API base, the loopback seam for the regressions.
fn load_with_bases(organization: &str, api: &str, cache: &Path, all_targets: bool) -> Corpus {
    let mut repos: Vec<RepoRef> = Vec::new();
    let mut failures: Vec<String> = Vec::new();
    if is_github_login(organization) {
        enumerate_repos(api, organization, MAX_PAGES, &mut repos, &mut failures);
    } else {
        failures.push(format!(
            "{organization}: the organization login is not a valid GitHub login"
        ));
    }
    let mut report = crate::organization::OrganizationReport {
        name: organization.to_string(),
        ..Default::default()
    };
    let mut roots: Vec<Option<PathBuf>> = Vec::with_capacity(repos.len());
    let mut packages: Vec<PackageSource> = Vec::new();
    for (index, repo) in repos.into_iter().enumerate() {
        let (commit, snapshot) = acquire_repo(api, &repo, &mut failures, cache);
        let Some(snapshot) = snapshot else {
            report
                .repositories
                .push(crate::organization::OrganizationRepository {
                    name: repo.name,
                    commit,
                });
            roots.push(None);
            continue;
        };
        discover_repo(
            &snapshot,
            index,
            &repo.name,
            all_targets,
            &mut report,
            &mut packages,
            &mut failures,
        );
        report
            .repositories
            .push(crate::organization::OrganizationRepository {
                name: repo.name,
                commit,
            });
        roots.push(Some(snapshot.root));
    }
    report.coverage = crate::organization::OrganizationCoverage {
        complete: failures.is_empty(),
        failures,
    };
    Corpus {
        report,
        roots,
        packages,
    }
}

/// One enumerated repository.
struct RepoRef {
    /// The `owner/repo` name.
    name: String,
    /// The default branch, from the listing.
    default: Option<String>,
}

/// A valid GitHub login, one to 39 alphanumeric, hyphen or underscore characters starting
/// with an alphanumeric.
fn is_github_login(login: &str) -> bool {
    (1..=39).contains(&login.len())
        && login
            .bytes()
            .next()
            .is_some_and(|byte| byte.is_ascii_alphanumeric())
        && login
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
}

fn is_repository_name(name: &str) -> bool {
    (1..=100).contains(&name.len())
        && name != "."
        && name != ".."
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'_'))
}

/// The first listing page of `owner`, chosen by its account kind, recording a failure when
/// the kind is unknown.
fn listing_url(api: &str, owner: &str, failures: &mut Vec<String>) -> Option<String> {
    let encoded = crate::context_upstream::percent_encode(owner);
    let lookup = format!("{api}/users/{encoded}");
    let kind = match api_json(&lookup) {
        Ok(value) => value
            .get("type")
            .and_then(serde_json::Value::as_str)
            .map(str::to_owned),
        Err(error) => {
            failures.push(format!("{owner}: {error}"));
            return None;
        }
    };
    match kind.as_deref() {
        Some("Organization") => Some(format!(
            "{api}/orgs/{encoded}/repos?per_page={PAGE_SIZE}&page=1"
        )),
        Some("User") if token_owner(api, owner, failures) => Some(format!(
            "{api}/user/repos?affiliation=owner&visibility=all&per_page={PAGE_SIZE}&page=1"
        )),
        Some("User") => Some(format!(
            "{api}/users/{encoded}/repos?type=owner&per_page={PAGE_SIZE}&page=1"
        )),
        _ => {
            failures.push(format!(
                "{owner}: GET {lookup} names no organization or user account"
            ));
            None
        }
    }
}

/// Whether the credential belongs to `owner`, a missing credential meaning it does not and
/// any other failure recorded.
fn token_owner(api: &str, owner: &str, failures: &mut Vec<String>) -> bool {
    let url = format!("{api}/user");
    match crate::context_network::get_raw(
        &url,
        crate::context_network::READ_LIMIT,
        crate::context_network::REQUEST_DEADLINE,
    ) {
        Ok(response) if crate::context_network::is_success(response.status) => {
            serde_json::from_slice::<serde_json::Value>(&response.body)
                .ok()
                .and_then(|value| {
                    value
                        .get("login")
                        .and_then(serde_json::Value::as_str)
                        .map(|login| login.eq_ignore_ascii_case(owner))
                })
                .unwrap_or(false)
        }
        Ok(response) if response.status == 401 => false,
        Ok(response) => {
            failures.push(format!(
                "{owner}: {} GET {url} HTTP {}",
                response.id, response.status
            ));
            false
        }
        Err(error) => {
            failures.push(format!("{owner}: {error}"));
            false
        }
    }
}

/// One bounded `JSON` API document, or the correlated failure.
fn api_json(url: &str) -> Result<serde_json::Value, String> {
    let response = crate::context_network::get_raw(
        url,
        crate::context_network::READ_LIMIT,
        crate::context_network::REQUEST_DEADLINE,
    )?;
    if !crate::context_network::is_success(response.status) {
        return Err(format!(
            "{} GET {url} HTTP {}",
            response.id, response.status
        ));
    }
    serde_json::from_slice(&response.body)
        .map_err(|error| format!("{} GET {url} invalid JSON {error}", response.id))
}

/// List every accessible repository of `organization` within `max_pages`, following
/// next-page links and retaining the repositories already listed when a page fails.
fn enumerate_repos(
    api: &str,
    organization: &str,
    max_pages: u32,
    repos: &mut Vec<RepoRef>,
    failures: &mut Vec<String>,
) {
    let Some(mut url) = listing_url(api, organization, failures) else {
        return;
    };
    let api_prefix = format!("{api}/");
    let mut seen: BTreeSet<String> = BTreeSet::new();
    for _ in 0..max_pages {
        let crate::context_network::RawResponse {
            status,
            body,
            id,
            next,
            ..
        } = match crate::context_network::get_raw(
            &url,
            crate::context_network::READ_LIMIT,
            crate::context_network::REQUEST_DEADLINE,
        ) {
            Ok(response) => response,
            Err(error) => {
                failures.push(format!("{organization}: {error}"));
                return;
            }
        };
        if !crate::context_network::is_success(status) {
            failures.push(format!("{organization}: {id} GET {url} HTTP {status}"));
            return;
        }
        let Ok(serde_json::Value::Array(entries)) = serde_json::from_slice(&body) else {
            failures.push(format!(
                "{organization}: the repository listing is not a JSON array"
            ));
            return;
        };
        for (offset, entry) in entries.iter().enumerate() {
            let Some(name) = entry.get("full_name").and_then(serde_json::Value::as_str) else {
                failures.push(format!(
                    "{organization}: repository listing entry {offset} has no full_name"
                ));
                continue;
            };
            let mut parts = name.split('/');
            let (Some(owner), Some(repo), None) = (parts.next(), parts.next(), parts.next()) else {
                failures.push(format!(
                    "{organization}: repository listing entry {offset} has an invalid full_name"
                ));
                continue;
            };
            if !is_github_login(owner) || !is_repository_name(repo) {
                failures.push(format!(
                    "{organization}: repository listing entry {offset} has an invalid full_name"
                ));
                continue;
            }
            if !owner.eq_ignore_ascii_case(organization) {
                failures.push(format!(
                    "{organization}: repository listing entry {offset} names another owner"
                ));
                continue;
            }
            let name = format!("{owner}/{repo}");
            if !seen.insert(name.to_ascii_lowercase()) {
                continue;
            }
            repos.push(RepoRef {
                name,
                default: entry
                    .get("default_branch")
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_owned),
            });
        }
        let Some(next) = next else {
            return;
        };
        if !next.starts_with(&api_prefix) {
            failures.push(format!(
                "{organization}: {id} next page {next} leaves the API"
            ));
            return;
        }
        url = next;
    }
    failures.push(format!(
        "{organization}: the listing exceeds {max_pages} pages"
    ));
}

/// One published snapshot.
struct Snapshot {
    /// The private commit directory.
    commit: PathBuf,
    /// The canonical snapshot root inside it.
    root: PathBuf,
}

/// Pin the default branch and acquire the pinned snapshot, recording repository-qualified
/// failures for skipped links and for an unavailable repository.
fn acquire_repo(
    api: &str,
    repo: &RepoRef,
    failures: &mut Vec<String>,
    cache: &Path,
) -> (Option<String>, Option<Snapshot>) {
    let Some(default) = &repo.default else {
        failures.push(format!("{}: the default branch is unknown", repo.name));
        return (None, None);
    };
    match pin_commit(api, &repo.name, default) {
        Ok(None) => (None, None),
        Err(cause) => {
            failures.push(format!("{}: {}", repo.name, cause));
            (None, None)
        }
        Ok(Some(sha)) => match acquire_snapshot(api, &repo.name, &sha, cache) {
            Ok((snapshot, report)) => {
                failures.extend(report.lines().map(|line| format!("{}: {line}", repo.name)));
                (Some(sha), Some(snapshot))
            }
            Err(error) => {
                failures.push(format!("{}: {}", repo.name, error));
                (Some(sha), None)
            }
        },
    }
}

/// The full commit of one branch, distinguishing a GitHub-confirmed empty repository from a
/// pin failure.
fn pin_commit(api: &str, name: &str, default: &str) -> Result<Option<String>, SourceError> {
    let url = format!(
        "{api}/repos/{}/git/ref/heads/{branch}",
        crate::context_upstream::encode_path(name),
        branch = crate::context_upstream::percent_encode(default)
    );
    let crate::context_network::RawResponse {
        status, body, id, ..
    } = crate::context_network::get_raw(
        &url,
        crate::context_network::READ_LIMIT,
        crate::context_network::REQUEST_DEADLINE,
    )
    .map_err(SourceError::Api)?;
    if !crate::context_network::is_success(status) {
        if status == 409 {
            let message = serde_json::from_slice::<serde_json::Value>(&body)
                .ok()
                .and_then(|value| {
                    value
                        .get("message")
                        .and_then(serde_json::Value::as_str)
                        .map(str::to_ascii_lowercase)
                })
                .unwrap_or_default();
            if message.contains("git repository is empty") {
                return Ok(None);
            }
        }
        return Err(SourceError::Api(format!("{id} GET {url} HTTP {status}")));
    }
    let sha = serde_json::from_slice::<serde_json::Value>(&body)
        .ok()
        .and_then(|value| {
            value
                .get("object")
                .and_then(|object| object.get("sha"))
                .and_then(serde_json::Value::as_str)
                .filter(|sha| is_full_sha(sha))
                .map(str::to_owned)
        });
    match sha {
        Some(sha) => Ok(Some(sha)),
        None => Err(SourceError::Api(format!(
            "{id} GET {url} has no full commit identifier"
        ))),
    }
}

/// A full 40-character hexadecimal commit identifier.
fn is_full_sha(sha: &str) -> bool {
    sha.len() == 40 && sha.bytes().all(|byte| byte.is_ascii_hexdigit())
}

/// The snapshot for one pinned commit and its skipped-link report, downloaded and extracted
/// only when the cache misses.
fn acquire_snapshot(
    api: &str,
    name: &str,
    sha: &str,
    cache: &Path,
) -> Result<(Snapshot, String), SourceError> {
    let base = private_base(cache, name)?;
    let commit = base.join(sha);
    if !commit.is_dir() {
        let archive = base.join(format!("{sha}.tar.gz"));
        if !archive.exists() {
            let url = format!(
                "{api}/repos/{}/tarball/{sha}",
                crate::context_upstream::encode_path(name)
            );
            crate::context_network::download(&url, &archive).map_err(SourceError::Download)?;
            #[cfg(unix)]
            shield_archive(&archive);
        }
        publish(&archive, &base, &commit)?;
        std::fs::remove_file(&archive).ok();
    }
    let report = std::fs::read_to_string(commit.join(LINK_REPORT)).map_err(SourceError::Storage)?;
    let root = std::fs::canonicalize(commit.join(SNAPSHOT_TREE)).map_err(SourceError::Storage)?;
    Ok((Snapshot { commit, root }, report))
}

/// Extract `archive` and write its link report in a private stage beside `commit`, then
/// publish the stage as `commit` with one rename, a concurrent publisher's result winning.
fn publish(archive: &Path, base: &Path, commit: &Path) -> Result<(), SourceError> {
    let stage = tempfile::Builder::new()
        .prefix(".stage-")
        .tempdir_in(base)
        .map_err(SourceError::Storage)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(stage.path(), std::fs::Permissions::from_mode(0o700))
            .map_err(SourceError::Storage)?;
    }
    let skipped = crate::source_archive::extract_archive(
        archive,
        stage.path().join(SNAPSHOT_TREE),
        crate::source_archive::ArchiveKind::GitHub,
    )?;
    let mut report = String::new();
    for link in &skipped {
        let _ = writeln!(report, "{link}");
    }
    std::fs::write(stage.path().join(LINK_REPORT), report).map_err(SourceError::Storage)?;
    match std::fs::rename(stage.path(), commit) {
        Ok(()) => {
            let _ = stage.keep();
            Ok(())
        }
        Err(_) if commit.is_dir() => Ok(()),
        Err(error) => Err(SourceError::Storage(error)),
    }
}

/// The owner-only cache directory for one repository, under the organization namespace.
fn private_base(cache: &Path, name: &str) -> Result<PathBuf, SourceError> {
    let namespace = cache.join("organization");
    let base = namespace.join(blake3::hash(name.as_bytes()).to_hex().as_str());
    std::fs::create_dir_all(&base).map_err(SourceError::Storage)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        for dir in [namespace.as_path(), base.as_path()] {
            std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))
                .map_err(SourceError::Storage)?;
        }
    }
    Ok(base)
}

/// Keep a downloaded archive readable only by the owner.
#[cfg(unix)]
fn shield_archive(archive: &Path) {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(archive, std::fs::Permissions::from_mode(0o600)).ok();
}

/// The discovered packages of one snapshot, with their organization provenance.
fn discover_repo(
    snapshot: &Snapshot,
    repository: usize,
    name: &str,
    all_targets: bool,
    report: &mut crate::organization::OrganizationReport,
    packages: &mut Vec<PackageSource>,
    failures: &mut Vec<String>,
) {
    let discovery = crate::organization_snapshot::discover(&snapshot.commit, all_targets);
    for failure in discovery.failures {
        failures.push(format!("{name}: {failure}"));
    }
    for package in discovery.packages {
        let index = report.packages.len();
        report
            .packages
            .push(crate::organization::OrganizationPackage {
                repository,
                name: package.name,
                manifest: relative_manifest(&snapshot.root, &package.manifest),
            });
        packages.push(PackageSource {
            package: index,
            targets: package.targets,
        });
    }
}

/// A repository-relative manifest path.
fn relative_manifest(snapshot: &Path, manifest: &Path) -> String {
    crate::organization::path_label(manifest.strip_prefix(snapshot).unwrap_or(manifest))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::context_network::tests::Fixture;
    use crate::source_archive::tests::packed;
    use alloc::vec;

    /// A JSON repository listing entry.
    fn entry(name: &str) -> serde_json::Value {
        serde_json::json!({
            "full_name": name,
            "default_branch": "main",
        })
    }

    /// The GitHub 409 body confirming an empty repository.
    const EMPTY_BODY: &[u8] = b"{\"message\": \"Git Repository is empty\"}";

    /// Register the `/users/{login}` lookup answering account `kind`.
    fn account(fixture: &Fixture, login: &str, kind: &str) {
        fixture.route(
            &format!("/api/users/{login}"),
            200,
            &serde_json::to_vec(&serde_json::json!({ "login": login, "type": kind })).unwrap(),
        );
    }

    /// A fixture whose owner `o` is an organization.
    fn organization_fixture() -> Fixture {
        let fixture = Fixture::start();
        account(&fixture, "o", "Organization");
        fixture
    }

    /// The repository names a corpus lists.
    fn names(corpus: &Corpus) -> Vec<&str> {
        corpus
            .report
            .repositories
            .iter()
            .map(|repo| repo.name.as_str())
            .collect()
    }

    #[test]
    fn a_user_account_lists_its_public_repositories() {
        let fixture = Fixture::start();
        account(&fixture, "u", "User");
        fixture.route(
            "/api/user",
            401,
            b"{\"message\": \"Requires authentication\"}",
        );
        fixture.page_route("/api/users/u/repos", vec![listing(&[entry("u/app")])]);
        empty_pins(&fixture, &["u/app"]);
        let dir = tempfile::tempdir().unwrap();
        let corpus = load_with_bases("u", fixture.api_base().as_str(), dir.path(), false);
        assert_eq!(corpus.report.coverage.failures, Vec::<String>::new());
        assert_eq!(names(&corpus), ["u/app"]);
        assert_eq!(
            fixture.last_target("/api/users/u/repos").as_deref(),
            Some("/api/users/u/repos?type=owner&per_page=100&page=1")
        );
        assert_eq!(fixture.hits("/api/orgs/u/repos"), 0);
    }

    #[test]
    fn the_token_owner_lists_its_private_repositories() {
        let fixture = Fixture::start();
        account(&fixture, "u", "User");
        fixture.route("/api/user", 200, b"{\"login\": \"U\"}");
        fixture.page_route("/api/user/repos", vec![listing(&[entry("u/secret")])]);
        empty_pins(&fixture, &["u/secret"]);
        let dir = tempfile::tempdir().unwrap();
        let corpus = load_with_bases("u", fixture.api_base().as_str(), dir.path(), false);
        assert_eq!(corpus.report.coverage.failures, Vec::<String>::new());
        assert_eq!(names(&corpus), ["u/secret"]);
        assert_eq!(
            fixture.last_target("/api/user/repos").as_deref(),
            Some("/api/user/repos?affiliation=owner&visibility=all&per_page=100&page=1")
        );
        assert_eq!(fixture.hits("/api/users/u/repos"), 0);
    }

    #[test]
    fn an_unreadable_token_owner_leaves_the_public_listing_incomplete() {
        let fixture = Fixture::start();
        account(&fixture, "u", "User");
        fixture.route("/api/user", 500, b"{}");
        fixture.page_route("/api/users/u/repos", vec![listing(&[entry("u/app")])]);
        empty_pins(&fixture, &["u/app"]);
        let dir = tempfile::tempdir().unwrap();
        let corpus = load_with_bases("u", fixture.api_base().as_str(), dir.path(), false);
        assert!(!corpus.report.coverage.complete);
        assert_eq!(corpus.report.coverage.failures.len(), 1);
        assert!(corpus.report.coverage.failures[0].starts_with("u: "));
        assert!(corpus.report.coverage.failures[0].contains("/user HTTP 500"));
        assert_eq!(names(&corpus), ["u/app"]);
    }

    #[test]
    fn an_unknown_account_lists_nothing() {
        for (status, body) in [
            (404, b"{}".to_vec()),
            (200, b"{\"type\": \"Bot\"}".to_vec()),
            (200, b"not json".to_vec()),
        ] {
            let fixture = Fixture::start();
            fixture.route("/api/users/o", status, &body);
            let dir = tempfile::tempdir().unwrap();
            let corpus = load_with_bases("o", fixture.api_base().as_str(), dir.path(), false);
            assert_eq!(corpus.report.coverage.failures.len(), 1, "{status}");
            assert!(corpus.report.coverage.failures[0].starts_with("o: "));
            assert_eq!(corpus.report.repositories.len(), 0);
            assert_eq!(fixture.hits("/api/orgs/o/repos"), 0);
            assert_eq!(fixture.hits("/api/users/o/repos"), 0);
        }
    }

    #[test]
    fn github_remotes_name_their_owner() {
        for url in [
            "git@github.com:Owner-1/repo.git",
            "https://github.com/Owner-1/repo",
            "https://user@GitHub.com/Owner-1/repo.git",
            "ssh://git@github.com:22/Owner-1/repo.git",
            "git://github.com/Owner-1/repo.git",
        ] {
            assert_eq!(github_owner(url), Some("Owner-1"), "{url}");
        }
        for url in [
            "git@gitlab.com:owner/repo.git",
            "https://github.com.evil.example/owner/repo",
            "/srv/git/repo",
            "C:\\repos\\app",
            "https://github.com/",
            "https://github.com/-bad/repo",
        ] {
            assert_eq!(github_owner(url), None, "{url}");
        }
    }

    #[test]
    fn the_origin_remote_is_read_from_git() {
        let dir = tempfile::tempdir().unwrap();
        let git = |args: &[&str]| {
            let status = std::process::Command::new("git")
                .current_dir(dir.path())
                .args(args)
                .status()
                .unwrap();
            assert!(status.success(), "git {args:?}");
        };
        git(&["init", "-q"]);
        assert!(matches!(
            origin_url(dir.path()),
            Err(OwnerError::NoOrigin(_))
        ));
        git(&["remote", "add", "origin", "git@github.com:owner/app.git"]);
        assert_eq!(
            origin_url(dir.path()).unwrap(),
            "git@github.com:owner/app.git"
        );
    }

    #[test]
    fn the_actions_owner_precedes_the_origin_remote() {
        let remote = || Ok(String::from("git@github.com:remote/app.git"));
        let unread = || Err(OwnerError::NoOrigin(String::from("the variable decides")));
        assert_eq!(resolve_owner(Some("from-env"), unread).unwrap(), "from-env");
        assert_eq!(resolve_owner(Some(""), remote).unwrap(), "remote");
        assert_eq!(resolve_owner(None, remote).unwrap(), "remote");
        assert!(matches!(
            resolve_owner(None, || Ok(String::from("https://gitlab.com/a/b"))),
            Err(OwnerError::NotGitHub(url)) if url == "https://gitlab.com/a/b"
        ));
        assert!(matches!(
            resolve_owner(None, || Err(OwnerError::NoOrigin(String::from("none")))),
            Err(OwnerError::NoOrigin(_))
        ));
    }

    /// A gzip tar of one root directory without any Cargo package.
    fn snapshot_archive(root: &str) -> Vec<u8> {
        packed(vec![
            (b'5', format!("{root}/").into(), String::new(), Vec::new()),
            (
                0,
                format!("{root}/readme").into(),
                String::new(),
                b"snapshot".to_vec(),
            ),
        ])
    }

    /// A gzip tar of one root directory holding the standalone `good` package.
    fn package_archive(root: &str) -> Vec<u8> {
        packed(vec![
            (b'5', format!("{root}/").into(), String::new(), Vec::new()),
            (
                0,
                format!("{root}/Cargo.toml").into(),
                String::new(),
                b"[package]\nname = \"good\"\nversion = \"0.1.0\"\nedition = \"2024\"\n".to_vec(),
            ),
            (
                b'5',
                format!("{root}/src/").into(),
                String::new(),
                Vec::new(),
            ),
            (
                0,
                format!("{root}/src/lib.rs").into(),
                String::new(),
                b"pub fn good() {}\n".to_vec(),
            ),
        ])
    }

    fn listing(entries: &[serde_json::Value]) -> (u16, Vec<u8>) {
        (200, serde_json::to_vec(entries).unwrap())
    }

    fn pin_response(sha: &str) -> (u16, Vec<u8>) {
        (
            200,
            serde_json::to_vec(&serde_json::json!({ "object": { "sha": sha } })).unwrap(),
        )
    }

    /// Register the empty-repository pin response for every listed repository.
    fn empty_pins(fixture: &Fixture, names: &[&str]) {
        for name in names {
            fixture.route(
                &format!("/api/repos/{name}/git/ref/heads/main"),
                409,
                EMPTY_BODY,
            );
        }
    }

    #[test]
    fn enumeration_paginates_to_the_last_page() {
        let fixture = organization_fixture();
        let page_one: Vec<serde_json::Value> =
            (0..100).map(|i| entry(&format!("o/r{i:03}"))).collect();
        let page_two: Vec<serde_json::Value> =
            (100..105).map(|i| entry(&format!("o/r{i:03}"))).collect();
        fixture.page_route(
            "/api/orgs/o/repos",
            vec![listing(&page_one), listing(&page_two)],
        );
        let names: Vec<String> = (0..105).map(|i| format!("o/r{i:03}")).collect();
        let refs: Vec<&str> = names.iter().map(String::as_str).collect();
        empty_pins(&fixture, &refs);
        let dir = tempfile::tempdir().unwrap();
        let corpus = load_with_bases("o", fixture.api_base().as_str(), dir.path(), false);
        assert_eq!(corpus.report.name, "o");
        assert_eq!(corpus.report.repositories.len(), 105);
        assert_eq!(fixture.hits("/api/orgs/o/repos"), 2);
        assert_eq!(
            fixture.last_target("/api/orgs/o/repos").as_deref(),
            Some("/api/orgs/o/repos?per_page=100&page=2")
        );
        assert!(corpus.report.coverage.complete);
        assert_eq!(corpus.report.coverage.failures, Vec::<String>::new());
        assert_eq!(corpus.roots.len(), corpus.report.repositories.len());
        assert!(corpus.roots.iter().all(Option::is_none));
        assert!(
            corpus
                .report
                .repositories
                .iter()
                .all(|repo| repo.commit.is_none())
        );
        assert!(corpus.packages.is_empty());
    }

    #[test]
    fn next_links_preserve_repositories_after_empty_and_short_pages() {
        let fixture = organization_fixture();
        fixture.page_route(
            "/api/orgs/o/repos",
            vec![
                listing(&[]),
                listing(&[entry("o/first")]),
                listing(&[entry("o/last")]),
            ],
        );
        empty_pins(&fixture, &["o/first", "o/last"]);
        let cache = tempfile::tempdir().unwrap();
        let corpus = load_with_bases("o", fixture.api_base().as_str(), cache.path(), false);
        assert!(
            corpus.report.coverage.complete,
            "{:?}",
            corpus.report.coverage.failures
        );
        assert!(
            corpus
                .report
                .repositories
                .iter()
                .map(|repo| repo.name.as_str())
                .eq(["o/first", "o/last"])
        );
    }

    #[test]
    fn a_failed_page_retains_the_earlier_repositories() {
        let fixture = organization_fixture();
        let page_one: Vec<serde_json::Value> =
            (0..50).map(|i| entry(&format!("o/r{i:03}"))).collect();
        fixture.page_route(
            "/api/orgs/o/repos",
            vec![listing(&page_one), (500, Vec::new())],
        );
        let names: Vec<String> = (0..50).map(|i| format!("o/r{i:03}")).collect();
        let refs: Vec<&str> = names.iter().map(String::as_str).collect();
        empty_pins(&fixture, &refs);
        let dir = tempfile::tempdir().unwrap();
        let corpus = load_with_bases("o", fixture.api_base().as_str(), dir.path(), false);
        assert_eq!(corpus.report.repositories.len(), 50);
        assert_eq!(fixture.hits("/api/orgs/o/repos"), 2);
        assert!(!corpus.report.coverage.complete);
        assert_eq!(corpus.report.coverage.failures.len(), 1);
        assert!(corpus.report.coverage.failures[0].starts_with("o: "));
        assert!(corpus.report.coverage.failures[0].contains("HTTP 500"));
        assert!(corpus.roots.iter().all(Option::is_none));
    }

    #[test]
    fn a_missing_organization_is_incomplete_coverage() {
        let fixture = organization_fixture();
        fixture.route("/api/orgs/o/repos", 404, b"[]");
        let dir = tempfile::tempdir().unwrap();
        let corpus = load_with_bases("o", fixture.api_base().as_str(), dir.path(), false);
        assert_eq!(corpus.report.repositories.len(), 0);
        assert_eq!(corpus.roots.len(), 0);
        assert!(!corpus.report.coverage.complete);
        assert!(corpus.report.coverage.failures[0].starts_with("o: "));
        assert!(corpus.report.coverage.failures[0].contains("HTTP 404"));
    }

    #[test]
    fn duplicate_repository_entries_are_listed_once() {
        let fixture = organization_fixture();
        let page_one = vec![entry("o/r1"), entry("o/r1"), entry("o/r2")];
        let page_two = vec![entry("o/r3"), entry("o/r1")];
        fixture.page_route(
            "/api/orgs/o/repos",
            vec![listing(&page_one), listing(&page_two)],
        );
        empty_pins(&fixture, &["o/r1", "o/r2", "o/r3"]);
        let dir = tempfile::tempdir().unwrap();
        let corpus = load_with_bases("o", fixture.api_base().as_str(), dir.path(), false);
        assert_eq!(fixture.hits("/api/orgs/o/repos"), 2);
        let names: Vec<&str> = corpus
            .report
            .repositories
            .iter()
            .map(|repo| repo.name.as_str())
            .collect();
        assert_eq!(names, ["o/r1", "o/r2", "o/r3"]);
        assert!(corpus.report.coverage.complete);
    }

    #[test]
    fn a_git_confirmed_empty_repository_is_a_valid_empty_input() {
        let fixture = organization_fixture();
        fixture.page_route(
            "/api/orgs/o/repos",
            vec![listing(&[entry("o/solo")]), (200, b"[]".to_vec())],
        );
        fixture.route("/api/repos/o/solo/git/ref/heads/main", 409, EMPTY_BODY);
        let dir = tempfile::tempdir().unwrap();
        let corpus = load_with_bases("o", fixture.api_base().as_str(), dir.path(), false);
        assert_eq!(corpus.report.repositories.len(), 1);
        assert!(corpus.report.repositories[0].commit.is_none());
        assert!(corpus.roots[0].is_none());
        assert!(corpus.report.coverage.complete);
        assert_eq!(fixture.hits("/api/repos/o/solo/git/ref/heads/main"), 1);
    }

    #[test]
    fn an_unrelated_conflict_is_a_repository_failure() {
        let fixture = organization_fixture();
        fixture.page_route(
            "/api/orgs/o/repos",
            vec![listing(&[entry("o/app")]), (200, b"[]".to_vec())],
        );
        fixture.route(
            "/api/repos/o/app/git/ref/heads/main",
            409,
            b"{\"message\": \"Something else\"}",
        );
        let dir = tempfile::tempdir().unwrap();
        let corpus = load_with_bases("o", fixture.api_base().as_str(), dir.path(), false);
        assert_eq!(corpus.report.repositories.len(), 1);
        assert!(corpus.report.repositories[0].commit.is_none());
        assert!(corpus.roots[0].is_none());
        assert!(!corpus.report.coverage.complete);
        assert!(corpus.report.coverage.failures[0].starts_with("o/app: "));
        assert!(corpus.report.coverage.failures[0].contains("HTTP 409"));
    }

    #[test]
    fn an_invalid_organization_login_is_rejected_without_requests() {
        let fixture = organization_fixture();
        let dir = tempfile::tempdir().unwrap();
        let corpus = load_with_bases("../evil", fixture.api_base().as_str(), dir.path(), false);
        assert_eq!(corpus.report.repositories.len(), 0);
        assert_eq!(corpus.roots.len(), 0);
        assert!(!corpus.report.coverage.complete);
        assert!(corpus.report.coverage.failures[0].contains("not a valid GitHub login"));
        assert_eq!(fixture.hits("/api/orgs/..%2Fevil/repos"), 0);
    }

    #[test]
    fn organization_logins_follow_the_github_rules() {
        let fixture = organization_fixture();
        for login in ["rust-num", "a_b", "x"] {
            account(&fixture, login, "Organization");
            fixture.page_route(&format!("/api/orgs/{login}/repos"), vec![listing(&[])]);
            let dir = tempfile::tempdir().unwrap();
            let corpus = load_with_bases(login, fixture.api_base().as_str(), dir.path(), false);
            assert_eq!(
                corpus.report.coverage.failures,
                Vec::<String>::new(),
                "{login}"
            );
            assert_eq!(
                fixture.hits(&format!("/api/orgs/{login}/repos")),
                1,
                "{login}"
            );
        }
        for login in [String::from("-lead"), String::from("a.b"), "a".repeat(40)] {
            let dir = tempfile::tempdir().unwrap();
            let corpus = load_with_bases(&login, fixture.api_base().as_str(), dir.path(), false);
            assert!(
                corpus.report.coverage.failures[0].contains("not a valid GitHub login"),
                "{login}"
            );
            assert_eq!(
                fixture.hits(&format!("/api/orgs/{login}/repos")),
                0,
                "{login}"
            );
        }
    }

    #[test]
    fn valid_repository_names_preserve_organization_coverage() {
        let fixture = organization_fixture();
        let names = [
            String::from("o/.github"),
            String::from("o/sdk.v2"),
            format!("o/{}", "r".repeat(80)),
            String::from("o/COM1"),
            String::from("o/trailing."),
        ];
        let entries: Vec<_> = names.iter().map(|name| entry(name)).collect();
        fixture.page_route(
            "/api/orgs/o/repos",
            vec![listing(&entries), (200, b"[]".to_vec())],
        );
        empty_pins(&fixture, &names.each_ref().map(String::as_str));
        let cache = tempfile::tempdir().unwrap();
        let corpus = load_with_bases("o", fixture.api_base().as_str(), cache.path(), false);
        assert!(
            corpus.report.coverage.complete,
            "{:?}",
            corpus.report.coverage.failures
        );
        assert!(
            corpus
                .report
                .repositories
                .iter()
                .map(|repo| &repo.name)
                .eq(names.iter())
        );
    }

    #[test]
    fn invalid_repository_entries_are_rejected_with_failures() {
        let fixture = organization_fixture();
        let page_one = vec![
            serde_json::json!({ "full_name": "nodash", "default_branch": "main" }),
            serde_json::json!({ "full_name": "o/a/b", "default_branch": "main" }),
            serde_json::json!({ "full_name": "other/r1", "default_branch": "main" }),
            entry("o/r1"),
            entry("o/r2"),
            entry("o/."),
            entry("o/.."),
            entry("o/bad name"),
            entry(&format!("o/{}", "r".repeat(101))),
        ];
        fixture.page_route(
            "/api/orgs/o/repos",
            vec![listing(&page_one), (200, b"[]".to_vec())],
        );
        empty_pins(&fixture, &["o/r1", "o/r2"]);
        let dir = tempfile::tempdir().unwrap();
        let corpus = load_with_bases("o", fixture.api_base().as_str(), dir.path(), false);
        let names: Vec<&str> = corpus
            .report
            .repositories
            .iter()
            .map(|repo| repo.name.as_str())
            .collect();
        assert_eq!(names, ["o/r1", "o/r2"]);
        assert!(!corpus.report.coverage.complete);
        let failures = corpus.report.coverage.failures.join("\n");
        assert_eq!(failures.matches("has an invalid full_name").count(), 6);
        assert!(failures.contains("names another owner"));
    }

    #[test]
    fn pin_refresh_reuses_the_immutable_commit_cache() {
        let fixture = organization_fixture();
        let sha_a = "a".repeat(40);
        let sha_b = "b".repeat(40);
        fixture.page_route(
            "/api/orgs/o/repos",
            vec![listing(&[entry("o/app")]), (200, b"[]".to_vec())],
        );
        fixture.page_route(
            "/api/repos/o/app/git/ref/heads/main",
            vec![pin_response(&sha_a), pin_response(&sha_b)],
        );
        fixture.route(
            &format!("/api/repos/o/app/tarball/{sha_a}"),
            200,
            &snapshot_archive("o-app-a000000"),
        );
        fixture.route(
            &format!("/api/repos/o/app/tarball/{sha_b}"),
            200,
            &snapshot_archive("o-app-b000000"),
        );
        let cache = tempfile::tempdir().unwrap();
        let first = load_with_bases("o", fixture.api_base().as_str(), cache.path(), false);
        assert_eq!(
            first.report.repositories[0].commit.as_deref(),
            Some(sha_a.as_str())
        );
        let root_a = first.roots[0].as_ref().expect("the first snapshot");
        assert!(root_a.ends_with(Path::new(&sha_a).join(SNAPSHOT_TREE)));
        assert!(root_a.join("readme").exists());
        assert!(first.report.coverage.complete);
        assert_eq!(
            fixture.hits(&format!("/api/repos/o/app/tarball/{sha_a}")),
            1
        );
        fixture.page_route(
            "/api/orgs/o/repos",
            vec![listing(&[entry("o/app")]), (200, b"[]".to_vec())],
        );
        let second = load_with_bases("o", fixture.api_base().as_str(), cache.path(), false);
        assert_eq!(
            second.report.repositories[0].commit.as_deref(),
            Some(sha_b.as_str())
        );
        let root_b = second.roots[0].as_ref().expect("the second snapshot");
        assert_ne!(root_a, root_b);
        assert!(root_b.ends_with(Path::new(&sha_b).join(SNAPSHOT_TREE)));
        assert_eq!(
            fixture.hits(&format!("/api/repos/o/app/tarball/{sha_a}")),
            1
        );
        assert_eq!(
            fixture.hits(&format!("/api/repos/o/app/tarball/{sha_b}")),
            1
        );
        assert_eq!(fixture.hits("/api/repos/o/app/git/ref/heads/main"), 2);
        assert!(second.report.coverage.complete);
    }

    #[cfg(unix)]
    #[test]
    fn a_skipped_link_is_reported_on_every_run() {
        let fixture = organization_fixture();
        let sha = "e".repeat(40);
        fixture.route(
            "/api/repos/o/app/git/ref/heads/main",
            200,
            &serde_json::to_vec(&serde_json::json!({ "object": { "sha": sha } })).unwrap(),
        );
        fixture.route(
            &format!("/api/repos/o/app/tarball/{sha}"),
            200,
            &packed(vec![
                (b'5', "o-app-e000000/".into(), String::new(), Vec::new()),
                (
                    0,
                    "o-app-e000000/Cargo.toml".into(),
                    String::new(),
                    b"[package]\nname = \"good\"\nversion = \"0.1.0\"\nedition = \"2024\"\n"
                        .to_vec(),
                ),
                (
                    0,
                    "o-app-e000000/src/lib.rs".into(),
                    String::new(),
                    b"pub fn good() {}\n".to_vec(),
                ),
                (
                    b'2',
                    "o-app-e000000/dangling".into(),
                    "missing".to_owned(),
                    Vec::new(),
                ),
            ]),
        );
        let cache = tempfile::tempdir().unwrap();
        for run in ["first", "cached"] {
            fixture.page_route(
                "/api/orgs/o/repos",
                vec![listing(&[entry("o/app")]), (200, b"[]".to_vec())],
            );
            let corpus = load_with_bases("o", fixture.api_base().as_str(), cache.path(), false);
            assert_eq!(
                corpus.report.coverage.failures,
                ["o/app: archive link \"dangling\" cannot be resolved"],
                "{run}"
            );
            assert!(!corpus.report.coverage.complete, "{run}");
            assert_eq!(corpus.report.packages.len(), 1, "{run}");
            assert_eq!(corpus.report.packages[0].name, "good", "{run}");
        }
        assert_eq!(fixture.hits(&format!("/api/repos/o/app/tarball/{sha}")), 1);
    }

    #[test]
    fn a_failed_archive_keeps_the_other_repositories() {
        let fixture = organization_fixture();
        let sha_bad = "a".repeat(40);
        let sha_good = "b".repeat(40);
        fixture.page_route(
            "/api/orgs/o/repos",
            vec![
                listing(&[entry("o/bad"), entry("o/good")]),
                (200, b"[]".to_vec()),
            ],
        );
        fixture.route(
            "/api/repos/o/bad/git/ref/heads/main",
            200,
            &serde_json::to_vec(&serde_json::json!({ "object": { "sha": sha_bad } })).unwrap(),
        );
        fixture.route(
            "/api/repos/o/good/git/ref/heads/main",
            200,
            &serde_json::to_vec(&serde_json::json!({ "object": { "sha": sha_good } })).unwrap(),
        );
        fixture.route(&format!("/api/repos/o/bad/tarball/{sha_bad}"), 404, b"");
        fixture.route(
            &format!("/api/repos/o/good/tarball/{sha_good}"),
            200,
            &package_archive("o-good-b000000"),
        );
        let cache = tempfile::tempdir().unwrap();
        let corpus = load_with_bases("o", fixture.api_base().as_str(), cache.path(), false);
        assert_eq!(corpus.report.repositories.len(), 2);
        assert_eq!(
            corpus.report.repositories[0].commit.as_deref(),
            Some(sha_bad.as_str())
        );
        assert!(corpus.roots[0].is_none());
        assert_eq!(
            corpus.report.repositories[1].commit.as_deref(),
            Some(sha_good.as_str())
        );
        assert!(corpus.roots[1].is_some());
        assert!(!corpus.report.coverage.complete);
        assert_eq!(corpus.report.coverage.failures.len(), 1);
        assert!(corpus.report.coverage.failures[0].starts_with("o/bad: "));
        assert_eq!(corpus.report.packages.len(), 1);
        assert_eq!(corpus.report.packages[0].name, "good");
        assert_eq!(corpus.report.packages[0].manifest, "Cargo.toml");
        assert_eq!(corpus.report.packages[0].repository, 1);
        assert_eq!(corpus.packages.len(), corpus.report.packages.len());
        assert_eq!(corpus.packages[0].package, 0);
        let target = corpus.packages[0]
            .targets
            .iter()
            .find(|target| target.library)
            .expect("the lib target");
        assert_eq!(target.name, "good");
        assert!(target.doc);
        assert!(target.src.ends_with("src/lib.rs"));
    }

    #[test]
    fn a_redirected_archive_never_sees_the_credential() {
        let fixture = organization_fixture();
        let sha = "c".repeat(40);
        fixture.page_route(
            "/api/orgs/o/repos",
            vec![listing(&[entry("o/app")]), (200, b"[]".to_vec())],
        );
        fixture.route(
            "/api/repos/o/app/git/ref/heads/main",
            200,
            &serde_json::to_vec(&serde_json::json!({ "object": { "sha": sha } })).unwrap(),
        );
        let location = format!("{}/payload?sig=signed-url", fixture.raw_base().as_str());
        fixture.redirect_route(&format!("/api/repos/o/app/tarball/{sha}"), 302, &location);
        fixture.route("/raw/payload", 200, &snapshot_archive("o-app-c000000"));
        let dir = tempfile::tempdir().unwrap();
        let corpus = load_with_bases("o", fixture.api_base().as_str(), dir.path(), false);
        assert!(corpus.roots[0].is_some());
        assert!(corpus.report.coverage.complete);
        assert_eq!(fixture.last_auth("/raw/payload"), None);
        assert_eq!(
            fixture.last_target("/raw/payload").as_deref(),
            Some("/raw/payload?sig=signed-url")
        );
        assert_eq!(
            fixture.last_auth(&format!("/api/repos/o/app/tarball/{sha}")),
            None
        );
    }

    #[test]
    #[cfg(unix)]
    fn the_organization_cache_namespace_is_owner_only() {
        use std::os::unix::fs::PermissionsExt;
        let fixture = organization_fixture();
        let sha = "d".repeat(40);
        fixture.page_route(
            "/api/orgs/o/repos",
            vec![listing(&[entry("o/app")]), (200, b"[]".to_vec())],
        );
        fixture.route(
            "/api/repos/o/app/git/ref/heads/main",
            200,
            &serde_json::to_vec(&serde_json::json!({ "object": { "sha": sha } })).unwrap(),
        );
        fixture.route(
            &format!("/api/repos/o/app/tarball/{sha}"),
            200,
            &snapshot_archive("o-app-d000000"),
        );
        let cache = tempfile::tempdir().unwrap();
        let corpus = load_with_bases("o", fixture.api_base().as_str(), cache.path(), false);
        assert!(corpus.report.coverage.complete);
        let base = cache.path().join("organization");
        let snapshot = corpus.roots[0].as_ref().expect("the cached snapshot");
        let commit = snapshot.parent().unwrap();
        for dir in [base.as_path(), commit.parent().unwrap(), commit] {
            assert_eq!(
                std::fs::metadata(dir).unwrap().permissions().mode() & 0o777,
                0o700,
                "the namespace stays owner-only in {dir:?}"
            );
        }
    }

    #[test]
    fn a_pin_without_a_full_commit_is_a_repository_failure() {
        for sha in [String::from("abc"), "g".repeat(40)] {
            let fixture = organization_fixture();
            fixture.page_route("/api/orgs/o/repos", vec![listing(&[entry("o/app")])]);
            fixture.route(
                "/api/repos/o/app/git/ref/heads/main",
                200,
                &pin_response(&sha).1,
            );
            let cache = tempfile::tempdir().unwrap();
            let corpus = load_with_bases("o", fixture.api_base().as_str(), cache.path(), false);
            assert!(
                corpus.report.coverage.failures[0].contains("has no full commit identifier"),
                "{sha}"
            );
            assert_eq!(corpus.report.repositories[0].commit, None, "{sha}");
            assert_eq!(fixture.hits(&format!("/api/repos/o/app/tarball/{sha}")), 0);
        }
    }

    #[test]
    fn a_losing_publisher_keeps_the_winner_and_a_blocking_file_fails() {
        let dir = tempfile::tempdir().unwrap();
        let archive = dir.path().join("snapshot.tar.gz");
        std::fs::write(&archive, package_archive("o-app-f000000")).unwrap();
        let winner = dir.path().join("won");
        std::fs::create_dir_all(winner.join(SNAPSHOT_TREE)).unwrap();
        std::fs::write(winner.join(LINK_REPORT), "earlier\n").unwrap();
        publish(&archive, dir.path(), &winner).unwrap();
        assert_eq!(
            std::fs::read_to_string(winner.join(LINK_REPORT)).unwrap(),
            "earlier\n"
        );
        let blocked = dir.path().join("blocked");
        std::fs::write(&blocked, "a file").unwrap();
        assert!(matches!(
            publish(&archive, dir.path(), &blocked),
            Err(SourceError::Storage(_))
        ));
        assert!(std::fs::read_dir(dir.path()).unwrap().all(|entry| {
            !entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with(".stage-")
        }));
    }

    #[cfg(unix)]
    #[test]
    fn an_archive_kept_after_a_failed_extraction_stays_owner_only() {
        use std::os::unix::fs::PermissionsExt;
        let fixture = organization_fixture();
        let sha = "f".repeat(40);
        fixture.page_route("/api/orgs/o/repos", vec![listing(&[entry("o/app")])]);
        fixture.route(
            "/api/repos/o/app/git/ref/heads/main",
            200,
            &pin_response(&sha).1,
        );
        fixture.route(
            &format!("/api/repos/o/app/tarball/{sha}"),
            200,
            b"not a gzip stream",
        );
        let cache = tempfile::tempdir().unwrap();
        let corpus = load_with_bases("o", fixture.api_base().as_str(), cache.path(), false);
        assert!(!corpus.report.coverage.complete);
        let archive = private_base(cache.path(), "o/app")
            .unwrap()
            .join(format!("{sha}.tar.gz"));
        assert_eq!(
            std::fs::metadata(archive).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }

    #[test]
    fn an_unreachable_token_owner_leaves_the_public_listing_incomplete() {
        let fixture = Fixture::start();
        account(&fixture, "u", "User");
        fixture.short_route("/api/user", 64);
        fixture.page_route("/api/users/u/repos", vec![listing(&[entry("u/app")])]);
        empty_pins(&fixture, &["u/app"]);
        let dir = tempfile::tempdir().unwrap();
        let corpus = load_with_bases("u", fixture.api_base().as_str(), dir.path(), false);
        assert_eq!(corpus.report.coverage.failures.len(), 1);
        assert!(corpus.report.coverage.failures[0].starts_with("u: "));
        assert_eq!(names(&corpus), ["u/app"]);
    }

    #[test]
    fn unusable_listing_pages_are_recorded() {
        let malformed = serde_json::to_vec(&[serde_json::json!({ "default_branch": "main" })]);
        for (setup, expected) in [
            (Some(b"not json".to_vec()), "is not a JSON array"),
            (Some(b"{}".to_vec()), "is not a JSON array"),
            (Some(malformed.unwrap()), "entry 0 has no full_name"),
            (None, "o: "),
        ] {
            let fixture = organization_fixture();
            match setup {
                Some(body) => fixture.route("/api/orgs/o/repos", 200, &body),
                None => fixture.short_route("/api/orgs/o/repos", 64),
            }
            let dir = tempfile::tempdir().unwrap();
            let corpus = load_with_bases("o", fixture.api_base().as_str(), dir.path(), false);
            assert_eq!(corpus.report.repositories.len(), 0, "{expected}");
            assert_eq!(corpus.report.coverage.failures.len(), 1, "{expected}");
            assert!(
                corpus.report.coverage.failures[0].starts_with("o: ")
                    && corpus.report.coverage.failures[0].contains(expected),
                "{:?}",
                corpus.report.coverage.failures
            );
        }
    }

    #[test]
    fn a_next_page_outside_the_api_is_not_followed() {
        let fixture = organization_fixture();
        let page = serde_json::to_vec(&[entry("o/app")]).unwrap();
        fixture.next_route(
            "/api/orgs/o/repos",
            &page,
            "http://evil.example/api/orgs/o/repos",
        );
        empty_pins(&fixture, &["o/app"]);
        let dir = tempfile::tempdir().unwrap();
        let corpus = load_with_bases("o", fixture.api_base().as_str(), dir.path(), false);
        assert_eq!(names(&corpus), ["o/app"]);
        assert_eq!(corpus.report.coverage.failures.len(), 1);
        assert!(
            corpus.report.coverage.failures[0]
                .ends_with("next page http://evil.example/api/orgs/o/repos leaves the API")
        );
    }

    #[test]
    fn a_listing_past_the_page_cap_is_incomplete() {
        let fixture = organization_fixture();
        let api = fixture.api_base();
        let page = serde_json::to_vec(&[entry("o/app")]).unwrap();
        fixture.next_route("/api/orgs/o/repos", &page, &format!("{api}/orgs/o/repos"));
        let mut repos = Vec::new();
        let mut failures = Vec::new();
        enumerate_repos(&api, "o", 2, &mut repos, &mut failures);
        assert_eq!(fixture.hits("/api/orgs/o/repos"), 2);
        assert_eq!(repos.len(), 1);
        assert_eq!(failures, ["o: the listing exceeds 2 pages"]);
    }

    #[test]
    fn a_repository_without_a_default_branch_is_listed_unpinned() {
        let fixture = organization_fixture();
        fixture.page_route(
            "/api/orgs/o/repos",
            vec![listing(&[serde_json::json!({ "full_name": "o/app" })])],
        );
        let dir = tempfile::tempdir().unwrap();
        let corpus = load_with_bases("o", fixture.api_base().as_str(), dir.path(), false);
        assert_eq!(names(&corpus), ["o/app"]);
        assert_eq!(
            corpus.report.coverage.failures,
            ["o/app: the default branch is unknown"]
        );
        assert_eq!(corpus.roots, [None]);
    }

    #[test]
    fn a_pin_failure_other_than_an_empty_repository_is_recorded() {
        for status in [409, 500] {
            let fixture = organization_fixture();
            fixture.page_route("/api/orgs/o/repos", vec![listing(&[entry("o/app")])]);
            fixture.route(
                "/api/repos/o/app/git/ref/heads/main",
                status,
                b"{\"message\": \"Conflict\"}",
            );
            let dir = tempfile::tempdir().unwrap();
            let corpus = load_with_bases("o", fixture.api_base().as_str(), dir.path(), false);
            let failures = &corpus.report.coverage.failures;
            assert_eq!(failures.len(), 1, "{status}");
            assert!(failures[0].starts_with("o/app: "), "{failures:?}");
            assert!(
                failures[0].ends_with(&format!("HTTP {status}")),
                "{failures:?}"
            );
        }
    }

    /// A fixture listing `o/app` pinned at `sha`, its tarball answering `tarball`.
    fn pinned_fixture(sha: &str, tarball: (u16, &[u8])) -> Fixture {
        let fixture = organization_fixture();
        fixture.route("/api/orgs/o/repos", 200, &listing(&[entry("o/app")]).1);
        fixture.route(
            "/api/repos/o/app/git/ref/heads/main",
            200,
            &pin_response(sha).1,
        );
        fixture.route(
            &format!("/api/repos/o/app/tarball/{sha}"),
            tarball.0,
            tarball.1,
        );
        fixture
    }

    #[test]
    fn a_kept_archive_is_extracted_without_a_second_download() {
        let sha = "e".repeat(40);
        let fixture = pinned_fixture(&sha, (200, b"not a gzip stream"));
        let cache = tempfile::tempdir().unwrap();
        let api = fixture.api_base();
        load_with_bases("o", &api, cache.path(), false);
        let archive = private_base(cache.path(), "o/app")
            .unwrap()
            .join(format!("{sha}.tar.gz"));
        std::fs::write(&archive, package_archive("o-app-e")).unwrap();
        let corpus = load_with_bases("o", &api, cache.path(), false);
        assert_eq!(corpus.report.coverage.failures, Vec::<String>::new());
        assert_eq!(fixture.hits(&format!("/api/repos/o/app/tarball/{sha}")), 1);
        assert_eq!(corpus.report.packages.len(), 1);
        assert!(!archive.exists());
    }

    #[test]
    fn a_published_commit_without_its_tree_is_a_storage_failure() {
        let sha = "d".repeat(40);
        let fixture = pinned_fixture(&sha, (500, b""));
        let cache = tempfile::tempdir().unwrap();
        let commit = private_base(cache.path(), "o/app").unwrap().join(&sha);
        std::fs::create_dir(&commit).unwrap();
        std::fs::write(commit.join(LINK_REPORT), "").unwrap();
        let corpus = load_with_bases("o", fixture.api_base().as_str(), cache.path(), false);
        assert_eq!(corpus.report.coverage.failures.len(), 1);
        assert!(corpus.report.coverage.failures[0].starts_with("o/app: "));
        assert_eq!(corpus.roots, [None]);
        assert_eq!(
            corpus.report.repositories[0].commit.as_deref(),
            Some(sha.as_str())
        );
        assert_eq!(fixture.hits(&format!("/api/repos/o/app/tarball/{sha}")), 0);
    }

    #[test]
    fn a_snapshot_discovery_failure_is_qualified_by_the_repository() {
        let sha = "c".repeat(40);
        let archive = packed(vec![
            (b'5', "o-app-c/".into(), String::new(), Vec::new()),
            (
                0,
                "o-app-c/Cargo.toml".into(),
                String::new(),
                b"[package\n".to_vec(),
            ),
        ]);
        let fixture = pinned_fixture(&sha, (200, &archive));
        let cache = tempfile::tempdir().unwrap();
        let corpus = load_with_bases("o", fixture.api_base().as_str(), cache.path(), false);
        assert_eq!(corpus.report.coverage.failures.len(), 1);
        assert!(
            corpus.report.coverage.failures[0].starts_with("o/app: Cargo.toml: "),
            "{:?}",
            corpus.report.coverage.failures
        );
        assert_eq!(
            corpus.report.packages,
            Vec::<crate::organization::OrganizationPackage>::new()
        );
        assert!(corpus.roots[0].is_some());
    }
}
