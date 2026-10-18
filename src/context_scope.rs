//! Context corpus selection scopes and their selection against index records.

use crate::context_index::IndexedContext;
use alloc::fmt;
use alloc::string::{String, ToString};
use core::str::FromStr;

/// A selection over the workspace context corpus.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ContextScope {
    /// Every context of the workspace.
    Workspace,
    /// A compilation target by name or workspace-relative root source path.
    Crate(String),
    /// A physical source file, workspace-relative.
    File(String),
    /// A module and its descendant modules.
    Module(String),
    /// One exact module, without descendant modules.
    ModuleExact(String),
}

/// A context scope that could not be parsed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContextScopeError {
    /// The input that failed to parse.
    pub input: String,
}

impl fmt::Display for ContextScopeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "invalid context scope `{}`", self.input)
    }
}

impl core::error::Error for ContextScopeError {}

impl FromStr for ContextScope {
    type Err = ContextScopeError;

    fn from_str(input: &str) -> Result<Self, Self::Err> {
        if input == "workspace" {
            return Ok(ContextScope::Workspace);
        }
        let (prefix, value) = input.split_once(':').ok_or_else(|| ContextScopeError {
            input: input.to_string(),
        })?;
        let err = || ContextScopeError {
            input: input.to_string(),
        };
        match prefix {
            "crate" | "file" => {
                if value.is_empty() {
                    Err(err())
                } else {
                    let v = value.to_string();
                    Ok(match prefix {
                        "crate" => ContextScope::Crate(v),
                        _ => ContextScope::File(v),
                    })
                }
            }
            "module" if value.starts_with('-') => Err(err()),
            "module" => {
                if valid_module(value) {
                    Ok(ContextScope::Module(value.to_string()))
                } else {
                    Err(err())
                }
            }
            "module-exact" => {
                if valid_module(value) {
                    Ok(ContextScope::ModuleExact(value.to_string()))
                } else {
                    Err(err())
                }
            }
            _ => Err(err()),
        }
    }
}

impl fmt::Display for ContextScope {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ContextScope::Workspace => f.write_str("workspace"),
            ContextScope::Crate(value) => write!(f, "crate:{value}"),
            ContextScope::File(value) => write!(f, "file:{value}"),
            ContextScope::Module(value) => write!(f, "module:{value}"),
            ContextScope::ModuleExact(value) => write!(f, "module-exact:{value}"),
        }
    }
}

impl serde::Serialize for ContextScope {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.to_string())
    }
}

impl<'de> serde::Deserialize<'de> for ContextScope {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let input = String::deserialize(deserializer)?;
        input.parse().map_err(serde::de::Error::custom)
    }
}

impl ContextScope {
    /// Whether `record` falls inside this scope.
    #[must_use]
    pub(crate) fn matches(&self, record: &IndexedContext) -> bool {
        match self {
            ContextScope::Workspace => true,
            ContextScope::Crate(needle) => {
                *needle == record.crate_name || paths_equal(needle, &record.crate_root)
            }
            ContextScope::File(needle) => paths_equal(needle, &record.site.file),
            ContextScope::Module(needle) => segments_prefix(&record.module, needle),
            ContextScope::ModuleExact(needle) => segments_equal(&record.module, needle),
        }
    }
}

/// A module path of nonempty `::`-separated segments.
fn valid_module(value: &str) -> bool {
    if value.is_empty() || value.starts_with(':') || value.ends_with(':') {
        return false;
    }
    value
        .split("::")
        .all(|segment| !segment.is_empty() && !segment.contains(':'))
}

/// `needle`'s segments prefix `haystack`'s, the whole module included.
fn segments_prefix(haystack: &str, needle: &str) -> bool {
    let mut rest = haystack.split("::");
    for segment in needle.split("::") {
        match rest.next() {
            Some(hay) if hay == segment => {}
            _ => return false,
        }
    }
    true
}

/// `needle` and `haystack` name the same module.
fn segments_equal(haystack: &str, needle: &str) -> bool {
    let mut rest = haystack.split("::");
    for segment in needle.split("::") {
        match rest.next() {
            Some(hay) if hay == segment => {}
            _ => return false,
        }
    }
    rest.next().is_none()
}

/// A path compared the way source roots are written: forward or back slashes,
/// collapsed separators, leading `./` components dropped.
fn paths_equal(a: &str, b: &str) -> bool {
    let (mut ya, mut yb) = (a.as_bytes(), b.as_bytes());
    let mut leading_a = true;
    let mut leading_b = true;
    loop {
        let (ca, ra) = next_component(ya);
        let (cb, rb) = next_component(yb);
        match (ca, cb) {
            (None, None) => return true,
            (None, Some(y)) => return leading_b && y == b".",
            (Some(x), None) => return leading_a && x == b".",
            (Some(x), Some(y)) => {
                let drop_a = leading_a && x == b".";
                let drop_b = leading_b && y == b".";
                if drop_a {
                    ya = ra;
                }
                if drop_b {
                    yb = rb;
                }
                if !drop_a && !drop_b {
                    leading_a = false;
                    leading_b = false;
                    if x != y {
                        return false;
                    }
                    ya = ra;
                    yb = rb;
                }
            }
        }
    }
}

