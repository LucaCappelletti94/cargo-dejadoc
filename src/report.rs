//! Report rendering.

use crate::Report;

use std::fmt::Write;

/// Render the report for humans.
#[must_use]
pub fn human(report: &Report, verbose: bool) -> String {
    let mut out = String::new();
    let _ = writeln!(
        &mut out,
        "{} doctests, {} unique, {} duplicated groups",
        report.total,
        report.unique,
        report.groups.len()
    );
    if report.groups.is_empty() {
        return out;
    }
    out.push('\n');
    for (i, group) in report.groups.iter().enumerate() {
        let _ = writeln!(
            &mut out,
            "[{}] {} sites, {} tokens",
            group.id,
            group.sites.len(),
            group.tokens
        );
        for site in &group.sites {
            let info = if site.info.is_empty() {
                String::new()
            } else {
                format!("   ({})", site.info.join(", "))
            };
            let _ = writeln!(
                &mut out,
                "  {}:{}  {}{}",
                site.file.display(),
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
    out
}

/// Render the report as JSON.
///
/// # Panics
///
/// `Report` is `Serialize`; serialization failure would be a bug in this
/// crate.
#[must_use]
pub fn json(report: &Report) -> String {
    serde_json::to_string(report).expect("Report serializes to JSON")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{DocTest, Group};

    fn site(file: &str, line: u32, item: &str, code: &str, info: &[&str]) -> DocTest {
        DocTest {
            file: std::path::PathBuf::from(file),
            line,
            item: item.to_string(),
            info: info.iter().map(ToString::to_string).collect(),
            code: code.to_string(),
            allow: false,
        }
    }

    fn report() -> Report {
        Report {
            total: 34,
            unique: 12,
            groups: vec![Group {
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
    fn human_summary_only_when_clean() {
        let report = Report {
            total: 3,
            unique: 3,
            groups: Vec::new(),
        };
        assert_eq!(
            human(&report, false),
            "3 doctests, 3 unique, 0 duplicated groups\n"
        );
    }

    #[test]
    fn human_group_section_without_code() {
        assert_eq!(
            human(&report(), false),
            "34 doctests, 12 unique, 1 duplicated groups\n\n[ab12cd34] 2 sites, 5 tokens\n  src/a.rs:14  m::a   (no_run)\n  src/b.rs:97  m::b\n"
        );
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
            vec!["hash", "id", "sites", "unparsed"]
        );
        assert_eq!(group["id"], "ab12cd34");
        assert_eq!(group["unparsed"], false);
        assert_eq!(group["sites"].as_array().unwrap().len(), 2);
        let first = &group["sites"][0];
        assert_eq!(
            first.as_object().unwrap().keys().collect::<Vec<_>>(),
            vec!["code", "file", "info", "item", "line"]
        );
        assert_eq!(first["file"], "src/a.rs");
        assert_eq!(first["line"], 14);
        assert_eq!(first["item"], "m::a");
        assert_eq!(first["info"][0], "no_run");
    }

    #[test]
    fn human_multiple_groups_separated_by_blank_line() {
        let report = Report {
            total: 4,
            unique: 2,
            groups: vec![
                Group {
                    id: "ab12cd34".into(),
                    hash: "ab12cd34".into(),
                    unparsed: false,
                    tokens: 5,
                    sites: vec![
                        site("src/a.rs", 14, "m::a", "fn f() {}", &[]),
                        site("src/b.rs", 97, "m::b", "fn f() {}", &[]),
                    ],
                },
                Group {
                    id: "ef56gh78".into(),
                    hash: "ef56gh78".into(),
                    unparsed: false,
                    tokens: 3,
                    sites: vec![site("src/c.rs", 5, "n::c", "fn g() {}", &[])],
                },
            ],
        };
        assert_eq!(
            human(&report, false),
            "4 doctests, 2 unique, 2 duplicated groups\n\n[ab12cd34] 2 sites, 5 tokens\n  src/a.rs:14  m::a\n  src/b.rs:97  m::b\n\n[ef56gh78] 1 sites, 3 tokens\n  src/c.rs:5  n::c\n"
        );
    }
}
