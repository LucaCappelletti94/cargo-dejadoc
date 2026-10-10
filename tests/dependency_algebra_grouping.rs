//! Primitive algebra membership, metadata and canonical size boundaries.

#[path = "support/dependency.rs"]
mod support;

use dejadoc::{Dejadoc, DocTest, Report, SourceFile, TargetScan, group};

fn site(item: &str, code: &str, info: &[&str]) -> DocTest {
    DocTest {
        file: format!("src/{item}.rs"),
        line: 7,
        end: Some(11),
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
            let mut items: Vec<_> = group.sites.iter().map(|site| site.item.as_str()).collect();
            items.sort_unstable();
            items
        })
        .collect();
    groups.sort_unstable();
    groups
}

#[test]
fn algebra_grouping_preserves_exact_fixture_members_and_failing_flags() {
    for case in support::cases().iter().filter(|case| {
        case.family == "algebra"
            && case.expected == "same"
            && [
                "A01_commute_bitwise",
                "A02_associate_bitwise",
                "A03_reverse_comparison",
                "A12_bitwise_idempotence",
            ]
            .contains(&case.id.as_str())
    }) {
        let sites = [
            site("pass_a", &case.a, &[]),
            site("pass_b", &case.b, &[]),
            site("fail_a", &case.a, &["compile_fail"]),
            site("fail_b", &case.b, &["compile_fail"]),
        ];
        let report = group(&sites, 2, 0);
        assert_eq!(members(&report), [vec!["pass_a", "pass_b"]], "{}", case.id);
        assert!(report.groups[0].sites.iter().all(|member| {
            member.line == 7 && member.end == Some(11) && member.info.is_empty()
        }));
    }
}

#[test]
fn algebra_threshold_uses_one_canonical_size_in_both_site_orders() {
    for (left, right) in [
        ("fn f(a: u32) -> u32 { a & a }", "fn g(x: u32) -> u32 { x }"),
        (
            "fn f(a: u32) -> u32 { (a ^ 1u32) ^ 2u32 }",
            "fn g(x: u32) -> u32 { 3u32 ^ x }",
        ),
    ] {
        let a =
            syn_canon::canonicalize(syn::parse_str(&format!("fn main() {{ {left} }}")).unwrap());
        let b =
            syn_canon::canonicalize(syn::parse_str(&format!("fn main() {{ {right} }}")).unwrap());
        assert_eq!(a, b);
        assert_eq!(a.leaf_tokens(), b.leaf_tokens());
        assert_eq!(a.body_units(), b.body_units());
        let sites = [site("first", left, &[]), site("second", right, &[])];
        let reversed = [sites[1].clone(), sites[0].clone()];
        let included = group(&sites, 2, a.leaf_tokens());
        let reversed_report = group(&reversed, 2, a.leaf_tokens());
        assert_eq!(members(&included), [vec!["first", "second"]]);
        assert_eq!(members(&reversed_report), members(&included));
        assert_eq!(included.groups[0].id, reversed_report.groups[0].id);
        assert_eq!(included.groups[0].tokens, a.leaf_tokens());
        assert_eq!(group(&sites, 2, a.leaf_tokens() + 1).groups.len(), 0);
    }
}

fn target(text: &str) -> TargetScan {
    TargetScan {
        name: "algebra".to_owned(),
        files: vec![SourceFile {
            path: "src/lib.rs".to_owned(),
            segments: Vec::new(),
            parsed: syn::parse_file(text).unwrap(),
            text: text.to_owned(),
            rustdoc: true,
        }],
        library: true,
    }
}

#[test]
fn algebra_contextual_function_groups_preserve_modules_owners_and_neighbours() {
    let text = include_str!("fixtures/algebraws/src/lib.rs");
    let target = target(text);
    for threshold in [0, 30] {
        let report = Dejadoc::default()
            .functions()
            .fn_min_tokens(threshold)
            .run_targets("", std::slice::from_ref(&target), &|_, _| None);
        let function_groups: Vec<_> = report
            .groups
            .iter()
            .filter(|group| group.kind == dejadoc::Kind::Function)
            .collect();
        let mut function_members: Vec<Vec<_>> = function_groups
            .iter()
            .map(|group| {
                let mut items: Vec<_> = group.sites.iter().map(|site| site.item.as_str()).collect();
                items.sort_unstable();
                items
            })
            .collect();
        function_members.sort_unstable();
        assert_eq!(
            function_members,
            [
                vec!["algebra::Owner::first", "algebra::Owner::same"],
                vec!["algebra::left::first", "algebra::left::same"],
                vec!["algebra::right::first", "algebra::right::same"],
            ]
        );
        assert!(function_groups.iter().all(|group| group.tokens >= 30));
        for group in function_groups {
            assert!(group.remedy.is_some());
            assert!(group.sites.iter().all(|site| {
                site.file == "src/lib.rs" && site.line > 0 && site.end.is_some() && site.public
            }));
        }
    }
}
