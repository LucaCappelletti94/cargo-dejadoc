//! Value grouping membership, canonical sizes and thresholds.

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
fn value_grouping_keeps_exact_passing_members_and_excludes_failing_variants() {
    for case in support::cases().iter().filter(|case| {
        case.family == "temporary"
            && case.expected == "same"
            && [
                "A07_copy_alias",
                "A08_inline_temporary",
                "A10_tuple_destructure",
                "A18_dead_primitive",
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
fn value_threshold_uses_one_canonical_size_in_both_site_orders() {
    for (left, right) in [
        (
            "fn f(input: u32) -> u32 { let mask = input & 15u32; let copied = mask; copied ^ 3u32 }",
            "fn g(input: u32) -> u32 { let mask = input & 15u32; mask ^ 3u32 }",
        ),
        (
            "fn f(input: u32) -> u32 { let mask = input & 15u32; mask ^ 3u32 }",
            "fn g(input: u32) -> u32 { (input & 15u32) ^ 3u32 }",
        ),
    ] {
        let a =
            syn_canon::canonicalize(syn::parse_str(&format!("fn main() {{ {left} }}")).unwrap());
        let b =
            syn_canon::canonicalize(syn::parse_str(&format!("fn main() {{ {right} }}")).unwrap());
        assert_eq!(a, b);
        assert_eq!(a.leaf_tokens(), 15);
        assert_eq!(b.leaf_tokens(), 15);
        assert_eq!(a.body_units(), 12);
        assert_eq!(b.body_units(), 12);
        let sites = [site("first", left, &[]), site("second", right, &[])];
        let reversed = [site("second", right, &[]), site("first", left, &[])];
        let included = group(&sites, 2, 15);
        let reversed_report = group(&reversed, 2, 15);
        assert_eq!(members(&included), [vec!["first", "second"]]);
        assert_eq!(members(&reversed_report), members(&included));
        assert_eq!(included.groups[0].id, reversed_report.groups[0].id);
        assert_eq!(included.groups[0].tokens, 15);
        assert_eq!(group(&sites, 2, 16).groups.len(), 0);
    }
}

fn form(source: &str) -> syn_canon::CanonicalForm {
    syn_canon::canonicalize(syn::parse_str(source).unwrap())
}

fn failing(source: &str) -> syn_canon::CanonicalForm {
    syn_canon::canonicalize_failing(syn::parse_str(source).unwrap())
}

fn same(left: &str, right: &str) {
    let a = form(left);
    let b = form(right);
    assert_eq!(a, b, "{left}\n{right}");
    assert_eq!(a.body_units(), b.body_units());
    assert_eq!(a.leaf_tokens(), b.leaf_tokens());
    assert_ne!(failing(left), failing(right));
}

#[test]
fn value_composition_erases_single_and_multiple_use_scalars() {
    same(
        "fn f(input: u32) -> (u32, u32) { let base = input & 15u32; let shifted = base >> 2u32; let widened = base | 4u32; let copied = shifted; (widened, copied ^ 3u32) }",
        "fn g(input: u32) -> (u32, u32) { ((input & 15u32) | 4u32, ((input & 15u32) >> 2u32) ^ 3u32) }",
    );
}

#[test]
fn value_composition_destructures_tuples_and_deletes_dead_bindings() {
    same(
        "fn f(input: u32) -> (u32, u32) { let (low, high) = (input & 15u32, input >> 4u32); let dead = 21u32; (low ^ high, low) }",
        "fn g(input: u32) -> (u32, u32) { ((input & 15u32) ^ (input >> 4u32), input & 15u32) }",
    );
}

#[test]
fn value_composition_composes_constant_boolean_and_idempotence_laws() {
    same(
        "fn f(input: u32, gate: bool) -> (u32, bool) { let low = input & 15u32; let summed = 2u32 + 3u32; let flagged = if gate { true } else { false }; (low ^ summed, flagged) }",
        "fn g(input: u32, gate: bool) -> (u32, bool) { ((input & 15u32) ^ 5u32, gate) }",
    );
    same(
        "fn f(input: u32) -> u32 { let high = input >> 4u32; let low = input & 15u32; let masked = low & low; masked ^ 3u32 }",
        "fn g(input: u32) -> u32 { (input & 15u32) ^ 3u32 }",
    );
}

fn function_ids(report: &Report) -> Vec<&str> {
    let mut ids: Vec<&str> = report
        .groups
        .iter()
        .filter(|group| group.kind == dejadoc::Kind::Function)
        .map(|group| group.id.as_str())
        .collect();
    ids.sort_unstable();
    ids
}
fn target(text: &str) -> TargetScan {
    TargetScan {
        name: "value".to_owned(),
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
fn value_cli_fixture_preserves_exact_function_and_doctest_members() {
    let text = include_str!("fixtures/valuews/src/lib.rs");
    let target = target(text);
    for threshold in [0, 13] {
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
                vec!["value::Owner::first", "value::Owner::same"],
                vec!["value::left::first", "value::left::same"],
                vec!["value::right::first", "value::right::same"],
            ]
        );
        assert!(function_groups.iter().all(|group| group.tokens == 13));
        for group in &function_groups {
            assert!(group.remedy.is_some());
            assert!(group.sites.iter().all(|site| {
                site.file == "src/lib.rs" && site.line > 0 && site.end.is_some() && site.public
            }));
        }
        let doctest_groups: Vec<_> = report
            .groups
            .iter()
            .filter(|group| group.kind == dejadoc::Kind::Doctest)
            .collect();
        assert_eq!(doctest_groups.len(), 1);
        let sites = &doctest_groups[0].sites;
        assert_eq!(sites.len(), 2);
        assert!(
            sites.iter().all(|site| site.item == "value"
                && site.info.is_empty()
                && site.file == "src/lib.rs")
        );
        assert_ne!(sites[0].line, sites[1].line);
        assert_eq!(doctest_groups[0].tokens, 14);
        let grouped: Vec<&str> = report
            .groups
            .iter()
            .flat_map(|group| group.sites.iter())
            .map(|site| site.item.as_str())
            .collect();
        assert!(!grouped.iter().any(|item| item.contains("different")));
        assert_eq!(grouped.iter().filter(|item| **item == "value").count(), 2);
    }
}

#[test]
fn value_cli_fixture_boundaries_and_reversed_sites() {
    let text = include_str!("fixtures/valuews/src/lib.rs");
    let mut target = target(text);
    let scan = |floor: usize| {
        Dejadoc::default()
            .functions()
            .fn_min_tokens(floor)
            .run_targets("", std::slice::from_ref(&target), &|_, _| None)
    };
    let base = scan(0);
    let function_groups: Vec<_> = base
        .groups
        .iter()
        .filter(|group| group.kind == dejadoc::Kind::Function)
        .collect();
    assert_eq!(function_groups.len(), 3);
    for group in &function_groups {
        let included = scan(13);
        let excluded = scan(14);
        assert!(included.groups.iter().any(|member| member.id == group.id));
        assert!(!excluded.groups.iter().any(|member| member.id == group.id));
    }
    target.files[0].parsed.items.reverse();
    let reversed_scan = Dejadoc::default().functions().fn_min_tokens(0).run_targets(
        "",
        std::slice::from_ref(&target),
        &|_, _| None,
    );
    assert_eq!(function_ids(&base), function_ids(&reversed_scan));
    let doctest_sites: Vec<DocTest> = base
        .groups
        .iter()
        .filter(|group| group.kind == dejadoc::Kind::Doctest)
        .flat_map(|group| group.sites.iter().cloned())
        .collect();
    assert_eq!(doctest_sites.len(), 2);
    let reversed_sites: Vec<DocTest> = doctest_sites.iter().rev().cloned().collect();
    let included = group(&doctest_sites, 1, 0);
    let reversed_doctests = group(&reversed_sites, 1, 0);
    assert_eq!(included.groups.len(), 1);
    assert_eq!(members(&included), members(&reversed_doctests));
    assert_eq!(included.groups[0].id, reversed_doctests.groups[0].id);
}
