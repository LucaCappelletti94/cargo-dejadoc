//! Primitive algebra membership, metadata and canonical size boundaries.

#[path = "support/dependency.rs"]
mod support;

#[path = "support/fixture_group.rs"]
mod fixture_group;
#[path = "support/group_members.rs"]
mod group_members;
#[path = "support/public_sites.rs"]
mod public_sites;
#[path = "support/site_src.rs"]
mod site_src;

use echoes::{Echoes, SourceFile, TargetScan, group};
use fixture_group::assert_fixture_group;
use group_members::members;
use public_sites::assert_public_sites;
use site_src::site;

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
        assert_fixture_group(case);
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
        assert_eq!(members(&included.groups), [vec!["first", "second"]]);
        assert_eq!(members(&reversed_report.groups), members(&included.groups));
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
            cfg: Vec::new(),
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
    for threshold in [0, 10] {
        let report = Echoes::default()
            .functions()
            .fn_min_tokens(threshold)
            .run_targets("", std::slice::from_ref(&target), &|_, _| None);
        let function_groups: Vec<_> = report
            .groups
            .iter()
            .filter(|group| group.kind == echoes::Kind::Function)
            .collect();
        let function_members = members(function_groups.iter().copied());
        assert_eq!(
            function_members,
            [
                vec!["algebra::Owner::first", "algebra::Owner::same"],
                vec!["algebra::left::first", "algebra::left::same"],
                vec!["algebra::right::first", "algebra::right::same"],
            ]
        );
        assert!(function_groups.iter().all(|group| group.tokens == 10));
        assert_public_sites(&function_groups);
    }
}
