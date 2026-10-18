//! Fixture builders for the scenario reports.

use serde_json::{Value, json};

use std::fmt::Write;

/// One duplicate site of a function or a context.
pub fn site(file: &str, line: i64, end: i64, item: &str) -> Value {
    json!({
        "file": file,
        "line": line,
        "column": 1,
        "end": end,
        "end_column": 2,
        "item": item,
        "kind": "function",
    })
}

/// One duplicate doctest site.
pub fn doc_site(file: &str, line: i64, end: i64, item: &str) -> Value {
    json!({
        "file": file,
        "line": line,
        "end": end,
        "item": item,
        "info": [],
        "code": "let total = value + 1u32;",
    })
}

/// One duplicate group.
pub fn group(gid: &str, hash: &str, sites: &[Value]) -> Value {
    json!({
        "kind": "doctest",
        "id": gid,
        "hash": hash,
        "unparsed": false,
        "tokens": 20,
        "sites": sites,
    })
}

/// One selected-corpus context group.
pub fn ctx_group(gid: &str, hash: &str, sites: &[Value]) -> Value {
    json!({
        "id": gid,
        "hash": hash,
        "tokens": 35,
        "sites": sites,
    })
}

/// One dependency coverage block.
pub fn coverage(
    complete: bool,
    packages: i64,
    indexed_files: i64,
    cached_files: i64,
    failures: &[&str],
    warnings: &[&str],
) -> Value {
    json!({
        "complete": complete,
        "packages": packages,
        "indexed_files": indexed_files,
        "cached_files": cached_files,
        "failures": failures,
        "warnings": warnings,
    })
}

/// One dependency reuse report.
pub fn reuse(groups: &[Value], cov: Option<Value>) -> Value {
    json!({
        "groups": groups,
        "coverage": cov.unwrap_or_else(|| coverage(true, 2, 3, 1, &[], &[])),
    })
}

/// One dependency reuse group.
pub fn reuse_group(hash: &str, local: &Value, candidates: &[Value]) -> Value {
    json!({
        "hash": hash,
        "tokens": 35,
        "local": local,
        "candidates": candidates,
    })
}

/// One dependency candidate, set a field at a time.
pub struct Cand {
    package: String,
    version: String,
    depth: i64,
    item: String,
    action: String,
    access: String,
    repository: Option<String>,
    api_path: Option<String>,
    source_url: Option<String>,
    issue_url: Option<String>,
    existing_issue: Option<Value>,
    lookup_warning: Option<String>,
}

impl Cand {
    /// A candidate without any optional field set.
    pub fn new(package: &str, version: &str, depth: i64, item: &str, action: &str) -> Self {
        Self {
            package: package.to_string(),
            version: version.to_string(),
            depth,
            item: item.to_string(),
            action: action.to_string(),
            access: "public-function".into(),
            repository: None,
            api_path: None,
            source_url: None,
            issue_url: None,
            existing_issue: None,
            lookup_warning: None,
        }
    }

    pub fn access(mut self, access: &str) -> Self {
        self.access = access.to_string();
        self
    }

    pub fn repository(mut self, repository: &str) -> Self {
        self.repository = Some(repository.to_string());
        self
    }

    pub fn api_path(mut self, api_path: &str) -> Self {
        self.api_path = Some(api_path.to_string());
        self
    }

    pub fn source_url(mut self, source_url: &str) -> Self {
        self.source_url = Some(source_url.to_string());
        self
    }

    pub fn issue_url(mut self, issue_url: &str) -> Self {
        self.issue_url = Some(issue_url.to_string());
        self
    }

    pub fn existing_issue(mut self, url: &str, state: &str) -> Self {
        self.existing_issue = Some(json!({ "url": url, "state": state }));
        self
    }

    pub fn lookup_warning(mut self, warning: &str) -> Self {
        self.lookup_warning = Some(warning.to_string());
        self
    }

    /// The candidate as a report object.
    pub fn build(self) -> Value {
        json!({
            "package": self.package,
            "version": self.version,
            "source": format!("registry+https://github.com/rust-lang/crates.io-index#sha256:{}", self.package),
            "depth": self.depth,
            "site": site(&format!("{}/src/lib.rs", self.package), 20, 24, &self.item),
            "access": self.access,
            "conditions": [],
            "notes": [],
            "action": self.action,
            "repository": self.repository,
            "api_path": self.api_path,
            "source_url": self.source_url,
            "issue_url": self.issue_url,
            "existing_issue": self.existing_issue,
            "lookup_warning": self.lookup_warning,
        })
    }
}

/// One scan report.
pub fn report(
    groups: &[Value],
    context_groups: &[Value],
    reuse_rep: Option<&Value>,
    total: i64,
    unique: i64,
) -> Value {
    let context_blocks: usize = context_groups
        .iter()
        .map(|g| {
            g.get("sites")
                .and_then(Value::as_array)
                .map_or(0usize, Vec::len)
        })
        .sum();
    let mut r = json!({
        "total": total,
        "unique": unique,
        "functions": 0,
        "unique_functions": 0,
        "groups": groups,
        "context_blocks": context_blocks,
        "unique_context_blocks": context_groups.len(),
        "context_groups": context_groups,
    });
    if let Some(rep) = reuse_rep {
        r.as_object_mut()
            .expect("a report object")
            .insert("reuse".to_string(), rep.clone());
    }
    r
}

/// The settled extraction request URL, title and body percent-encoded.
pub fn issue_url(owner: &str, repo: &str, item: &str) -> String {
    let title = format!("[DEJADOC] Extract a public helper from {item}");
    let body = "Please extract the following range into a public function so downstream crates can reuse it: https://github.com/{owner}/{repo}/blob/abcdef1234567890abcdef1234567890abcdef12/src/lib.rs#L20-L24\n<!-- Check open and closed issues and pull requests for this range before submitting, and state your willingness to contribute a pull request. -->";
    format!(
        "https://github.com/{owner}/{repo}/issues/new?title={}&body={}",
        pct(&title),
        pct(body)
    )
}

/// Percent-encode every byte outside the unreserved set, the `quote` behaviour with no safe set.
fn pct(s: &str) -> String {
    let mut out = String::new();
    for &b in s.as_bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(char::from(b));
            }
            _ => {
                out.write_fmt(format_args!("%{b:02X}"))
                    .expect("a write to a String");
            }
        }
    }
    out
}
