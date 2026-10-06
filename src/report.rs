//! Report rendering.

use crate::{Kind, Remedy, Report};

use alloc::format;
use alloc::string::String;
use core::fmt::Write;

/// Render the report for humans.
#[must_use]
pub fn human(report: &Report, verbose: bool) -> String {
    let mut out = String::new();
    let _ = write!(
        &mut out,
        "{} doctests ({} unique), {} functions ({} unique), {} duplicated groups",
        report.total,
        report.unique,
        report.functions,
        report.unique_functions,
        report.groups.len()
    );
    if report.context_blocks > 0 {
        let _ = write!(
            &mut out,
            ", {} context blocks ({} unique), {} approximate groups",
            report.context_blocks,
            report.unique_context_blocks,
            report.context_groups.len(),
        );
    }
    let _ = writeln!(&mut out);
    if report.groups.is_empty() && report.context_groups.is_empty() {
        return out;
    }
    out.push('\n');
    for (i, group) in report.groups.iter().enumerate() {
        let _ = match group.kind {
            Kind::Doctest => writeln!(
                &mut out,
                "[{}] {} sites, {} tokens",
                group.id,
                group.sites.len(),
                group.tokens
            ),
            Kind::Function => writeln!(
                &mut out,
                "[{}] {} functions, {} tokens, {}",
                group.id,
                group.sites.len(),
                group.tokens,
                advice(group.remedy.unwrap_or(Remedy::Delete))
            ),
        };
        let deletes = group.kind == Kind::Function
            && group.remedy.unwrap_or(Remedy::Delete) == Remedy::Delete;
        for (copy, site) in group.sites.iter().enumerate() {
            let info = if deletes && copy > 0 && site.public {
                format!("   (public API, {PUBLIC_ADVICE})")
            } else if site.info.is_empty() {
                String::new()
            } else {
                format!("   ({})", site.info.join(", "))
            };
            let _ = writeln!(
                &mut out,
                "  {}:{}  {}{}",
                site.file.as_str(),
                site.line,
                site.item,
                info
            );
            if verbose {
                for line in site.code.lines() {
                    let _ = writeln!(&mut out, "    {line}");
                }
            }
        }
        if i + 1 < report.groups.len() {
            out.push('\n');
        }
    }
    context_human(&mut out, report);
    out.push('\n');
    if report.groups.iter().any(|g| g.kind == Kind::Doctest) {
        out.push_str(
            "Remove the copies listed above, or keep one on purpose with a `rust,dejadoc` fence\n",
        );
    }
    if report.groups.iter().any(|g| g.kind == Kind::Function) {
        out.push_str("Act on each function group as its advice says, or keep one on purpose with a `// dejadoc: allow` comment above it\n");
    }
    if !report.context_groups.is_empty() {
        out.push_str(
            "Context block groups are approximate syntax matches, and they name no copy to delete\n",
        );
    }
    out
}

/// The advice for a copy other crates may call, which deleting would break.
const PUBLIC_ADVICE: &str = "make it call the kept copy or deprecate it";

/// The advice the human report gives a function group.
fn advice(remedy: Remedy) -> &'static str {
    match remedy {
        Remedy::Delete => "delete or update all but the first",
        Remedy::MergeCfg => "one function under the merged cfg can replace them",
        Remedy::GenericOrMacro => "a generic or a macro can share one body",
        Remedy::HelperOrMacro => "a helper or a macro can share one body",
    }
}
fn context_human(out: &mut String, report: &Report) {
    for (i, group) in report.context_groups.iter().enumerate() {
        if i > 0 || !report.groups.is_empty() {
            out.push('\n');
        }
        let _ = writeln!(
            out,
            "[{}] {} sites, {} tokens, approximate",
            group.id,
            group.sites.len(),
            group.tokens
        );
        for site in &group.sites {
            let _ = writeln!(
                out,
                "  {}:{}:{}-{}:{}  {} ({})",
                site.file,
                site.line,
                site.column,
                site.end,
                site.end_column,
                site.item,
                crate::contexts::kind_name(site.kind)
            );
        }
    }
}

/// Render the report as JSON.
///
/// # Panics
///
/// `Report` is `Serialize`. Serialization failure would be a bug in this
/// crate.
#[must_use]
pub fn json(report: &Report) -> String {
    serde_json::to_string(report).expect("Report serializes to JSON")
}

