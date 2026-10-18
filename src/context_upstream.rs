//! Snapshot-verified upstream permalinks and exact-range issue lookup without publication.

use alloc::collections::BTreeMap;
use alloc::collections::btree_map::Entry;
use alloc::format;
use alloc::string::String;
use alloc::string::ToString;
use alloc::vec::Vec;
use std::fs;
use std::path::{Path, PathBuf};

use crate::context_network;
use crate::reuse::{DependencyCandidate, ExistingIssue, PackageSource, ReuseAction};

/// The title tag of extraction requests.
const ISSUE_TAG: &str = "[DEJADOC]";

/// The issue search page size.
const SEARCH_PER_PAGE: usize = 100;

/// The issue search page cap.
const MAX_PAGES: u32 = 10;

/// The warning for an incomplete issue search.
const TRUNCATED_WARNING: &str = "issue search was incomplete. prior requests may be unchecked";

/// The hosts that may carry a declared repository.
const GITHUB_HOSTS: &[&str] = &["github.com", "www.github.com"];

/// The verified commit and package path from a `.cargo_vcs_info.json`.
struct VcsInfo {
    sha: String,
    path_in_vcs: Option<String>,
}

/// A memoized read-only response.
struct Response {
    status: u16,
    body: Vec<u8>,
    rate: Option<u64>,
}

/// A borrowed view of a memoized response.
struct View<'a> {
    status: u16,
    body: &'a [u8],
    rate: Option<u64>,
}

/// A memoized client with immutable provenance persisted under `private`.
pub(crate) struct Upstream {
    cache_dir: PathBuf,
    api: String,
    raw: String,
    memo: BTreeMap<String, Result<Response, String>>,
    source: BTreeMap<String, Result<(u16, Option<String>), String>>,
}

impl Upstream {
    /// A client pointed at the production GitHub API and raw hosts.
    #[must_use]
    pub fn new(cache_dir: &Path) -> Self {
        Self::with_base(
            cache_dir,
            "https://api.github.com",
            "https://raw.githubusercontent.com",
        )
    }

    /// A client pointed at alternate API and raw bases for tests and
    /// mirrors; the links it produces always name the real `github.com`.
    #[must_use]
    pub fn with_base(cache_dir: &Path, api_base: &str, raw_base: &str) -> Self {
        Self {
            cache_dir: cache_dir.to_path_buf(),
            api: api_base.trim_end_matches('/').to_string(),
            raw: raw_base.trim_end_matches('/').to_string(),
            memo: BTreeMap::new(),
            source: BTreeMap::new(),
        }
    }

    /// Enrich candidates with verified links and extraction requests, returning unresolved conditions.
    pub fn enrich(
        &mut self,
        package: &PackageSource,
        candidates: &mut [&mut DependencyCandidate],
    ) -> Vec<String> {
        let mut conditions: Vec<String> = Vec::new();
        for slot in candidates.iter_mut() {
            let candidate = &mut **slot;
            if let Some(condition) = self.enrich_one(package, candidate) {
                conditions.push(condition.clone());
                candidate.lookup_warning = Some(condition);
            }
        }
        conditions.sort();
        conditions.dedup();
        conditions
    }

    /// The verified upstream commit for `package`, preferring local
    /// evidence, then persisted immutable provenance.
    fn commit_for(&mut self, package: &PackageSource, repo: &str) -> Option<String> {
        let local = vcs_commit(&package.root).or_else(|| pinned_commit(&package.source));
        if let Some(sha) = local {
            self.persist_provenance(package, sha.clone());
            return Some(sha);
        }
        self.read_provenance(package)
            .or_else(|| self.resolve_tag_commit(repo, &package.version))
    }

    fn enrich_one(
        &mut self,
        package: &PackageSource,
        candidate: &mut DependencyCandidate,
    ) -> Option<String> {
        let repo = if let Some(repo) = package
            .repository
            .as_deref()
            .and_then(normalize_github_repository)
        {
            candidate.repository = Some(repo.clone());
            repo
        } else {
            if let Some(declared) = package.repository.as_deref() {
                candidate.repository = Some(declared.to_string());
                return Some("upstream repository is not on GitHub".to_string());
            }
            return Some("upstream repository not declared by the package".to_string());
        };
        let Some(commit) = self.commit_for(package, &repo) else {
            return Some(format!(
                "no verified upstream commit for {} {}",
                package.name, package.version
            ));
        };
        let file = candidate.site.file.clone();
        let Some(fragment) = range_fragment(
            candidate.site.line,
            candidate.site.end,
            candidate.site.end_column,
        ) else {
            return Some(format!("upstream source range of {file} is empty"));
        };
        let Some(expected) = package.source_hashes.get(&file) else {
            return Some(format!("no indexed source snapshot for {file}"));
        };
        let prefix = if package.source.starts_with("git+") {
            match git_checkout_prefix(package, &repo) {
                Some(Ok(prefix)) => Some(prefix),
                Some(Err(reason)) => return Some(reason),
                None => None,
            }
        } else {
            vcs_info(&package.root).and_then(|info| info.path_in_vcs)
        };
        let path_in_vcs = prefix.map(|prefix| prefix.trim_start_matches('/').to_string());
        let repo_file = match path_in_vcs {
            Some(prefix) if !prefix.is_empty() => format!("{prefix}/{file}"),
            _ => file.clone(),
        };
        if repo_file.starts_with('/') || repo_file.split('/').any(|segment| segment == "..") {
            return Some(format!(
                "upstream source path of {file} escapes the repository"
            ));
        }
        let path = encode_path(&repo_file);
        let raw_url = format!("{}/{}/{commit}/{path}", self.raw, repo);
        let (status, fetched) = match self.source_hash(&raw_url) {
            Ok(value) => value,
            Err(err) => {
                let message = if context_network::is_oversize(&err) {
                    format!("upstream source {file} at {commit} exceeds the read bound")
                } else {
                    format!("upstream source {file} at {commit} {err}")
                };
                return Some(message);
            }
        };
        if status == 404 {
            return Some(format!("upstream source {file} is not at commit {commit}"));
        }
        if !context_network::is_success(status) {
            return Some(format!("upstream source {file} at {commit} HTTP {status}"));
        }
        if fetched.as_deref() != Some(expected.as_str()) {
            return Some(format!(
                "upstream source {file} at {commit} differs from the indexed snapshot"
            ));
        }
        let permalink = format!("https://github.com/{repo}/blob/{commit}/{path}{fragment}");
        candidate.source_url = Some(permalink.clone());
        if candidate.action != ReuseAction::RequestExtraction {
            return None;
        }
        self.lookup(&repo, &permalink, candidate)
    }

    /// A memoized bounded GET of a JSON or small body, caching both success
    /// and transport failures.
    fn get(&mut self, url: &str) -> Result<View<'_>, String> {
        let stored = match self.memo.entry(url.to_string()) {
            Entry::Occupied(entry) => entry.into_mut(),
            Entry::Vacant(entry) => entry.insert(
                context_network::get_raw(
                    url,
                    context_network::READ_LIMIT,
                    context_network::REQUEST_DEADLINE,
                )
                .map(|(status, body, _id, rate)| Response { status, body, rate }),
            ),
        };
        match stored {
            Ok(response) => Ok(View {
                status: response.status,
                body: response.body.as_slice(),
                rate: response.rate,
            }),
            Err(err) => Err(err.clone()),
        }
    }

    /// A memoized bounded read of a source file, returning the status and the
    /// blake3 hex of the body, which is `None` unless the status is a success.
    fn source_hash(&mut self, url: &str) -> Result<(u16, Option<String>), String> {
        if let Some(hit) = self.source.get(url) {
            return hit.clone();
        }
        let result = match context_network::get_raw(
            url,
            context_network::SOURCE_LIMIT,
            context_network::REQUEST_DEADLINE,
        ) {
            Err(err) => Err(err),
            Ok((status, body, _id, _rate)) => {
                let hash = context_network::is_success(status)
                    .then(|| blake3::hash(&body).to_hex().to_string());
                Ok((status, hash))
            }
        };
        self.source.insert(url.to_string(), result.clone());
        result
    }

    /// Search tagged open and closed issues for the exact permalink, setting
    /// the match or the prefilled request URL on `candidate`.
    fn lookup(
        &mut self,
        repo: &str,
        permalink: &str,
        candidate: &mut DependencyCandidate,
    ) -> Option<String> {
        let query = format!("repo:{repo} in:title \"{ISSUE_TAG}\" is:issue");
        let mut page = 1u32;
        let mut incomplete = false;
        let mut unavailable = None;
        'search: loop {
            let url = format!(
                "{}/search/issues?q={}&per_page={SEARCH_PER_PAGE}&page={page}",
                self.api,
                percent_encode(&query)
            );
            let View { status, body, rate } = match self.get(&url) {
                Ok(response) => response,
                Err(err) => {
                    unavailable = Some(format!("issue lookup unavailable {err}"));
                    break;
                }
            };
            if status == 429 || (status == 403 && rate == Some(0)) {
                unavailable = Some("issue lookup unavailable GitHub rate limit".to_string());
                break;
            }
            if status == 403 {
                unavailable = Some("issue lookup unavailable GitHub permission denied".to_string());
                break;
            }
            if !context_network::is_success(status) {
                unavailable = Some(format!("issue lookup unavailable HTTP {status}"));
                break;
            }
            let Ok(value) = serde_json::from_slice::<serde_json::Value>(body) else {
                unavailable = Some("issue lookup unavailable invalid JSON".to_string());
                break;
            };
            if value
                .get("incomplete_results")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false)
            {
                incomplete = true;
            }
            let Some(items) = value.get("items").and_then(serde_json::Value::as_array) else {
                unavailable = Some("issue search results were unreadable".to_string());
                break;
            };
            for item in items {
                if item.get("pull_request").is_some() {
                    continue;
                }
                let Some(title) = item.get("title").and_then(serde_json::Value::as_str) else {
                    continue;
                };
                if !title.contains(ISSUE_TAG) {
                    continue;
                }
                let Some(body_text) = item.get("body").and_then(serde_json::Value::as_str) else {
                    continue;
                };
                if !body_contains_permalink(body_text, permalink) {
                    continue;
                }
                let Some(html_url) = item.get("html_url").and_then(serde_json::Value::as_str)
                else {
                    continue;
                };
                let state = item
                    .get("state")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("unknown");
                let issue = ExistingIssue {
                    url: html_url.to_string(),
                    state: state.to_string(),
                };
                if issue.state == "open" {
                    candidate.existing_issue = Some(issue);
                    break 'search;
                }
                if candidate.existing_issue.is_none() {
                    candidate.existing_issue = Some(issue);
                }
            }
            if items.len() < SEARCH_PER_PAGE {
                break;
            }
            page += 1;
            if page > MAX_PAGES {
                incomplete = true;
                break;
            }
        }
        if candidate.existing_issue.is_some() {
            candidate.issue_url = None;
        } else {
            candidate.issue_url = Some(request_url(repo, &candidate.site.item, permalink));
        }
        unavailable.or_else(|| incomplete.then(|| TRUNCATED_WARNING.to_string()))
    }

    /// The persistent provenance file, under the cache's private subtree.
    fn provenance_path(&self) -> PathBuf {
        self.cache_dir
            .join("private")
            .join("upstream")
            .join("provenance.json")
    }

    fn provenance_key(package: &PackageSource) -> String {
        format!("{}@{}|{}", package.name, package.version, package.source)
    }

    /// Record a locally established commit for later runs, best effort.
    fn persist_provenance(&self, package: &PackageSource, sha: String) {
        let path = self.provenance_path();
        let mut map: BTreeMap<String, String> = fs::read_to_string(&path)
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default();
        map.insert(Self::provenance_key(package), sha);
        let text = serde_json::to_string(&map).expect("a string map always serializes");
        if let Some(parent) = path.parent() {
            let _ = fs::create_dir_all(parent);
        }
        let part = path.with_file_name(format!(
            "provenance.json.part-{}",
            context_network::next_id()
        ));
        if fs::write(&part, text).is_err() {
            return;
        }
        if fs::rename(&part, path).is_err() {
            let _ = fs::remove_file(&part);
        }
    }

    /// A previously persisted immutable commit for `package`.
    fn read_provenance(&self, package: &PackageSource) -> Option<String> {
        fs::read_to_string(self.provenance_path())
            .ok()
            .and_then(|text| serde_json::from_str::<BTreeMap<String, String>>(&text).ok())
            .and_then(|map| map.get(&Self::provenance_key(package)).cloned())
    }

    /// The commit behind a version tag, tried as `v{version}` then
    /// `{version}`.
    fn resolve_tag_commit(&mut self, repo: &str, version: &str) -> Option<String> {
        let tags = [format!("v{version}"), version.to_string()];
        for tag in tags {
            if let Some(commit) = self.tag_commit(repo, &tag) {
                return Some(commit);
            }
        }
        None
    }

    fn tag_commit(&mut self, repo: &str, tag: &str) -> Option<String> {
        let url = format!(
            "{}/repos/{repo}/git/ref/tags/{}",
            self.api,
            encode_path(tag)
        );
        let View {
            status,
            body,
            rate: _,
        } = self.get(&url).ok()?;
        if !context_network::is_success(status) {
            return None;
        }
        let value: serde_json::Value = serde_json::from_slice(body).ok()?;
        let object = value.get("object")?;
        let sha = object.get("sha")?.as_str()?.to_string();
        match object.get("type")?.as_str()? {
            "commit" => Some(sha),
            "tag" => {
                let url = format!("{}/repos/{repo}/git/tags/{sha}", self.api);
                let View {
                    status,
                    body,
                    rate: _,
                } = self.get(&url).ok()?;
                if !context_network::is_success(status) {
                    return None;
                }
                let value: serde_json::Value = serde_json::from_slice(body).ok()?;
                let commit = value.get("object")?.get("sha")?.as_str()?.to_string();
                Some(commit)
            }
            _ => None,
        }
    }
}