/// The next path component of `bytes`, and the remainder after it.
fn next_component(bytes: &[u8]) -> (Option<&[u8]>, &[u8]) {
    let mut i = 0;
    while i < bytes.len() && (bytes[i] == b'/' || bytes[i] == b'\\') {
        i += 1;
    }
    let start = i;
    while i < bytes.len() && bytes[i] != b'/' && bytes[i] != b'\\' {
        i += 1;
    }
    let component = if i == start {
        None
    } else {
        Some(&bytes[start..i])
    };
    while i < bytes.len() && (bytes[i] == b'/' || bytes[i] == b'\\') {
        i += 1;
    }
    (component, &bytes[i..])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ContextSite;
    use crate::context_index::IndexedContext;
    use crate::reuse::ContextAccess;
    use alloc::vec::Vec;

    fn record(module: &str, crate_name: &str, crate_root: &str, file: &str) -> IndexedContext {
        IndexedContext {
            form: "form".to_string(),
            tokens: 1,
            site: ContextSite {
                file: file.to_string(),
                line: 1,
                column: 1,
                end: 1,
                end_column: 2,
                item: module.to_string(),
                kind: crate::ContextKind::Block,
            },
            module: module.to_string(),
            crate_name: crate_name.to_string(),
            crate_root: crate_root.to_string(),
            access: ContextAccess::Unresolved,
            api_path: None,
            conditions: Vec::new(),
            extraction_notes: Vec::new(),
        }
    }

    fn parse(input: &str) -> ContextScope {
        input.parse().unwrap()
    }

    #[test]
    fn every_scope_form_parses_and_round_trips() {
        let forms: [(ContextScope, &str); 5] = [
            (ContextScope::Workspace, "workspace"),
            (ContextScope::Crate("app".to_string()), "crate:app"),
            (
                ContextScope::File("app/src/lib.rs".to_string()),
                "file:app/src/lib.rs",
            ),
            (
                ContextScope::Module("app::parser".to_string()),
                "module:app::parser",
            ),
            (
                ContextScope::ModuleExact("app::parser".to_string()),
                "module-exact:app::parser",
            ),
        ];
        for (scope, spelling) in forms {
            assert_eq!(parse(spelling), scope);
            assert_eq!(scope.to_string(), spelling);
            assert_eq!(parse(&scope.to_string()), scope);
        }
    }

    #[test]
    fn scope_serde_round_trips_through_its_spelling() {
        for scope in [
            ContextScope::Workspace,
            ContextScope::Crate("app".to_string()),
            ContextScope::File("app/src/lib.rs".to_string()),
            ContextScope::Module("app::parser".to_string()),
            ContextScope::ModuleExact("app::parser".to_string()),
        ] {
            let json = serde_json::to_string(&scope).unwrap();
            assert_eq!(json, alloc::format!("\"{scope}\""));
            assert_eq!(serde_json::from_str::<ContextScope>(&json).unwrap(), scope);
        }
    }

    #[test]
    fn malformed_scopes_are_rejected() {
        for input in [
            "",
            "workspace:",
            "workspace:app",
            "bogus:app",
            "crate:",
            "file:",
            "module:",
            "module-exact:",
            "module:a::",
            "module::a",
            "module-a::b",
            "module:app:::parser",
        ] {
            assert!(input.parse::<ContextScope>().is_err(), "{input}");
        }
    }

    #[test]
    fn workspace_scope_selects_every_record() {
        let record = record("app::parser", "app", "app/src/lib.rs", "app/src/parser.rs");
        assert!(ContextScope::Workspace.matches(&record));
    }

    #[test]
    fn crate_scope_selects_by_name_or_root_path() {
        let record = record("app", "app", "app/src/lib.rs", "app/src/lib.rs");
        assert!(parse("crate:app").matches(&record));
        assert!(parse("crate:app/src/lib.rs").matches(&record));
        assert!(parse("crate:./app/src/lib.rs").matches(&record));
        assert!(parse("crate:/app/src/lib.rs").matches(&record));
        assert!(!parse("crate:other").matches(&record));
        assert!(!parse("crate:app/src/nope.rs").matches(&record));
    }

    #[test]
    fn crate_scope_matches_backslashed_roots_like_forward_slashed_ones() {
        let record = record("app", "app", "app/src/lib.rs", "app/src/lib.rs");
        assert!(parse("crate:app\\src\\lib.rs").matches(&record));
    }

    #[test]
    fn file_scope_selects_the_physical_file() {
        let record = record("app::parser", "app", "app/src/lib.rs", "app/src/parser.rs");
        assert!(parse("file:app/src/parser.rs").matches(&record));
        assert!(parse("file:app\\src\\parser.rs").matches(&record));
        assert!(!parse("file:app/src/lib.rs").matches(&record));
    }

    #[test]
    fn module_scope_includes_descendants_but_exact_does_not() {
        let leaf = record(
            "app::parser::child",
            "app",
            "app/src/lib.rs",
            "app/src/parser.rs",
        );
        let root = record("app", "app", "app/src/lib.rs", "app/src/lib.rs");
        assert!(parse("module:app::parser").matches(&leaf));
        assert!(parse("module:app").matches(&leaf));
        assert!(parse("module-exact:app::parser::child").matches(&leaf));
        assert!(!parse("module-exact:app::parser").matches(&leaf));
        assert!(!parse("module:app::parser::child::deeper").matches(&leaf));
        assert!(!parse("module:app::parserx").matches(&leaf));
        assert!(!parse("module:appx").matches(&root));
        assert!(parse("module:app").matches(&root));
        assert!(parse("module-exact:app").matches(&root));
    }
    #[test]
    fn a_scope_error_reports_its_input() {
        let error = "bogus:app".parse::<ContextScope>().unwrap_err();
        assert_eq!(error.to_string(), "invalid context scope `bogus:app`");
        assert_eq!(error.input, "bogus:app");
    }

    #[test]
    fn a_lone_dot_component_matches_an_empty_path_only() {
        assert!(paths_equal("", "."));
        assert!(paths_equal(".", ""));
        assert!(!paths_equal(".", "x/y"));
        assert!(!paths_equal("x/y", "."));
    }
}