/// Render the report as GitHub workflow annotations, one per copy to
/// remove plus one per approximate context site. They need no token, so a
/// fork pull request still shows them anchored in the diff.
#[must_use]
pub fn annotations(report: &Report, no_fail: bool) -> String {
    let level = if no_fail { "warning" } else { "error" };
    let mut out = String::new();
    for group in &report.groups {
        let Some((kept, copies)) = group.sites.split_first() else {
            continue;
        };
        for copy in copies {
            let (file, line, item, kept_item, kept_file, kept_line, id) = (
                escape_property(copy.file.as_str()),
                copy.line,
                escape(&copy.item),
                escape(&kept.item),
                escape(kept.file.as_str()),
                kept.line,
                &group.id,
            );
            let _ = match group.kind {
                Kind::Doctest => writeln!(
                    &mut out,
                    "::{level} file={file},line={line},title=dejadoc::{item} duplicates the doctest of {kept_item} at {kept_file}:{kept_line}, group {id}"
                ),
                Kind::Function => match group.remedy.unwrap_or(Remedy::Delete) {
                    Remedy::Delete if copy.public => writeln!(
                        &mut out,
                        "::{level} file={file},line={line},title=dejadoc::{item} repeats {kept_item} at {kept_file}:{kept_line} and is public API, {PUBLIC_ADVICE}, group {id}"
                    ),
                    Remedy::Delete => writeln!(
                        &mut out,
                        "::{level} file={file},line={line},title=dejadoc::{item} duplicates the function {kept_item} at {kept_file}:{kept_line}, group {id}"
                    ),
                    Remedy::MergeCfg => writeln!(
                        &mut out,
                        "::{level} file={file},line={line},title=dejadoc::{item} repeats {kept_item} at {kept_file}:{kept_line} under another cfg, one function under both cfgs can replace them, group {id}"
                    ),
                    Remedy::GenericOrMacro => writeln!(
                        &mut out,
                        "::{level} file={file},line={line},title=dejadoc::{item} repeats {kept_item} at {kept_file}:{kept_line} on another type, a generic or a macro can share it, group {id}"
                    ),
                    Remedy::HelperOrMacro => writeln!(
                        &mut out,
                        "::{level} file={file},line={line},title=dejadoc::{item} repeats {kept_item} at {kept_file}:{kept_line} and can't be deleted, a helper or a macro can share it, group {id}"
                    ),
                },
            };
        }
    }
    for group in &report.context_groups {
        for site in &group.sites {
            let (file, line, item, id) = (
                escape_property(site.file.as_str()),
                site.line,
                escape(&site.item),
                &group.id,
            );
            let _ = writeln!(
                &mut out,
                "::{level} file={file},line={line},col={},endLine={},endColumn={},title=dejadoc::approximate context block in {item}, group {id}",
                site.column,
                site.end,
                site.end_column.saturating_sub(1)
            );
        }
    }
    out
}

/// A workflow command message, with the characters that would end it
/// percent encoded.
fn escape(text: &str) -> String {
    text.replace('%', "%25")
        .replace('\r', "%0D")
        .replace('\n', "%0A")
}