/// The lowercased `owner/repo` when `value` declares a GitHub repository, in
/// any accepted URL form.
#[must_use]
pub(crate) fn normalize_github_repository(value: &str) -> Option<String> {
    let value = value.strip_prefix("git+").unwrap_or(value);
    let (host, path) = if value.contains("://") {
        let rest = value.split_once("://")?.1;
        let (authority, path) = rest.split_once('/')?;
        (authority_host(authority)?, path)
    } else if let Some((user_at_host, path)) = value.split_once(':') {
        let host = user_at_host.rsplit('@').next()?;
        (host.to_ascii_lowercase().clone(), path)
    } else {
        return None;
    };
    if !GITHUB_HOSTS.contains(&host.as_str()) {
        return None;
    }
    github_owner_repo(path)
}

/// The lowercased host of an `authority` string without a port.
fn authority_host(authority: &str) -> Option<String> {
    let authority = authority.rsplit('@').next()?;
    let host = if let Some(host) = authority.strip_prefix('[') {
        host.split(']').next()?
    } else {
        authority
            .split_once(':')
            .filter(|(_, port)| port.parse::<u16>().is_ok())
            .map_or(authority, |(host, _)| host)
    };
    Some(host.to_ascii_lowercase())
}

/// The `owner/repo` of a GitHub repository path, trimmed of a `.git` suffix.
fn github_owner_repo(path: &str) -> Option<String> {
    let path = path.strip_prefix('/').unwrap_or(path);
    let path = path.split(['?', '#']).next().unwrap_or(path);
    let path = path.strip_suffix(".git").unwrap_or(path);
    let path = path.strip_suffix('/').unwrap_or(path);
    let (owner, repo) = path.split_once('/')?;
    (!owner.is_empty() && !repo.is_empty()).then(|| format!("{owner}/{repo}").to_ascii_lowercase())
}

/// The `#L{start}-L{end}` permalink fragment, honoring the exclusive
/// `end_column`, or `None` for an empty range.
#[must_use]
pub(crate) fn range_fragment(line: u32, end: u32, end_column: usize) -> Option<String> {
    let last = if end_column > 1 {
        end
    } else {
        end.saturating_sub(1)
    };
    (last >= line).then(|| format!("#L{line}-L{last}"))
}

/// The percent-encoding of `value`, leaving unreserved bytes bare.
fn percent_encode(value: &str) -> String {
    encode(value, false)
}

fn encode(value: &str, preserve_slashes: bool) -> String {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    let mut out = String::with_capacity(value.len());
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric()
            || matches!(byte, b'-' | b'.' | b'_' | b'~')
            || (preserve_slashes && byte == b'/')
        {
            out.push(char::from(byte));
        } else {
            out.push('%');
            out.push(char::from(HEX[usize::from(byte >> 4)]));
            out.push(char::from(HEX[usize::from(byte & 15)]));
        }
    }
    out
}

/// The percent-encoded path segments of `path`, joined with `/`.
#[must_use]
fn encode_path(path: &str) -> String {
    encode(path, true)
}

/// Whether `body` contains `permalink` as a complete URL token.
#[must_use]
fn body_contains_permalink(body: &str, permalink: &str) -> bool {
    let bytes = body.as_bytes();
    let plen = permalink.len();
    let mut from = 0usize;
    while let Some(offset) = body[from..].find(permalink) {
        let start = from + offset;
        let end = start + plen;
        let preceded = start > 0 && is_url_byte(bytes[start - 1]);
        let continued = end < bytes.len()
            && (is_url_byte(bytes[end])
                || (bytes[end] == b'.'
                    && bytes.get(end + 1).is_some_and(u8::is_ascii_alphanumeric)));
        if !preceded && !continued {
            return true;
        }
        from = start + 1;
    }
    false
}

/// A byte that can extend a URL token rather than end it.
#[must_use]
fn is_url_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric()
        || matches!(
            byte,
            b'-' | b'_' | b'~' | b'%' | b'/' | b'?' | b'#' | b'&' | b'='
        )
}

/// The 40-hex-digit validation of a commit sha.
fn sha_commit(sha: &str) -> Option<String> {
    (sha.len() == 40 && sha.bytes().all(|byte| byte.is_ascii_hexdigit()))
        .then(|| sha.to_ascii_lowercase().clone())
}

/// The commit and package path recorded by a package's
/// `.cargo_vcs_info.json`.
fn vcs_info(root: &Path) -> Option<VcsInfo> {
    let text = fs::read_to_string(root.join(".cargo_vcs_info.json")).ok()?;
    let value: serde_json::Value = serde_json::from_str(&text).ok()?;
    let sha = value.get("git")?.get("sha1")?.as_str()?.to_string();
    let sha = sha_commit(&sha)?;
    let path_in_vcs = value
        .get("path_in_vcs")
        .and_then(serde_json::Value::as_str)
        .map(str::to_string);
    Some(VcsInfo { sha, path_in_vcs })
}

/// The commit recorded by a package's local `.cargo_vcs_info.json`.
fn vcs_commit(root: &Path) -> Option<String> {
    vcs_info(root).map(|info| info.sha)
}

/// The commit pinned by a `git+...#<sha>` Cargo source id.
fn pinned_commit(source: &str) -> Option<String> {
    source
        .strip_prefix("git+")?
        .rsplit('#')
        .next()
        .and_then(sha_commit)
}

/// The repository root for a git checkout, found by walking up from `root`
/// to the nearest directory that holds a `.git` entry.
fn git_repo_root(root: &Path) -> Option<PathBuf> {
    let mut current = Some(root);
    while let Some(dir) = current {
        if dir.join(".git").exists() {
            return Some(dir.to_path_buf());
        }
        current = dir.parent();
    }
    None
}

/// The `origin` remote url recorded in a checkout's `.git/config`.
fn git_origin(root: &Path) -> Option<String> {
    let config = fs::read_to_string(root.join(".git").join("config")).ok()?;
    let mut in_origin = false;
    for line in config.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            in_origin = line == "[remote \"origin\"]";
            continue;
        }
        if in_origin
            && let Some((key, value)) = line.split_once('=')
            && key.trim() == "url"
        {
            return Some(value.trim().to_string());
        }
    }
    None
}

/// Verify the package path against its declared Git origin, returning `None` outside a checkout.
fn git_checkout_prefix(package: &PackageSource, repo: &str) -> Option<Result<String, String>> {
    let repo_root = git_repo_root(&package.root)?;
    let Some(origin) = git_origin(&repo_root) else {
        return Some(Err(format!(
            "git checkout of {} {} has no recorded origin",
            package.name, package.version
        )));
    };
    if normalize_github_repository(&origin).as_deref() != Some(repo) {
        return Some(Err(format!(
            "git checkout origin of {} {} does not match the declared repository",
            package.name, package.version
        )));
    }
    let rel = package.root.strip_prefix(&repo_root).ok()?;
    Some(Ok(rel.to_string_lossy().replace('\\', "/")))
}

/// The extraction-request body carrying only the upstream permalink.
fn extraction_body(permalink: &str) -> String {
    format!(
        "Please extract this source range into a public function or method.\n\n{permalink}\n\n\
         <!-- dejadoc: check existing issues and pull requests for this range, and state \
         willingness to contribute the extraction as a pull request. -->"
    )
}

