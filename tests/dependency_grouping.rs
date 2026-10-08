//! Public grouping boundaries for dependency canonicalization.

#[path = "support/dependency.rs"]
mod support;

use dejadoc::{Dejadoc, DocTest, Report, SourceFile, TargetScan, group};

fn site(item: &str, code: &str, info: &[&str]) -> DocTest {
    DocTest {
        file: format!("{item}.rs"),
        line: 1,
        end: None,
        item: item.to_owned(),
        info: info.iter().map(|tag| (*tag).to_owned()).collect(),
        code: code.to_owned(),
        allow: false,
        self_type: None,
        public: false,
    }
}

fn members(report: &Report) -> Vec<Vec<&str>> {
    let mut groups: Vec<Vec<&str>> = report
        .groups
        .iter()
        .map(|group| {
            let mut items: Vec<&str> = group.sites.iter().map(|site| site.item.as_str()).collect();
            items.sort_unstable();
            items
        })
        .collect();
    groups.sort_unstable();
    groups
}

fn check_pair(case: &support::Case, failing: bool) {
    let info: &[&str] = if failing { &["compile_fail"] } else { &[] };
    let sites = [site("a", &case.a, info), site("b", &case.b, info)];
    let report = group(&sites, 1, 0);
    let expected = if case.family == "control" {
        vec![vec!["a", "b"]]
    } else {
        vec![vec!["a"], vec!["b"]]
    };
    assert_eq!(members(&report), expected, "{} failing={failing}", case.id);
}

#[test]
fn dependency_negatives_and_alpha_control_keep_their_actual_members() {
    for case in support::cases()
        .iter()
        .filter(|case| case.expected != "same" || case.family == "control")
    {
        check_pair(case, false);
    }
}

#[test]
fn dependency_failing_mode_preserves_every_baseline_relation() {
    for case in support::cases() {
        check_pair(case, true);
    }
}

#[test]
fn dependency_compile_fail_flags_separate_matching_source_forms() {
    let case = support::cases()
        .iter()
        .find(|case| case.id == "C01_alpha_control")
        .unwrap();
    let sites = [
        site("pass_a", &case.a, &[]),
        site("pass_b", &case.b, &[]),
        site("fail_a", &case.a, &["compile_fail"]),
        site("fail_b", &case.b, &["compile_fail"]),
    ];
    let report = group(&sites, 2, 0);
    assert_eq!(
        members(&report),
        vec![vec!["fail_a", "fail_b"], vec!["pass_a", "pass_b"]]
    );
}

#[test]
fn dependency_text_fallback_keeps_literal_distinctions_and_site_identity() {
    let sites = [
        site("compact", "let value = @ 1;", &[]),
        site("spaced", "let value    = @  1;", &[]),
        site("different", "let value = @ 2;", &[]),
    ];
    let report = group(&sites, 1, 0);
    assert_eq!(
        members(&report),
        vec![vec!["compact", "spaced"], vec!["different"]]
    );
    assert!(report.groups.iter().all(|group| group.unparsed));
}

#[test]
fn dependency_text_fallback_filters_by_original_word_count() {
    let sites = [
        site("first", "let value=@;", &["compile_fail"]),
        site("second", "let value=@;", &["compile_fail"]),
    ];
    let included = group(&sites, 2, 2);
    assert_eq!(members(&included), vec![vec!["first", "second"]]);
    assert!(included.groups[0].unparsed);
    assert_eq!(included.groups[0].tokens, 2);
    assert_eq!(members(&group(&sites, 2, 3)), Vec::<Vec<&str>>::new());
}

#[test]
fn dependency_nesting_fallback_cannot_merge_with_the_parsed_form() {
    let deep = format!("let value = {}1u32{};", "(".repeat(256), ")".repeat(256));
    let sites = [
        site("plain", "let value = 1u32;", &[]),
        site("parentheses", "let other = ((1u32));", &[]),
        site("deep", &deep, &[]),
    ];
    let report = group(&sites, 1, 0);
    assert_eq!(
        members(&report),
        vec![vec!["deep"], vec!["parentheses", "plain"]]
    );
    let parsed = report.groups.iter().find(|group| !group.unparsed).unwrap();
    let fallback = report.groups.iter().find(|group| group.unparsed).unwrap();
    assert_eq!(
        parsed
            .sites
            .iter()
            .map(|site| site.item.as_str())
            .collect::<Vec<_>>(),
        ["parentheses", "plain"]
    );
    assert_eq!(fallback.sites[0].item, "deep");
}

#[test]
fn dependency_function_groups_respect_original_module_scopes() {
    let text = "mod left {\n\
        pub fn first(input: u32) -> u32 { input & 15u32 }\n\
        pub fn second(value: u32) -> u32 { value & 15u32 }\n\
        }\n\
        mod right {\n\
        pub fn third(input: u32) -> u32 { input & 15u32 }\n\
        pub fn fourth(value: u32) -> u32 { value & 15u32 }\n\
        }\n";
    let target = TargetScan {
        name: "dependency".to_owned(),
        files: vec![SourceFile {
            path: "src/lib.rs".to_owned(),
            segments: Vec::new(),
            parsed: syn::parse_file(text).unwrap(),
            text: text.to_owned(),
            rustdoc: true,
        }],
        library: true,
    };
    let report =
        Dejadoc::default()
            .functions()
            .fn_min_tokens(0)
            .run_targets("", &[target], &|_, _| None);
    assert_eq!(
        members(&report),
        vec![
            vec!["dependency::left::first", "dependency::left::second"],
            vec!["dependency::right::fourth", "dependency::right::third"]
        ]
    );
    assert!(
        report
            .groups
            .iter()
            .flat_map(|group| &group.sites)
            .all(|site| site.public)
    );
}