/// A workflow command property value, which a `,` or `:` would also end.
fn escape_property(text: &str) -> String {
    escape(text).replace(':', "%3A").replace(',', "%2C")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{DocTest, Group, Remedy};
    use alloc::string::ToString;
    use alloc::vec;
    use alloc::vec::Vec;

    fn site(file: &str, line: u32, item: &str, code: &str, info: &[&str]) -> DocTest {
        DocTest {
            file: file.to_string(),
            line,
            end: None,
            item: item.to_string(),
            info: info.iter().map(ToString::to_string).collect(),
            code: code.to_string(),
            allow: false,
            self_type: None,
            public: false,
        }
    }

    fn report() -> Report {
        Report {
            total: 34,
            unique: 12,
            functions: 0,
            unique_functions: 0,
            context_blocks: 0,
            unique_context_blocks: 0,
            context_groups: Vec::new(),
            groups: vec![Group {
                kind: crate::Kind::Doctest,
                remedy: None,
                id: "ab12cd34".into(),
                hash: "ab12cd34".into(),
                unparsed: false,
                tokens: 5,
                sites: vec![
                    site("src/a.rs", 14, "m::a", "fn f() {}", &["no_run"]),
                    site("src/b.rs", 97, "m::b", "fn f() {}", &[]),
                ],
            }],
        }
    }

    #[test]
    fn human_verbose_prints_code_indented() {
        let out = human(&report(), true);
        assert!(out.contains("\n    fn f() {}\n"), "got: {out:?}");
        assert_eq!(out.matches("fn f() {}").count(), 2);
    }

    #[test]
    fn json_shape() {
        let value: serde_json::Value = serde_json::from_str(&json(&report())).unwrap();
        assert_eq!(value["total"], 34);
        assert_eq!(value["unique"], 12);
        let group = &value["groups"][0];
        // serde_json values order keys alphabetically; pin the exact sets.
        assert_eq!(
            group.as_object().unwrap().keys().collect::<Vec<_>>(),
            vec!["hash", "id", "kind", "sites", "tokens", "unparsed"]
        );
        assert_eq!(group["id"], "ab12cd34");
        assert_eq!(group["kind"], "doctest");
        assert_eq!(group["tokens"], 5);
        assert_eq!(group["unparsed"], false);
        assert_eq!(group["sites"].as_array().unwrap().len(), 2);
        let first = &group["sites"][0];
        assert_eq!(
            first.as_object().unwrap().keys().collect::<Vec<_>>(),
            vec!["code", "end", "file", "info", "item", "line"]
        );
        assert_eq!(first["file"], "src/a.rs");
        assert_eq!(first["line"], 14);
        assert_eq!(first["end"], serde_json::Value::Null);
        assert_eq!(first["item"], "m::a");
        assert_eq!(first["info"][0], "no_run");
    }

    /// A report holding one function group of two copies with `remedy`.
    fn function_report(remedy: Remedy) -> Report {
        Report {
            total: 0,
            unique: 0,
            functions: 7,
            unique_functions: 6,
            context_blocks: 0,
            unique_context_blocks: 0,
            context_groups: Vec::new(),
            groups: vec![Group {
                kind: crate::Kind::Function,
                id: "cd34ef56".into(),
                hash: "cd34ef56".into(),
                unparsed: false,
                tokens: 40,
                remedy: Some(remedy),
                sites: vec![
                    site("src/a.rs", 3, "m::A::run", "fn run() {}", &[]),
                    site("src/a.rs", 9, "m::B::run", "fn run() {}", &[]),
                ],
            }],
        }
    }

    #[test]
    fn a_public_copy_is_called_or_deprecated_never_deleted() {
        let mut report = function_report(Remedy::Delete);
        report.groups[0].sites[1].public = true;
        assert_eq!(
            annotations(&report, false),
            "::error file=src/a.rs,line=9,title=dejadoc::m::B::run repeats m::A::run at src/a.rs:3 and is public API, make it call the kept copy or deprecate it, group cd34ef56\n"
        );
        assert!(human(&report, false).contains(
            "  src/a.rs:9  m::B::run   (public API, make it call the kept copy or deprecate it)\n"
        ));
        // The kept copy being public changes nothing, and nor does a copy that isn't deleted.
        let mut report = function_report(Remedy::Delete);
        report.groups[0].sites[0].public = true;
        assert!(annotations(&report, false).contains("duplicates the function m::A::run"));
        assert!(!human(&report, false).contains("public API"));
        let mut report = function_report(Remedy::GenericOrMacro);
        report.groups[0].sites[1].public = true;
        assert!(!human(&report, false).contains("public API"));
        assert!(annotations(&report, false).contains("on another type"));
    }

    #[test]
    fn function_annotations_follow_the_remedy() {
        let head = "::error file=src/a.rs,line=9,title=dejadoc::m::B::run ";
        for (remedy, body) in [
            (
                Remedy::Delete,
                "duplicates the function m::A::run at src/a.rs:3",
            ),
            (
                Remedy::MergeCfg,
                "repeats m::A::run at src/a.rs:3 under another cfg, one function under both cfgs can replace them",
            ),
            (
                Remedy::GenericOrMacro,
                "repeats m::A::run at src/a.rs:3 on another type, a generic or a macro can share it",
            ),
            (
                Remedy::HelperOrMacro,
                "repeats m::A::run at src/a.rs:3 and can't be deleted, a helper or a macro can share it",
            ),
        ] {
            assert_eq!(
                annotations(&function_report(remedy), false),
                format!("{head}{body}, group cd34ef56\n")
            );
        }
    }

    #[test]
    fn annotations_mark_every_copy_but_the_first() {
        let out = annotations(&report(), false);
        let lines: Vec<&str> = out.lines().collect();
        assert_eq!(lines.len(), 1);
        assert_eq!(
            lines[0],
            "::error file=src/b.rs,line=97,title=dejadoc::m::b duplicates the doctest of m::a at src/a.rs:14, group ab12cd34"
        );
    }

    #[test]
    fn annotations_warn_when_the_run_cannot_fail() {
        let out = annotations(&report(), true);
        assert!(out.starts_with("::warning file=src/b.rs,line=97,"), "{out}");
    }

    #[test]
    fn annotations_are_empty_without_duplicates() {
        let clean = Report {
            total: 3,
            unique: 3,
            functions: 0,
            unique_functions: 0,
            context_blocks: 0,
            unique_context_blocks: 0,
            context_groups: Vec::new(),
            groups: Vec::new(),
        };
        assert_eq!(annotations(&clean, false), "");
    }

    #[test]
    fn annotations_skip_a_group_without_sites() {
        let mut empty = report();
        empty.groups[0].sites.clear();
        assert_eq!(annotations(&empty, false), "");
    }

    #[test]
    fn annotations_mark_every_later_site_of_a_larger_group() {
        let mut three = report();
        three.groups[0]
            .sites
            .push(site("src/c.rs", 3, "m::c", "fn f() {}", &[]));
        let out = annotations(&three, false);
        assert_eq!(out.lines().count(), 2);
        assert!(out.contains("file=src/c.rs,line=3,"), "{out}");
        assert!(
            out.lines().all(|l| l.contains("of m::a at src/a.rs:14")),
            "{out}"
        );
    }

    #[test]
    fn annotations_escape_the_message() {
        let mut odd = report();
        odd.groups[0].sites[0].item = "m::a%weird".into();
        let out = annotations(&odd, false);
        assert!(out.contains("of m::a%25weird at"), "{out}");
    }

    #[test]
    fn annotations_escape_the_file_property() {
        let mut odd = report();
        odd.groups[0].sites[1].file = "src/a,b:c%.rs".into();
        let out = annotations(&odd, false);
        assert!(
            out.starts_with("::error file=src/a%2Cb%3Ac%25.rs,line=97,"),
            "{out}"
        );
    }

    /// One approximate context group of two function bodies.
    fn context_report() -> Report {
        Report {
            total: 0,
            unique: 0,
            functions: 0,
            unique_functions: 0,
            groups: Vec::new(),
            context_blocks: 5,
            unique_context_blocks: 2,
            context_groups: vec![crate::ContextGroup {
                id: "ab12cd34".into(),
                hash: "ab12cd34".into(),
                tokens: 13,
                sites: vec![
                    context_site(
                        "src/a.rs",
                        9,
                        5,
                        12,
                        6,
                        "m::a",
                        crate::ContextKind::FunctionBody,
                    ),
                    context_site(
                        "src/b.rs",
                        41,
                        9,
                        44,
                        10,
                        "m::b",
                        crate::ContextKind::FunctionBody,
                    ),
                ],
            }],
        }
    }

    /// One context site at its half-open source range.
    fn context_site(
        file: &str,
        line: u32,
        column: usize,
        end: u32,
        end_column: usize,
        item: &str,
        kind: crate::ContextKind,
    ) -> crate::ContextSite {
        crate::ContextSite {
            file: file.to_string(),
            line,
            column,
            end,
            end_column,
            item: item.to_string(),
            kind,
        }
    }

    #[test]
    fn json_serializes_context_groups() {
        let value: serde_json::Value = serde_json::from_str(&json(&context_report())).unwrap();
        assert_eq!(value["context_blocks"], 5);
        assert_eq!(value["unique_context_blocks"], 2);
        let group = &value["context_groups"][0];
        assert_eq!(
            group.as_object().unwrap().keys().collect::<Vec<_>>(),
            vec!["hash", "id", "sites", "tokens"]
        );
        let site = &group["sites"][0];
        assert_eq!(
            site.as_object().unwrap().keys().collect::<Vec<_>>(),
            vec![
                "column",
                "end",
                "end_column",
                "file",
                "item",
                "kind",
                "line"
            ]
        );
        assert_eq!(site["kind"], "function-body");
        assert_eq!(site["column"], 5);
        assert_eq!(site["end_column"], 6);
    }

    #[test]
    fn context_annotations_mark_every_site_as_approximate() {
        let report = context_report();
        let errors = annotations(&report, false);
        let warnings = annotations(&report, true);
        assert_eq!(errors.lines().count(), report.context_groups[0].sites.len());
        assert!(errors.contains("file=src/a.rs,line=9,col=5,endLine=12,endColumn=5,"));
        assert!(errors.contains("file=src/b.rs,line=41,col=9,endLine=44,endColumn=9,"));
        assert!(errors.lines().all(|line| line.starts_with("::error ")));
        assert!(warnings.lines().all(|line| line.starts_with("::warning ")));
    }
}