/// The prefilled new-issue URL for one permalink.
fn request_url(repo: &str, item: &str, permalink: &str) -> String {
    let title = format!("{ISSUE_TAG} Extract a public helper from {item}");
    format!(
        "https://github.com/{repo}/issues/new?title={}&body={}",
        percent_encode(&title),
        percent_encode(&extraction_body(permalink))
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::context_network::tests::Fixture;
    use crate::reuse::ContextAccess;
    use crate::{ContextKind, ContextSite};
    use alloc::vec;

    const SHA: &str = "0123456789abcdef0123456789abcdef01234567";
    const PIN: &str = "fedcba9876543210fedcba9876543210fedcba98";
    const TAG_OBJECT: &str = "ffffffffffffffffffffffffffffffffffffffff";
    const BODY: &str =
        "fn helper(value: u32) -> u32 {\n    let total = value + 1u32;\n    total * 2u32\n}\n";
    const SOURCE: &str = "registry+https://github.com/rust-lang/crates.io-index";
    const TITLE: &str = "[DEJADOC] Extract a public helper from dep::helper";
    const FILE: &str = "src/lib.rs";

    fn permalink(commit: &str) -> String {
        format!("https://github.com/o/repo/blob/{commit}/{FILE}#L1-L4")
    }

    fn hash_of(text: &str) -> String {
        blake3::hash(text.as_bytes()).to_hex().to_string()
    }

    fn lib_hashes() -> BTreeMap<String, String> {
        let mut hashes = BTreeMap::new();
        hashes.insert(FILE.to_string(), hash_of(BODY));
        hashes
    }

    fn write_source_file(root: &Path, rel: &str, text: &str) {
        let path = root.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }

    fn vcs_info(root: &Path, sha: &str) {
        std::fs::write(
            root.join(".cargo_vcs_info.json"),
            format!("{{\"git\": {{\"sha1\": \"{sha}\"}}}}"),
        )
        .unwrap();
    }

    fn vcs_info_with_path(root: &Path, sha: &str, path_in_vcs: &str) {
        std::fs::write(
            root.join(".cargo_vcs_info.json"),
            format!("{{\"path_in_vcs\": \"{path_in_vcs}\", \"git\": {{\"sha1\": \"{sha}\"}}}}"),
        )
        .unwrap();
    }

    fn make_package(
        root: &Path,
        repository: Option<&str>,
        source: &str,
        source_hashes: BTreeMap<String, String>,
    ) -> PackageSource {
        PackageSource {
            name: "dep".into(),
            version: "1.2.3".into(),
            source: source.into(),
            root: root.to_path_buf(),
            repository: repository.map(str::to_string),
            source_hashes,
        }
    }

    fn candidate(
        action: ReuseAction,
        file: &str,
        line: u32,
        end: u32,
        end_column: usize,
    ) -> DependencyCandidate {
        DependencyCandidate {
            package: "dep".into(),
            version: "1.2.3".into(),
            source: SOURCE.into(),
            depth: 1,
            site: ContextSite {
                file: file.into(),
                line,
                column: 1,
                end,
                end_column,
                item: "dep::helper".into(),
                kind: ContextKind::FunctionBody,
            },
            access: ContextAccess::InternalFunction,
            api_path: None,
            conditions: Vec::new(),
            notes: Vec::new(),
            action,
            repository: None,
            source_url: None,
            issue_url: None,
            existing_issue: None,
            lookup_warning: None,
        }
    }

    fn upstream(cache: &Path, fixture: &Fixture) -> Upstream {
        Upstream::with_base(cache, &fixture.api_base(), &fixture.raw_base())
    }

    /// Run `enrich` over owned candidates, returning the distinct conditions.
    fn enrich_owned(
        up: &mut Upstream,
        package: &PackageSource,
        candidates: &mut [DependencyCandidate],
    ) -> Vec<String> {
        let mut batch: Vec<&mut DependencyCandidate> = candidates.iter_mut().collect();
        up.enrich(package, &mut batch)
    }

    fn verified_fixture(fixture: &Fixture) {
        fixture.route(&format!("/raw/o/repo/{SHA}/{FILE}"), 200, BODY.as_bytes());
        fixture.route(
            "/api/search/issues",
            200,
            search_json(&[]).to_string().as_bytes(),
        );
    }

    fn search_json(items: &[serde_json::Value]) -> serde_json::Value {
        serde_json::json!({
            "total_count": items.len(),
            "incomplete_results": false,
            "items": items,
        })
    }

    fn issue_item(number: u64, state: &str, body: &str, title: &str) -> serde_json::Value {
        serde_json::json!({
            "number": number,
            "state": state,
            "title": title,
            "body": body,
            "html_url": format!("https://github.com/o/repo/issues/{number}"),
        })
    }

    fn percent_decode(input: &str) -> String {
        let bytes = input.as_bytes();
        let mut out = Vec::new();
        let mut i = 0usize;
        while i < bytes.len() {
            match bytes[i] {
                b'%' if i + 2 < bytes.len() => {
                    let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).unwrap();
                    out.push(u8::from_str_radix(hex, 16).unwrap());
                    i += 3;
                }
                b'+' => {
                    out.push(b' ');
                    i += 1;
                }
                byte => {
                    out.push(byte);
                    i += 1;
                }
            }
        }
        String::from_utf8(out).unwrap()
    }

    fn query_params(query: &str) -> BTreeMap<String, String> {
        query
            .split('&')
            .map(|pair| {
                let (key, value) = pair.split_once('=').unwrap_or(("", ""));
                (key.to_string(), value.to_string())
            })
            .collect()
    }

    #[test]
    fn normalize_repository_accepts_github_forms() {
        assert_eq!(
            normalize_github_repository("https://github.com/O/Repo.git").as_deref(),
            Some("o/repo")
        );
        assert_eq!(
            normalize_github_repository("http://github.com/o/repo").as_deref(),
            Some("o/repo")
        );
        assert_eq!(
            normalize_github_repository("https://www.github.com/o/repo").as_deref(),
            Some("o/repo")
        );
        assert_eq!(
            normalize_github_repository("git+https://github.com/o/repo?branch=main").as_deref(),
            Some("o/repo")
        );
        assert_eq!(
            normalize_github_repository("ssh://git@github.com/o/repo.git").as_deref(),
            Some("o/repo")
        );
        assert_eq!(
            normalize_github_repository("git@github.com:o/repo").as_deref(),
            Some("o/repo")
        );
        assert_eq!(
            normalize_github_repository("https://gitlab.com/o/repo"),
            None
        );
        assert_eq!(
            normalize_github_repository("https://api.github.com/o/repo"),
            None
        );
        assert_eq!(normalize_github_repository("github.com/o/repo"), None);
        assert_eq!(
            normalize_github_repository("https://[::1]:443/o/repo"),
            None
        );
    }

    #[test]
    fn range_fragment_honors_exclusive_end_column() {
        assert_eq!(range_fragment(10, 14, 27).as_deref(), Some("#L10-L14"));
        assert_eq!(range_fragment(10, 14, 1).as_deref(), Some("#L10-L13"));
        assert_eq!(range_fragment(7, 7, 5).as_deref(), Some("#L7-L7"));
        assert_eq!(range_fragment(7, 7, 1), None);
        assert_eq!(range_fragment(10, 9, 5), None);
    }

    #[test]
    fn body_contains_permalink_checks_full_url_boundary() {
        let p = permalink(SHA);
        assert!(body_contains_permalink(&format!("see {p}."), &p));
        assert!(body_contains_permalink(&format!("`{p}`"), &p));
        assert!(body_contains_permalink(&p, &p));
        assert!(!body_contains_permalink(&format!("{p}0"), &p));
        assert!(!body_contains_permalink(&format!("{p}suffix"), &p));
        assert!(!body_contains_permalink(&format!("{p}/extra"), &p));
        assert!(!body_contains_permalink(&format!("{p}?ref=main"), &p));
        assert!(!body_contains_permalink(&format!("{p}-L9"), &p));
        for suffix in ["_more", "%20more", "~more", ".extra", "&other=1", "=other"] {
            assert!(
                !body_contains_permalink(&format!("{p}{suffix}"), &p),
                "{suffix}"
            );
        }
        for prefix in ["x", "https://example.test/?url=", "/", "%20"] {
            assert!(
                !body_contains_permalink(&format!("{prefix}{p}"), &p),
                "{prefix}"
            );
        }
    }

    #[test]
    fn enrich_verifies_source_and_builds_request() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("dep");
        std::fs::create_dir_all(&root).unwrap();
        write_source_file(&root, FILE, BODY);
        vcs_info(&root, SHA);
        let cache = dir.path().join("cache");
        let fixture = Fixture::start();
        verified_fixture(&fixture);
        let package = make_package(
            &root,
            Some("https://github.com/O/Repo.git"),
            SOURCE,
            lib_hashes(),
        );
        let mut candidates = vec![candidate(ReuseAction::RequestExtraction, FILE, 1, 4, 9)];
        let warnings = enrich_owned(&mut upstream(&cache, &fixture), &package, &mut candidates);
        assert!(warnings.is_empty(), "{warnings:?}");
        let c = &candidates[0];
        assert_eq!(c.repository.as_deref(), Some("o/repo"));
        assert_eq!(c.source_url.as_deref(), Some(permalink(SHA).as_str()));
        let issue_url = c.issue_url.as_deref().expect("prefilled request");
        assert!(
            issue_url.starts_with("https://github.com/o/repo/issues/new?title="),
            "{issue_url}"
        );
        assert!(
            issue_url.contains(
                "title=%5BDEJADOC%5D%20Extract%20a%20public%20helper%20from%20dep%3A%3Ahelper"
            ),
            "{issue_url}"
        );
        assert!(c.existing_issue.is_none());
        assert!(c.lookup_warning.is_none());
    }

    #[test]
    fn enrich_source_mismatch_withholds_link() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("dep");
        std::fs::create_dir_all(&root).unwrap();
        write_source_file(&root, FILE, BODY);
        vcs_info(&root, SHA);
        let cache = dir.path().join("cache");
        let fixture = Fixture::start();
        fixture.route(
            &format!("/raw/o/repo/{SHA}/{FILE}"),
            200,
            b"different upstream text",
        );
        let package = make_package(
            &root,
            Some("https://github.com/o/repo"),
            SOURCE,
            lib_hashes(),
        );
        let mut candidates = vec![candidate(ReuseAction::RequestExtraction, FILE, 1, 4, 9)];
        let warnings = enrich_owned(&mut upstream(&cache, &fixture), &package, &mut candidates);
        let c = &candidates[0];
        assert_eq!(c.repository.as_deref(), Some("o/repo"));
        assert!(c.source_url.is_none());
        assert!(c.issue_url.is_none());
        assert!(
            c.lookup_warning
                .as_deref()
                .unwrap_or_default()
                .contains("differs")
        );
        assert!(warnings.iter().any(|w| w.contains("differs")));
        assert_eq!(fixture.hits("/api/search/issues"), 0);
    }

    #[test]
    fn enrich_missing_snapshot_withholds_link() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("dep");
        std::fs::create_dir_all(&root).unwrap();
        vcs_info(&root, SHA);
        let cache = dir.path().join("cache");
        let fixture = Fixture::start();
        fixture.route(&format!("/raw/o/repo/{SHA}/{FILE}"), 200, BODY.as_bytes());
        let package = make_package(
            &root,
            Some("https://github.com/o/repo"),
            SOURCE,
            BTreeMap::new(),
        );
        let mut candidates = vec![candidate(ReuseAction::RequestExtraction, FILE, 1, 4, 9)];
        let warnings = enrich_owned(&mut upstream(&cache, &fixture), &package, &mut candidates);
        let c = &candidates[0];
        assert!(c.source_url.is_none());
        assert!(
            c.lookup_warning
                .as_deref()
                .unwrap_or_default()
                .contains("no indexed source snapshot")
        );
        assert!(
            warnings
                .iter()
                .any(|w| w.contains("no indexed source snapshot"))
        );
        assert_eq!(fixture.hits(&format!("/raw/o/repo/{SHA}/{FILE}")), 0);
    }

    #[test]
    fn enrich_memoizes_repeat_reads() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("dep");
        std::fs::create_dir_all(&root).unwrap();
        write_source_file(&root, FILE, BODY);
        vcs_info(&root, SHA);
        let cache = dir.path().join("cache");
        let fixture = Fixture::start();
        verified_fixture(&fixture);
        let package = make_package(
            &root,
            Some("https://github.com/o/repo"),
            SOURCE,
            lib_hashes(),
        );
        let mut candidates = vec![
            candidate(ReuseAction::RequestExtraction, FILE, 1, 4, 9),
            candidate(ReuseAction::RequestExtraction, FILE, 1, 4, 9),
        ];
        let mut up = upstream(&cache, &fixture);
        enrich_owned(&mut up, &package, &mut candidates);
        assert_eq!(fixture.hits(&format!("/raw/o/repo/{SHA}/{FILE}")), 1);
        assert_eq!(fixture.hits("/api/search/issues"), 1);
        enrich_owned(&mut up, &package, &mut candidates);
        assert_eq!(fixture.hits(&format!("/raw/o/repo/{SHA}/{FILE}")), 1);
        assert_eq!(fixture.hits("/api/search/issues"), 1);
    }

    #[test]
    fn enrich_request_body_carries_only_the_permalink() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("dep");
        std::fs::create_dir_all(&root).unwrap();
        write_source_file(&root, FILE, BODY);
        vcs_info(&root, SHA);
        let cache = dir.path().join("cache");
        let fixture = Fixture::start();
        verified_fixture(&fixture);
        let package = make_package(
            &root,
            Some("https://github.com/o/repo"),
            SOURCE,
            lib_hashes(),
        );
        let mut candidates = vec![candidate(ReuseAction::RequestExtraction, FILE, 1, 4, 9)];
        enrich_owned(&mut upstream(&cache, &fixture), &package, &mut candidates);
        let issue_url = candidates[0].issue_url.clone().expect("prefilled request");
        let params = query_params(issue_url.split_once('?').expect("query").1);
        let title = percent_decode(params.get("title").expect("title param"));
        let body = percent_decode(params.get("body").expect("body param"));
        assert_eq!(title, TITLE);
        assert!(body.contains(permalink(SHA).as_str()));
        assert!(body.contains("<!-- dejadoc"));
        assert!(!body.contains("value + 1u32"));
        assert!(!body.contains(dir.path().to_str().unwrap()));
        assert!(!body.contains("crates.io"));
    }

    #[test]
    fn enrich_existing_closed_request_replaces_creation_link() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("dep");
        std::fs::create_dir_all(&root).unwrap();
        write_source_file(&root, FILE, BODY);
        vcs_info(&root, SHA);
        let cache = dir.path().join("cache");
        let fixture = Fixture::start();
        fixture.route(&format!("/raw/o/repo/{SHA}/{FILE}"), 200, BODY.as_bytes());
        let item = issue_item(
            7,
            "closed",
            &format!("please extract {}", permalink(SHA)),
            TITLE,
        );
        fixture.route(
            "/api/search/issues",
            200,
            search_json(&[item]).to_string().as_bytes(),
        );
        let package = make_package(
            &root,
            Some("https://github.com/o/repo"),
            SOURCE,
            lib_hashes(),
        );
        let mut candidates = vec![candidate(ReuseAction::RequestExtraction, FILE, 1, 4, 9)];
        let warnings = enrich_owned(&mut upstream(&cache, &fixture), &package, &mut candidates);
        assert!(warnings.is_empty(), "{warnings:?}");
        let c = &candidates[0];
        let existing = c.existing_issue.as_ref().expect("existing request");
        assert_eq!(existing.url, "https://github.com/o/repo/issues/7");
        assert_eq!(existing.state, "closed");
        assert!(c.issue_url.is_none());
        assert!(c.lookup_warning.is_none());
    }

    #[test]
    fn enrich_open_request_wins_over_closed() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("dep");
        std::fs::create_dir_all(&root).unwrap();
        write_source_file(&root, FILE, BODY);
        vcs_info(&root, SHA);
        let cache = dir.path().join("cache");
        let fixture = Fixture::start();
        fixture.route(&format!("/raw/o/repo/{SHA}/{FILE}"), 200, BODY.as_bytes());
        let closed = issue_item(
            7,
            "closed",
            &format!("please extract {}", permalink(SHA)),
            TITLE,
        );
        let open = issue_item(
            9,
            "open",
            &format!("please extract {}", permalink(SHA)),
            TITLE,
        );
        fixture.route(
            "/api/search/issues",
            200,
            search_json(&[closed, open]).to_string().as_bytes(),
        );
        let package = make_package(
            &root,
            Some("https://github.com/o/repo"),
            SOURCE,
            lib_hashes(),
        );
        let mut candidates = vec![candidate(ReuseAction::RequestExtraction, FILE, 1, 4, 9)];
        enrich_owned(&mut upstream(&cache, &fixture), &package, &mut candidates);
        let existing = candidates[0]
            .existing_issue
            .as_ref()
            .expect("existing request");
        assert_eq!(existing.url, "https://github.com/o/repo/issues/9");
        assert_eq!(existing.state, "open");
        assert!(candidates[0].issue_url.is_none());
    }

    #[test]
    fn enrich_false_search_match_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("dep");
        std::fs::create_dir_all(&root).unwrap();
        write_source_file(&root, FILE, BODY);
        vcs_info(&root, SHA);
        let cache = dir.path().join("cache");
        let fixture = Fixture::start();
        fixture.route(&format!("/raw/o/repo/{SHA}/{FILE}"), 200, BODY.as_bytes());
        let near = format!("https://github.com/o/repo/blob/{SHA}/{FILE}#L1-L5");
        let item = issue_item(3, "open", &format!("please extract {near}"), TITLE);
        fixture.route(
            "/api/search/issues",
            200,
            search_json(&[item]).to_string().as_bytes(),
        );
        let package = make_package(
            &root,
            Some("https://github.com/o/repo"),
            SOURCE,
            lib_hashes(),
        );
        let mut candidates = vec![candidate(ReuseAction::RequestExtraction, FILE, 1, 4, 9)];
        let warnings = enrich_owned(&mut upstream(&cache, &fixture), &package, &mut candidates);
        assert!(warnings.is_empty(), "{warnings:?}");
        let c = &candidates[0];
        assert!(c.existing_issue.is_none());
        assert!(
            c.issue_url
                .as_deref()
                .unwrap_or_default()
                .starts_with("https://github.com/o/repo/issues/new?title=")
        );
        assert!(c.lookup_warning.is_none());
    }

    #[test]
    fn enrich_longer_range_tokens_do_not_match() {
        let p = permalink(SHA);
        let longer = p.strip_suffix("4").unwrap().to_string() + "40";
        let suffixed = format!("{p}suffix");
        for (body, match_expected) in [
            (longer, false),
            (suffixed, false),
            (format!("{p}."), true),
            (format!("{p}?ref=main"), false),
            (format!("{p}/more"), false),
        ] {
            let dir = tempfile::tempdir().unwrap();
            let root = dir.path().join("dep");
            std::fs::create_dir_all(&root).unwrap();
            write_source_file(&root, FILE, BODY);
            vcs_info(&root, SHA);
            let cache = dir.path().join("cache");
            let fixture = Fixture::start();
            fixture.route(&format!("/raw/o/repo/{SHA}/{FILE}"), 200, BODY.as_bytes());
            let item = issue_item(4, "open", &body, TITLE);
            fixture.route(
                "/api/search/issues",
                200,
                search_json(&[item]).to_string().as_bytes(),
            );
            let package = make_package(
                &root,
                Some("https://github.com/o/repo"),
                SOURCE,
                lib_hashes(),
            );
            let mut candidates = vec![candidate(ReuseAction::RequestExtraction, FILE, 1, 4, 9)];
            enrich_owned(&mut upstream(&cache, &fixture), &package, &mut candidates);
            assert_eq!(
                candidates[0].existing_issue.is_some(),
                match_expected,
                "{body}"
            );
            assert_eq!(candidates[0].issue_url.is_some(), !match_expected, "{body}");
        }
    }

    #[test]
    fn enrich_pull_request_result_ignored() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("dep");
        std::fs::create_dir_all(&root).unwrap();
        write_source_file(&root, FILE, BODY);
        vcs_info(&root, SHA);
        let cache = dir.path().join("cache");
        let fixture = Fixture::start();
        fixture.route(&format!("/raw/o/repo/{SHA}/{FILE}"), 200, BODY.as_bytes());
        let mut item = issue_item(
            5,
            "open",
            &format!("please extract {}", permalink(SHA)),
            TITLE,
        );
        item["pull_request"] = serde_json::json!({"url": "https://github.com/o/repo/pull/5"});
        fixture.route(
            "/api/search/issues",
            200,
            search_json(&[item]).to_string().as_bytes(),
        );
        let package = make_package(
            &root,
            Some("https://github.com/o/repo"),
            SOURCE,
            lib_hashes(),
        );
        let mut candidates = vec![candidate(ReuseAction::RequestExtraction, FILE, 1, 4, 9)];
        enrich_owned(&mut upstream(&cache, &fixture), &package, &mut candidates);
        assert!(candidates[0].existing_issue.is_none());
        assert!(candidates[0].issue_url.is_some());
    }

    #[test]
    fn enrich_untagged_title_is_not_a_match() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("dep");
        std::fs::create_dir_all(&root).unwrap();
        write_source_file(&root, FILE, BODY);
        vcs_info(&root, SHA);
        let cache = dir.path().join("cache");
        let fixture = Fixture::start();
        fixture.route(&format!("/raw/o/repo/{SHA}/{FILE}"), 200, BODY.as_bytes());
        let item = issue_item(
            6,
            "open",
            &format!("please extract {}", permalink(SHA)),
            "extract a helper please",
        );
        fixture.route(
            "/api/search/issues",
            200,
            search_json(&[item]).to_string().as_bytes(),
        );
        let package = make_package(
            &root,
            Some("https://github.com/o/repo"),
            SOURCE,
            lib_hashes(),
        );
        let mut candidates = vec![candidate(ReuseAction::RequestExtraction, FILE, 1, 4, 9)];
        enrich_owned(&mut upstream(&cache, &fixture), &package, &mut candidates);
        assert!(candidates[0].existing_issue.is_none());
        assert!(candidates[0].issue_url.is_some());
    }

    #[test]
    fn enrich_unknown_repository_keeps_candidate_bare() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("dep");
        std::fs::create_dir_all(&root).unwrap();
        write_source_file(&root, FILE, BODY);
        vcs_info(&root, SHA);
        let cache = dir.path().join("cache");
        let fixture = Fixture::start();
        fixture.route(&format!("/raw/o/repo/{SHA}/{FILE}"), 200, BODY.as_bytes());
        fixture.route(
            "/api/search/issues",
            200,
            search_json(&[]).to_string().as_bytes(),
        );
        let package = make_package(
            &root,
            Some("https://gitlab.com/o/repo"),
            SOURCE,
            lib_hashes(),
        );
        let mut candidates = vec![candidate(ReuseAction::RequestExtraction, FILE, 1, 4, 9)];
        let warnings = enrich_owned(&mut upstream(&cache, &fixture), &package, &mut candidates);
        let c = &candidates[0];
        assert_eq!(c.repository.as_deref(), Some("https://gitlab.com/o/repo"));
        assert!(c.source_url.is_none());
        assert!(c.issue_url.is_none());
        assert!(
            c.lookup_warning
                .as_deref()
                .unwrap_or_default()
                .contains("not on GitHub")
        );
        assert!(warnings.iter().any(|w| w.contains("not on GitHub")));
        assert_eq!(fixture.hits(&format!("/raw/o/repo/{SHA}/{FILE}")), 0);
        assert_eq!(fixture.hits("/api/search/issues"), 0);
    }

    #[test]
    fn enrich_git_pin_source_uses_pinned_commit() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("dep");
        std::fs::create_dir_all(&root).unwrap();
        write_source_file(&root, FILE, BODY);
        let cache = dir.path().join("cache");
        let fixture = Fixture::start();
        fixture.route(&format!("/raw/o/repo/{PIN}/{FILE}"), 200, BODY.as_bytes());
        fixture.route(
            "/api/search/issues",
            200,
            search_json(&[]).to_string().as_bytes(),
        );
        let source = format!("git+https://github.com/o/repo?branch=main#{PIN}");
        let package = make_package(
            &root,
            Some("https://github.com/o/repo"),
            &source,
            lib_hashes(),
        );
        let mut candidates = vec![candidate(ReuseAction::RequestExtraction, FILE, 1, 4, 9)];
        enrich_owned(&mut upstream(&cache, &fixture), &package, &mut candidates);
        assert_eq!(
            candidates[0].source_url.as_deref(),
            Some(permalink(PIN).as_str())
        );
    }

    #[test]
    fn enrich_monorepo_path_in_vcs_prefixes_the_path() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("dep");
        std::fs::create_dir_all(&root).unwrap();
        write_source_file(&root, FILE, BODY);
        vcs_info_with_path(&root, SHA, "crates/dep");
        let cache = dir.path().join("cache");
        let fixture = Fixture::start();
        fixture.route(
            &format!("/raw/o/repo/{SHA}/crates/dep/{FILE}"),
            200,
            BODY.as_bytes(),
        );
        fixture.route(
            "/api/search/issues",
            200,
            search_json(&[]).to_string().as_bytes(),
        );
        let package = make_package(
            &root,
            Some("https://github.com/o/repo"),
            SOURCE,
            lib_hashes(),
        );
        let mut candidates = vec![candidate(ReuseAction::RequestExtraction, FILE, 1, 4, 9)];
        let warnings = enrich_owned(&mut upstream(&cache, &fixture), &package, &mut candidates);
        assert!(warnings.is_empty(), "{warnings:?}");
        assert_eq!(
            candidates[0].source_url.as_deref(),
            Some(format!("https://github.com/o/repo/blob/{SHA}/crates/dep/{FILE}#L1-L4").as_str())
        );
    }

    #[test]
    fn enrich_rejects_a_path_in_vcs_that_leaves_the_repository() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("dep");
        std::fs::create_dir_all(&root).unwrap();
        write_source_file(&root, FILE, BODY);
        vcs_info_with_path(&root, SHA, "../../../other/repo/main");
        let cache = dir.path().join("cache");
        let fixture = Fixture::start();
        fixture.route(
            &format!("/raw/o/repo/{SHA}/../../../other/repo/main/{FILE}"),
            200,
            BODY.as_bytes(),
        );
        fixture.route(
            "/api/search/issues",
            200,
            search_json(&[]).to_string().as_bytes(),
        );
        let package = make_package(
            &root,
            Some("https://github.com/o/repo"),
            SOURCE,
            lib_hashes(),
        );
        let mut candidates = vec![candidate(ReuseAction::RequestExtraction, FILE, 1, 4, 9)];
        let warnings = enrich_owned(&mut upstream(&cache, &fixture), &package, &mut candidates);
        assert_eq!(candidates[0].source_url, None);
        assert_eq!(candidates[0].issue_url, None);
        assert!(
            warnings.iter().any(|warning| warning.contains("escapes")),
            "{warnings:?}"
        );
    }

    #[test]
    fn enrich_git_checkout_prefix_verifies_the_origin() {
        let dir = tempfile::tempdir().unwrap();
        let checkout = dir.path().join("checkout");
        let root = checkout.join("crates").join("dep");
        std::fs::create_dir_all(&root).unwrap();
        write_source_file(&root, FILE, BODY);
        let git_dir = checkout.join(".git");
        std::fs::create_dir_all(&git_dir).unwrap();
        std::fs::write(
            git_dir.join("config"),
            "[remote \"origin\"]\n\turl = https://github.com/o/repo\n\tfetch = +refs/heads/*:refs/remotes/origin/*\n",
        ).unwrap();
        let cache = dir.path().join("cache");
        let fixture = Fixture::start();
        fixture.route(
            &format!("/raw/o/repo/{PIN}/crates/dep/{FILE}"),
            200,
            BODY.as_bytes(),
        );
        fixture.route(
            "/api/search/issues",
            200,
            search_json(&[]).to_string().as_bytes(),
        );
        let source = format!("git+https://github.com/o/repo?branch=main#{PIN}");
        let package = make_package(
            &root,
            Some("https://github.com/o/repo"),
            &source,
            lib_hashes(),
        );
        let mut candidates = vec![candidate(ReuseAction::RequestExtraction, FILE, 1, 4, 9)];
        let warnings = enrich_owned(&mut upstream(&cache, &fixture), &package, &mut candidates);
        assert!(warnings.is_empty(), "{warnings:?}");
        assert_eq!(
            candidates[0].source_url.as_deref(),
            Some(format!("https://github.com/o/repo/blob/{PIN}/crates/dep/{FILE}#L1-L4").as_str())
        );
        let foreign = tempfile::tempdir().unwrap();
        let foreign_checkout = foreign.path().join("checkout");
        let foreign_root = foreign_checkout.join("crates").join("dep");
        std::fs::create_dir_all(&foreign_root).unwrap();
        write_source_file(&foreign_root, FILE, BODY);
        let foreign_git = foreign_checkout.join(".git");
        std::fs::create_dir_all(&foreign_git).unwrap();
        std::fs::write(
            foreign_git.join("config"),
            "[remote \"origin\"]\n\turl = https://github.com/other/repo\n",
        )
        .unwrap();
        let foreign_package = make_package(
            &foreign_root,
            Some("https://github.com/o/repo"),
            &source,
            lib_hashes(),
        );
        let mut foreign_candidates = vec![candidate(ReuseAction::RequestExtraction, FILE, 1, 4, 9)];
        let foreign_warnings = enrich_owned(
            &mut upstream(&cache, &fixture),
            &foreign_package,
            &mut foreign_candidates,
        );
        assert!(foreign_candidates[0].source_url.is_none());
        assert!(
            foreign_warnings
                .iter()
                .any(|w| w.contains("does not match"))
        );
    }

    #[test]
    fn enrich_tag_resolution_uses_version_tags() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("dep");
        std::fs::create_dir_all(&root).unwrap();
        write_source_file(&root, FILE, BODY);
        let cache = dir.path().join("cache");
        let fixture = Fixture::start();
        fixture.route(
            "/api/repos/o/repo/git/ref/tags/v1.2.3",
            200,
            serde_json::json!({"object": {"type": "tag", "sha": TAG_OBJECT}})
                .to_string()
                .as_bytes(),
        );
        fixture.route(
            format!("/api/repos/o/repo/git/tags/{TAG_OBJECT}").as_str(),
            200,
            serde_json::json!({"object": {"type": "commit", "sha": SHA}})
                .to_string()
                .as_bytes(),
        );
        fixture.route(&format!("/raw/o/repo/{SHA}/{FILE}"), 200, BODY.as_bytes());
        fixture.route(
            "/api/search/issues",
            200,
            search_json(&[]).to_string().as_bytes(),
        );
        let package = make_package(
            &root,
            Some("https://github.com/o/repo"),
            SOURCE,
            lib_hashes(),
        );
        let mut candidates = vec![candidate(ReuseAction::RequestExtraction, FILE, 1, 4, 9)];
        enrich_owned(&mut upstream(&cache, &fixture), &package, &mut candidates);
        assert_eq!(
            candidates[0].source_url.as_deref(),
            Some(permalink(SHA).as_str())
        );
        let provenance = std::fs::read_to_string(cache.join("private/upstream/provenance.json"))
            .unwrap_or_default();
        assert!(!provenance.contains(SOURCE), "{provenance}");
    }

    #[test]
    fn enrich_persists_and_reuses_immutable_provenance() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("dep");
        std::fs::create_dir_all(&root).unwrap();
        write_source_file(&root, FILE, BODY);
        vcs_info(&root, SHA);
        let cache = dir.path().join("cache");
        let fixture = Fixture::start();
        verified_fixture(&fixture);
        let package = make_package(
            &root,
            Some("https://github.com/o/repo"),
            SOURCE,
            lib_hashes(),
        );
        let mut candidates = vec![candidate(ReuseAction::RequestExtraction, FILE, 1, 4, 9)];
        enrich_owned(&mut upstream(&cache, &fixture), &package, &mut candidates);
        let provenance = std::fs::read_to_string(cache.join("private/upstream/provenance.json"))
            .expect("provenance file");
        assert!(provenance.contains(SHA), "{provenance}");
        std::fs::remove_file(root.join(".cargo_vcs_info.json")).unwrap();
        let fixture = Fixture::start();
        verified_fixture(&fixture);
        let mut candidates = vec![candidate(ReuseAction::RequestExtraction, FILE, 1, 4, 9)];
        let warnings = enrich_owned(&mut upstream(&cache, &fixture), &package, &mut candidates);
        assert!(warnings.is_empty(), "{warnings:?}");
        assert_eq!(
            candidates[0].source_url.as_deref(),
            Some(permalink(SHA).as_str())
        );
        assert_eq!(fixture.hits("/api/repos/o/repo/git/ref/tags/v1.2.3"), 0);
    }

    #[test]
    fn enrich_local_evidence_wins_over_persisted_provenance() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("dep");
        std::fs::create_dir_all(&root).unwrap();
        write_source_file(&root, FILE, BODY);
        vcs_info(&root, SHA);
        let cache = dir.path().join("cache");
        let provenance_dir = cache.join("private/upstream");
        std::fs::create_dir_all(&provenance_dir).unwrap();
        let key = format!("dep@1.2.3|{SOURCE}");
        let mut provenance = BTreeMap::new();
        provenance.insert(key, PIN.to_string());
        let text = serde_json::to_string(&provenance).unwrap();
        std::fs::write(provenance_dir.join("provenance.json"), text).unwrap();
        let fixture = Fixture::start();
        fixture.route(&format!("/raw/o/repo/{SHA}/{FILE}"), 200, BODY.as_bytes());
        fixture.route(
            "/api/search/issues",
            200,
            search_json(&[]).to_string().as_bytes(),
        );
        let package = make_package(
            &root,
            Some("https://github.com/o/repo"),
            SOURCE,
            lib_hashes(),
        );
        let mut candidates = vec![candidate(ReuseAction::RequestExtraction, FILE, 1, 4, 9)];
        enrich_owned(&mut upstream(&cache, &fixture), &package, &mut candidates);
        assert_eq!(
            candidates[0].source_url.as_deref(),
            Some(permalink(SHA).as_str())
        );
        assert_eq!(fixture.hits(&format!("/raw/o/repo/{PIN}/{FILE}")), 0);
    }

    #[test]
    fn enrich_discloses_http_failures() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("dep");
        std::fs::create_dir_all(&root).unwrap();
        write_source_file(&root, FILE, BODY);
        vcs_info(&root, SHA);
        let cache = dir.path().join("cache");
        let fixture = Fixture::start();
        fixture.route(&format!("/raw/o/repo/{SHA}/{FILE}"), 500, b"{}");
        let package = make_package(
            &root,
            Some("https://github.com/o/repo"),
            SOURCE,
            lib_hashes(),
        );
        let mut candidates = vec![candidate(ReuseAction::RequestExtraction, FILE, 1, 4, 9)];
        let warnings = enrich_owned(&mut upstream(&cache, &fixture), &package, &mut candidates);
        let c = &candidates[0];
        assert!(c.source_url.is_none());
        assert!(c.issue_url.is_none());
        assert!(
            c.lookup_warning
                .as_deref()
                .unwrap_or_default()
                .contains("HTTP 500")
        );
        assert!(warnings.iter().any(|w| w.contains("HTTP 500")));
    }

    #[test]
    fn enrich_memoizes_failed_reads() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("dep");
        std::fs::create_dir_all(&root).unwrap();
        write_source_file(&root, FILE, BODY);
        vcs_info(&root, SHA);
        let cache = dir.path().join("cache");
        let fixture = Fixture::start();
        fixture.route(&format!("/raw/o/repo/{SHA}/{FILE}"), 500, b"{}");
        let package = make_package(
            &root,
            Some("https://github.com/o/repo"),
            SOURCE,
            lib_hashes(),
        );
        let mut candidates = vec![
            candidate(ReuseAction::RequestExtraction, FILE, 1, 4, 9),
            candidate(ReuseAction::RequestExtraction, FILE, 1, 4, 9),
        ];
        enrich_owned(&mut upstream(&cache, &fixture), &package, &mut candidates);
        assert_eq!(fixture.hits(&format!("/raw/o/repo/{SHA}/{FILE}")), 1);
    }

    #[test]
    fn enrich_source_read_bound_disclosed() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("dep");
        std::fs::create_dir_all(&root).unwrap();
        write_source_file(&root, FILE, BODY);
        vcs_info(&root, SHA);
        let cache = dir.path().join("cache");
        let fixture = Fixture::start();
        fixture.fill_route(
            &format!("/raw/o/repo/{SHA}/{FILE}"),
            200,
            crate::context_network::SOURCE_LIMIT + 1,
        );
        let package = make_package(
            &root,
            Some("https://github.com/o/repo"),
            SOURCE,
            lib_hashes(),
        );
        let mut candidates = vec![candidate(ReuseAction::RequestExtraction, FILE, 1, 4, 9)];
        let warnings = enrich_owned(&mut upstream(&cache, &fixture), &package, &mut candidates);
        let c = &candidates[0];
        assert!(c.source_url.is_none());
        assert!(
            c.lookup_warning
                .as_deref()
                .unwrap_or_default()
                .contains("exceeds the read bound")
        );
        assert!(
            warnings
                .iter()
                .any(|w| w.contains("exceeds the read bound"))
        );
    }

    #[test]
    fn enrich_rate_limited_search_keeps_unchecked_link() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("dep");
        std::fs::create_dir_all(&root).unwrap();
        write_source_file(&root, FILE, BODY);
        vcs_info(&root, SHA);
        let cache = dir.path().join("cache");
        let fixture = Fixture::start();
        fixture.route(&format!("/raw/o/repo/{SHA}/{FILE}"), 200, BODY.as_bytes());
        fixture.rate_limited_route("/api/search/issues");
        let package = make_package(
            &root,
            Some("https://github.com/o/repo"),
            SOURCE,
            lib_hashes(),
        );
        let mut candidates = vec![candidate(ReuseAction::RequestExtraction, FILE, 1, 4, 9)];
        enrich_owned(&mut upstream(&cache, &fixture), &package, &mut candidates);
        let c = &candidates[0];
        assert_eq!(c.source_url.as_deref(), Some(permalink(SHA).as_str()));
        assert!(
            c.issue_url
                .as_deref()
                .unwrap_or_default()
                .starts_with("https://github.com/o/repo/issues/new?title=")
        );
        assert!(
            c.lookup_warning
                .as_deref()
                .unwrap_or_default()
                .contains("rate limit")
        );
        assert!(c.existing_issue.is_none());
    }

    #[test]
    fn enrich_permission_403_is_not_a_rate_limit() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("dep");
        std::fs::create_dir_all(&root).unwrap();
        write_source_file(&root, FILE, BODY);
        vcs_info(&root, SHA);
        let cache = dir.path().join("cache");
        let fixture = Fixture::start();
        fixture.route(&format!("/raw/o/repo/{SHA}/{FILE}"), 200, BODY.as_bytes());
        fixture.route("/api/search/issues", 403, b"{}");
        let package = make_package(
            &root,
            Some("https://github.com/o/repo"),
            SOURCE,
            lib_hashes(),
        );
        let mut candidates = vec![candidate(ReuseAction::RequestExtraction, FILE, 1, 4, 9)];
        enrich_owned(&mut upstream(&cache, &fixture), &package, &mut candidates);
        let c = &candidates[0];
        assert_eq!(c.source_url.as_deref(), Some(permalink(SHA).as_str()));
        assert!(
            c.issue_url
                .as_deref()
                .unwrap_or_default()
                .starts_with("https://github.com/o/repo/issues/new?title=")
        );
        let warning = c.lookup_warning.as_deref().unwrap_or_default();
        assert!(warning.contains("permission denied"), "{warning}");
        assert!(!warning.contains("rate limit"), "{warning}");
        assert!(c.existing_issue.is_none());
    }

    #[test]
    fn enrich_incomplete_search_disclosed() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("dep");
        std::fs::create_dir_all(&root).unwrap();
        write_source_file(&root, FILE, BODY);
        vcs_info(&root, SHA);
        let cache = dir.path().join("cache");
        let fixture = Fixture::start();
        fixture.route(&format!("/raw/o/repo/{SHA}/{FILE}"), 200, BODY.as_bytes());
        let incomplete = serde_json::json!({
            "total_count": 0,
            "incomplete_results": true,
            "items": [],
        });
        fixture.route("/api/search/issues", 200, incomplete.to_string().as_bytes());
        let package = make_package(
            &root,
            Some("https://github.com/o/repo"),
            SOURCE,
            lib_hashes(),
        );
        let mut candidates = vec![candidate(ReuseAction::RequestExtraction, FILE, 1, 4, 9)];
        enrich_owned(&mut upstream(&cache, &fixture), &package, &mut candidates);
        let c = &candidates[0];
        assert!(
            c.issue_url
                .as_deref()
                .unwrap_or_default()
                .starts_with("https://github.com/o/repo/issues/new?title=")
        );
        assert!(
            c.lookup_warning
                .as_deref()
                .unwrap_or_default()
                .contains("incomplete")
        );
    }

    #[test]
    fn enrich_missing_items_disclose_unchecked_lookup() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("dep");
        std::fs::create_dir_all(&root).unwrap();
        write_source_file(&root, FILE, BODY);
        vcs_info(&root, SHA);
        let cache = dir.path().join("cache");
        let fixture = Fixture::start();
        fixture.route(&format!("/raw/o/repo/{SHA}/{FILE}"), 200, BODY.as_bytes());
        let no_items = serde_json::json!({
            "total_count": 0,
            "incomplete_results": false,
        });
        fixture.route("/api/search/issues", 200, no_items.to_string().as_bytes());
        let package = make_package(
            &root,
            Some("https://github.com/o/repo"),
            SOURCE,
            lib_hashes(),
        );
        let mut candidates = vec![candidate(ReuseAction::RequestExtraction, FILE, 1, 4, 9)];
        enrich_owned(&mut upstream(&cache, &fixture), &package, &mut candidates);
        let c = &candidates[0];
        assert!(
            c.issue_url
                .as_deref()
                .unwrap_or_default()
                .starts_with("https://github.com/o/repo/issues/new?title=")
        );
        assert!(
            c.lookup_warning
                .as_deref()
                .unwrap_or_default()
                .contains("unreadable")
        );
        assert!(c.existing_issue.is_none());
    }

    #[test]
    fn enrich_search_pagination_walks_pages() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("dep");
        std::fs::create_dir_all(&root).unwrap();
        write_source_file(&root, FILE, BODY);
        vcs_info(&root, SHA);
        let cache = dir.path().join("cache");
        let fixture = Fixture::start();
        fixture.route(&format!("/raw/o/repo/{SHA}/{FILE}"), 200, BODY.as_bytes());
        let page_one: Vec<serde_json::Value> = (0u64..100)
            .map(|n| issue_item(n, "open", "unrelated tagged request", TITLE))
            .collect();
        let page_two = vec![issue_item(
            5,
            "closed",
            &format!("please extract {}", permalink(SHA)),
            TITLE,
        )];
        fixture.page_route(
            "/api/search/issues",
            vec![
                (200, search_json(&page_one).to_string().into_bytes()),
                (200, search_json(&page_two).to_string().into_bytes()),
            ],
        );
        let package = make_package(
            &root,
            Some("https://github.com/o/repo"),
            SOURCE,
            lib_hashes(),
        );
        let mut candidates = vec![candidate(ReuseAction::RequestExtraction, FILE, 1, 4, 9)];
        let warnings = enrich_owned(&mut upstream(&cache, &fixture), &package, &mut candidates);
        assert!(warnings.is_empty(), "{warnings:?}");
        let c = &candidates[0];
        let existing = c.existing_issue.as_ref().expect("existing request");
        assert_eq!(existing.url, "https://github.com/o/repo/issues/5");
        assert_eq!(existing.state, "closed");
        assert!(c.issue_url.is_none());
        assert_eq!(fixture.hits("/api/search/issues"), 2);
    }

    #[test]
    fn enrich_search_query_targets_the_repository() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("dep");
        std::fs::create_dir_all(&root).unwrap();
        write_source_file(&root, FILE, BODY);
        vcs_info(&root, SHA);
        let cache = dir.path().join("cache");
        let fixture = Fixture::start();
        verified_fixture(&fixture);
        let package = make_package(
            &root,
            Some("https://github.com/o/repo"),
            SOURCE,
            lib_hashes(),
        );
        let mut candidates = vec![candidate(ReuseAction::RequestExtraction, FILE, 1, 4, 9)];
        enrich_owned(&mut upstream(&cache, &fixture), &package, &mut candidates);
        let target = fixture
            .last_target("/api/search/issues")
            .expect("search target");
        assert!(target.contains("q=repo%3Ao%2Fr"), "{target}");
        assert!(target.contains("per_page=100"), "{target}");
        assert!(target.contains("in%3Atitle"), "{target}");
    }

    #[test]
    fn enrich_api_candidates_get_evidence_links_without_issue_buttons() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("dep");
        std::fs::create_dir_all(&root).unwrap();
        write_source_file(&root, FILE, BODY);
        vcs_info(&root, SHA);
        let cache = dir.path().join("cache");
        let fixture = Fixture::start();
        verified_fixture(&fixture);
        let package = make_package(
            &root,
            Some("https://github.com/o/repo"),
            SOURCE,
            lib_hashes(),
        );
        let mut candidates = vec![candidate(ReuseAction::ReuseApi, FILE, 1, 4, 9)];
        let warnings = enrich_owned(&mut upstream(&cache, &fixture), &package, &mut candidates);
        assert!(warnings.is_empty(), "{warnings:?}");
        let c = &candidates[0];
        assert_eq!(c.source_url.as_deref(), Some(permalink(SHA).as_str()));
        assert!(c.issue_url.is_none());
        assert!(c.existing_issue.is_none());
        assert!(c.lookup_warning.is_none());
        assert_eq!(fixture.hits("/api/search/issues"), 0);
    }

    #[test]
    fn enrich_empty_range_withholds_link() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("dep");
        std::fs::create_dir_all(&root).unwrap();
        write_source_file(&root, FILE, BODY);
        vcs_info(&root, SHA);
        let cache = dir.path().join("cache");
        let fixture = Fixture::start();
        fixture.route(&format!("/raw/o/repo/{SHA}/{FILE}"), 200, BODY.as_bytes());
        let package = make_package(
            &root,
            Some("https://github.com/o/repo"),
            SOURCE,
            lib_hashes(),
        );
        let mut candidates = vec![candidate(ReuseAction::RequestExtraction, FILE, 5, 5, 1)];
        let warnings = enrich_owned(&mut upstream(&cache, &fixture), &package, &mut candidates);
        let c = &candidates[0];
        assert!(c.source_url.is_none());
        assert!(
            c.lookup_warning
                .as_deref()
                .unwrap_or_default()
                .contains("is empty")
        );
        assert!(warnings.iter().any(|w| w.contains("is empty")));
        assert_eq!(fixture.hits(&format!("/raw/o/repo/{SHA}/{FILE}")), 0);
    }
    #[test]
    fn enrich_missing_commit_requires_a_version_tag() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("dep");
        std::fs::create_dir_all(&root).unwrap();
        write_source_file(&root, FILE, BODY);
        let cache = dir.path().join("cache");
        let fixture = Fixture::start();
        fixture.route("/api/repos/o/repo/git/ref/tags/v1.2.3", 404, b"");
        fixture.route("/api/repos/o/repo/git/ref/tags/1.2.3", 404, b"");
        let package = make_package(
            &root,
            Some("https://github.com/o/repo"),
            SOURCE,
            lib_hashes(),
        );
        let mut candidates = vec![candidate(ReuseAction::RequestExtraction, FILE, 1, 4, 9)];
        let warnings = enrich_owned(&mut upstream(&cache, &fixture), &package, &mut candidates);
        let c = &candidates[0];
        assert!(c.source_url.is_none());
        assert!(c.issue_url.is_none());
        assert!(
            warnings
                .iter()
                .any(|w| w.contains("no verified upstream commit")),
            "{warnings:?}"
        );
    }

    #[test]
    fn enrich_discloses_a_source_transport_failure() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("dep");
        std::fs::create_dir_all(&root).unwrap();
        write_source_file(&root, FILE, BODY);
        vcs_info(&root, SHA);
        let cache = dir.path().join("cache");
        let dead_port = {
            let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            listener.local_addr().unwrap().port()
        };
        let dead_base = format!("http://127.0.0.1:{dead_port}");
        let mut up = Upstream::with_base(&cache, "http://127.0.0.1:1", &dead_base);
        let package = make_package(
            &root,
            Some("https://github.com/o/repo"),
            SOURCE,
            lib_hashes(),
        );
        let mut candidates = vec![candidate(ReuseAction::RequestExtraction, FILE, 1, 4, 9)];
        let warnings = enrich_owned(&mut up, &package, &mut candidates);
        let c = &candidates[0];
        assert!(c.source_url.is_none());
        assert!(
            c.lookup_warning
                .as_deref()
                .unwrap_or_default()
                .contains(&format!("upstream source {FILE} at {SHA}")),
            "{:?}",
            c.lookup_warning
        );
        assert!(
            warnings.iter().any(|w| w.contains("djd.net.")),
            "{warnings:?}"
        );
    }

    #[test]
    fn enrich_missing_source_at_commit_withholds_link() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("dep");
        std::fs::create_dir_all(&root).unwrap();
        write_source_file(&root, FILE, BODY);
        vcs_info(&root, SHA);
        let cache = dir.path().join("cache");
        let fixture = Fixture::start();
        fixture.route(&format!("/raw/o/repo/{SHA}/{FILE}"), 404, b"");
        let package = make_package(
            &root,
            Some("https://github.com/o/repo"),
            SOURCE,
            lib_hashes(),
        );
        let mut candidates = vec![candidate(ReuseAction::RequestExtraction, FILE, 1, 4, 9)];
        let warnings = enrich_owned(&mut upstream(&cache, &fixture), &package, &mut candidates);
        assert!(candidates[0].source_url.is_none());
        assert!(
            warnings.iter().any(|w| w.contains("is not at commit")),
            "{warnings:?}"
        );
    }

    #[test]
    fn enrich_memoizes_unavailable_lookups() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("dep");
        std::fs::create_dir_all(&root).unwrap();
        write_source_file(&root, FILE, BODY);
        vcs_info(&root, SHA);
        let cache = dir.path().join("cache");
        let dead_port = {
            let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            listener.local_addr().unwrap().port()
        };
        let api_base = format!("http://127.0.0.1:{dead_port}");
        let fixture = Fixture::start();
        fixture.route(&format!("/raw/o/repo/{SHA}/{FILE}"), 200, BODY.as_bytes());
        let mut up = Upstream::with_base(&cache, &api_base, &fixture.raw_base());
        let package = make_package(
            &root,
            Some("https://github.com/o/repo"),
            SOURCE,
            lib_hashes(),
        );
        let mut candidates = vec![
            candidate(ReuseAction::RequestExtraction, FILE, 1, 4, 9),
            candidate(ReuseAction::RequestExtraction, FILE, 1, 4, 9),
        ];
        let warnings = enrich_owned(&mut up, &package, &mut candidates);
        for c in &candidates {
            assert!(
                c.lookup_warning
                    .as_deref()
                    .unwrap_or_default()
                    .contains("issue lookup unavailable"),
                "{:?}",
                c.lookup_warning
            );
        }
        assert_eq!(warnings.len(), 1, "{warnings:?}");
        assert!(
            warnings.iter().any(|w| w.contains("djd.net.")),
            "{warnings:?}"
        );
    }

    #[test]
    fn enrich_unavailable_search_http_status() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("dep");
        std::fs::create_dir_all(&root).unwrap();
        write_source_file(&root, FILE, BODY);
        vcs_info(&root, SHA);
        let cache = dir.path().join("cache");
        let fixture = Fixture::start();
        fixture.route(&format!("/raw/o/repo/{SHA}/{FILE}"), 200, BODY.as_bytes());
        fixture.route("/api/search/issues", 500, b"{}");
        let package = make_package(
            &root,
            Some("https://github.com/o/repo"),
            SOURCE,
            lib_hashes(),
        );
        let mut candidates = vec![candidate(ReuseAction::RequestExtraction, FILE, 1, 4, 9)];
        let warnings = enrich_owned(&mut upstream(&cache, &fixture), &package, &mut candidates);
        let warning = candidates[0].lookup_warning.as_deref().unwrap_or_default();
        assert!(
            warning.contains("issue lookup unavailable HTTP 500"),
            "{warning}"
        );
        assert!(
            warnings.iter().any(|w| w.contains("HTTP 500")),
            "{warnings:?}"
        );
    }

    #[test]
    fn enrich_unavailable_search_invalid_json() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("dep");
        std::fs::create_dir_all(&root).unwrap();
        write_source_file(&root, FILE, BODY);
        vcs_info(&root, SHA);
        let cache = dir.path().join("cache");
        let fixture = Fixture::start();
        fixture.route(&format!("/raw/o/repo/{SHA}/{FILE}"), 200, BODY.as_bytes());
        fixture.route("/api/search/issues", 200, b"not json");
        let package = make_package(
            &root,
            Some("https://github.com/o/repo"),
            SOURCE,
            lib_hashes(),
        );
        let mut candidates = vec![candidate(ReuseAction::RequestExtraction, FILE, 1, 4, 9)];
        let warnings = enrich_owned(&mut upstream(&cache, &fixture), &package, &mut candidates);
        let warning = candidates[0].lookup_warning.as_deref().unwrap_or_default();
        assert!(
            warning.contains("issue lookup unavailable invalid JSON"),
            "{warning}"
        );
        assert!(
            warnings.iter().any(|w| w.contains("invalid JSON")),
            "{warnings:?}"
        );
    }

    #[test]
    fn enrich_search_skips_items_missing_fields() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("dep");
        std::fs::create_dir_all(&root).unwrap();
        write_source_file(&root, FILE, BODY);
        vcs_info(&root, SHA);
        let cache = dir.path().join("cache");
        let fixture = Fixture::start();
        fixture.route(&format!("/raw/o/repo/{SHA}/{FILE}"), 200, BODY.as_bytes());
        let p = permalink(SHA);
        let no_title = serde_json::json!({
            "number": 1,
            "state": "open",
            "body": format!("please extract {p}"),
            "html_url": "https://github.com/o/repo/issues/1",
        });
        let no_body = serde_json::json!({
            "number": 2,
            "state": "open",
            "title": TITLE,
        });
        let no_url = serde_json::json!({
            "number": 3,
            "state": "open",
            "title": TITLE,
            "body": format!("please extract {p}"),
        });
        fixture.route(
            "/api/search/issues",
            200,
            search_json(&[no_title, no_body, no_url])
                .to_string()
                .as_bytes(),
        );
        let package = make_package(
            &root,
            Some("https://github.com/o/repo"),
            SOURCE,
            lib_hashes(),
        );
        let mut candidates = vec![candidate(ReuseAction::RequestExtraction, FILE, 1, 4, 9)];
        let warnings = enrich_owned(&mut upstream(&cache, &fixture), &package, &mut candidates);
        assert!(warnings.is_empty(), "{warnings:?}");
        let c = &candidates[0];
        assert!(c.existing_issue.is_none());
        assert!(c.issue_url.is_some());
    }

    #[test]
    fn enrich_search_page_cap_discloses_truncation() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("dep");
        std::fs::create_dir_all(&root).unwrap();
        write_source_file(&root, FILE, BODY);
        vcs_info(&root, SHA);
        let cache = dir.path().join("cache");
        let fixture = Fixture::start();
        fixture.route(&format!("/raw/o/repo/{SHA}/{FILE}"), 200, BODY.as_bytes());
        let page: Vec<serde_json::Value> = (0u64..100)
            .map(|n| issue_item(n, "open", "unrelated tagged request", TITLE))
            .collect();
        let pages: Vec<(u16, Vec<u8>)> = (0u16..10)
            .map(|_| (200, search_json(&page).to_string().into_bytes()))
            .collect();
        fixture.page_route("/api/search/issues", pages);
        let package = make_package(
            &root,
            Some("https://github.com/o/repo"),
            SOURCE,
            lib_hashes(),
        );
        let mut candidates = vec![candidate(ReuseAction::RequestExtraction, FILE, 1, 4, 9)];
        let warnings = enrich_owned(&mut upstream(&cache, &fixture), &package, &mut candidates);
        let c = &candidates[0];
        assert!(c.existing_issue.is_none());
        assert!(c.issue_url.is_some());
        assert!(
            warnings
                .iter()
                .any(|w| w.contains("issue search was incomplete")),
            "{warnings:?}"
        );
        assert_eq!(fixture.hits("/api/search/issues"), 10);
    }

    #[test]
    fn enrich_provenance_write_failure_is_best_effort() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("dep");
        std::fs::create_dir_all(&root).unwrap();
        write_source_file(&root, FILE, BODY);
        vcs_info(&root, SHA);
        let cache = dir.path().join("cache");
        std::fs::create_dir_all(&cache).unwrap();
        std::fs::write(cache.join("private"), "blocked").unwrap();
        let fixture = Fixture::start();
        verified_fixture(&fixture);
        let package = make_package(
            &root,
            Some("https://github.com/o/repo"),
            SOURCE,
            lib_hashes(),
        );
        let mut candidates = vec![candidate(ReuseAction::RequestExtraction, FILE, 1, 4, 9)];
        let warnings = enrich_owned(&mut upstream(&cache, &fixture), &package, &mut candidates);
        assert!(warnings.is_empty(), "{warnings:?}");
        assert_eq!(
            candidates[0].source_url.as_deref(),
            Some(permalink(SHA).as_str())
        );
        assert!(!cache.join("private/upstream").exists());
    }

    #[test]
    fn enrich_provenance_rename_failure_removes_the_part() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("dep");
        std::fs::create_dir_all(&root).unwrap();
        write_source_file(&root, FILE, BODY);
        vcs_info(&root, SHA);
        let cache = dir.path().join("cache");
        let provenance_dir = cache.join("private/upstream");
        std::fs::create_dir_all(&provenance_dir).unwrap();
        std::fs::create_dir_all(provenance_dir.join("provenance.json")).unwrap();
        let fixture = Fixture::start();
        verified_fixture(&fixture);
        let package = make_package(
            &root,
            Some("https://github.com/o/repo"),
            SOURCE,
            lib_hashes(),
        );
        let mut candidates = vec![candidate(ReuseAction::RequestExtraction, FILE, 1, 4, 9)];
        let warnings = enrich_owned(&mut upstream(&cache, &fixture), &package, &mut candidates);
        assert!(warnings.is_empty(), "{warnings:?}");
        assert_eq!(
            candidates[0].source_url.as_deref(),
            Some(permalink(SHA).as_str())
        );
        let entries: Vec<String> = std::fs::read_dir(&provenance_dir)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(entries, vec!["provenance.json".to_string()], "{entries:?}");
    }

    #[test]
    fn enrich_tag_object_endpoint_failure_withholds_commit() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("dep");
        std::fs::create_dir_all(&root).unwrap();
        write_source_file(&root, FILE, BODY);
        let cache = dir.path().join("cache");
        let fixture = Fixture::start();
        fixture.route(
            "/api/repos/o/repo/git/ref/tags/v1.2.3",
            200,
            serde_json::json!({"object": {"type": "tag", "sha": TAG_OBJECT}})
                .to_string()
                .as_bytes(),
        );
        fixture.route(
            format!("/api/repos/o/repo/git/tags/{TAG_OBJECT}").as_str(),
            500,
            b"{}",
        );
        fixture.route("/api/repos/o/repo/git/ref/tags/1.2.3", 404, b"");
        let package = make_package(
            &root,
            Some("https://github.com/o/repo"),
            SOURCE,
            lib_hashes(),
        );
        let mut candidates = vec![candidate(ReuseAction::RequestExtraction, FILE, 1, 4, 9)];
        let warnings = enrich_owned(&mut upstream(&cache, &fixture), &package, &mut candidates);
        assert!(candidates[0].source_url.is_none());
        assert!(
            warnings
                .iter()
                .any(|w| w.contains("no verified upstream commit")),
            "{warnings:?}"
        );
    }

    #[test]
    fn enrich_tag_object_type_withholds_commit() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("dep");
        std::fs::create_dir_all(&root).unwrap();
        write_source_file(&root, FILE, BODY);
        let cache = dir.path().join("cache");
        let fixture = Fixture::start();
        fixture.route(
            "/api/repos/o/repo/git/ref/tags/v1.2.3",
            200,
            serde_json::json!({"object": {"type": "blob", "sha": TAG_OBJECT}})
                .to_string()
                .as_bytes(),
        );
        fixture.route("/api/repos/o/repo/git/ref/tags/1.2.3", 404, b"");
        let package = make_package(
            &root,
            Some("https://github.com/o/repo"),
            SOURCE,
            lib_hashes(),
        );
        let mut candidates = vec![candidate(ReuseAction::RequestExtraction, FILE, 1, 4, 9)];
        let warnings = enrich_owned(&mut upstream(&cache, &fixture), &package, &mut candidates);
        assert!(candidates[0].source_url.is_none());
        assert!(
            warnings
                .iter()
                .any(|w| w.contains("no verified upstream commit")),
            "{warnings:?}"
        );
    }

    #[test]
    fn enrich_git_checkout_without_origin_discloses_the_gap() {
        let dir = tempfile::tempdir().unwrap();
        let checkout = dir.path().join("checkout");
        let root = checkout.join("crates").join("dep");
        std::fs::create_dir_all(&root).unwrap();
        write_source_file(&root, FILE, BODY);
        let git_dir = checkout.join(".git");
        std::fs::create_dir_all(&git_dir).unwrap();
        std::fs::write(git_dir.join("config"), "[core]\n\trepo = true\n").unwrap();
        let cache = dir.path().join("cache");
        let fixture = Fixture::start();
        let source = format!("git+https://github.com/o/repo?branch=main#{PIN}");
        let package = make_package(
            &root,
            Some("https://github.com/o/repo"),
            &source,
            lib_hashes(),
        );
        let mut candidates = vec![candidate(ReuseAction::RequestExtraction, FILE, 1, 4, 9)];
        let warnings = enrich_owned(&mut upstream(&cache, &fixture), &package, &mut candidates);
        assert!(candidates[0].source_url.is_none());
        assert!(
            warnings
                .iter()
                .any(|w| w.contains("has no recorded origin")),
            "{warnings:?}"
        );
    }

    #[test]
    fn percent_decode_maps_a_plus_to_a_space() {
        assert_eq!(percent_decode("a%2Bb+c%20d"), "a+b c d");
    }
}
