//! Helper grouping membership, canonical sizes and thresholds in the actual fixture.

use dejadoc::{Dejadoc, DocTest, Group, Kind, Report, SourceFile, TargetScan, group};

const DOCTEST_TOKENS: usize = 50;

fn pick<'a>(module: &'a syn::ItemMod, name: &str) -> &'a syn::ItemFn {
    let (_, inner) = module.content.as_ref().unwrap();
    inner
        .iter()
        .find_map(|item| match item {
            syn::Item::Fn(function) if function.sig.ident == name => Some(function),
            _ => None,
        })
        .unwrap()
}

fn group_ids(report: &Report) -> Vec<&str> {
    report
        .groups
        .iter()
        .map(|group| group.id.as_str())
        .collect()
}

fn target(text: &str) -> TargetScan {
    TargetScan {
        name: "helperws".to_owned(),
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

fn scan(text: &str, fn_min_tokens: usize, min_tokens: usize, threshold: usize) -> Report {
    Dejadoc::default()
        .functions()
        .fn_min_tokens(fn_min_tokens)
        .min_tokens(min_tokens)
        .threshold(threshold)
        .run_targets("", std::slice::from_ref(&target(text)), &|_, _| None)
}

fn members(groups: &[Group]) -> Vec<Vec<&str>> {
    let mut out: Vec<Vec<&str>> = groups
        .iter()
        .map(|group| {
            let mut items: Vec<_> = group.sites.iter().map(|site| site.item.as_str()).collect();
            items.sort_unstable();
            items
        })
        .collect();
    out.sort_unstable();
    out
}

fn function_groups(report: &Report) -> Vec<Group> {
    report
        .groups
        .iter()
        .filter(|group| group.kind == Kind::Function)
        .cloned()
        .collect()
}

#[test]
fn helper_ws_fixture_preserves_exact_function_and_doctest_members() {
    let text = include_str!("fixtures/helperws/src/lib.rs");
    for fn_min_tokens in [0, 14] {
        let report = scan(text, fn_min_tokens, 0, 2);
        let groups = function_groups(&report);
        assert_eq!(
            members(&groups),
            [
                vec!["helperws::left::first", "helperws::left::same"],
                vec!["helperws::right::first", "helperws::right::same"],
            ]
        );
        for group in &groups {
            // Two six-unit lets plus two ordered tail references.
            assert_eq!(
                group.tokens, 14,
                "the body metric derives from the retained shape"
            );
            assert!(group.remedy.is_some());
            assert!(group.sites.iter().all(|site| {
                site.file == "src/lib.rs" && site.line > 0 && site.end.is_some() && site.public
            }));
        }
        let doctest_groups: Vec<Group> = report
            .groups
            .iter()
            .filter(|group| group.kind == Kind::Doctest)
            .cloned()
            .collect();
        assert_eq!(doctest_groups.len(), 1);
        let sites = &doctest_groups[0].sites;
        assert_eq!(sites.len(), 2);
        assert!(sites.iter().all(|site| site.item == "helperws"
            && site.info.is_empty()
            && site.file == "src/lib.rs"));
        assert_eq!(sites[0].line, 1);
        assert_eq!(sites[0].end, Some(5));
        assert_eq!(sites[1].line, 7);
        assert_eq!(sites[1].end, Some(11));
        assert_eq!(
            doctest_groups[0].tokens, DOCTEST_TOKENS,
            "the doctest size is the canonical leaf count"
        );
        assert_eq!(report.functions, 18, "every declared function is a site");
        assert_eq!(
            report.unique_functions, 16,
            "the two scheduled pairs collapse once each"
        );
        assert_eq!(report.total, 4);
        assert_eq!(report.unique, 3, "the failing fences keep distinct forms");
        let grouped: Vec<&str> = report
            .groups
            .iter()
            .flat_map(|group| group.sites.iter())
            .map(|site| site.item.as_str())
            .collect();
        assert!(!grouped.iter().any(|item| item.contains("changed")));
        assert!(!grouped.iter().any(|item| item.contains("caller")));
        assert!(!grouped.iter().any(|item| item.contains("shadow")));
    }
    // The borrowed views select the same boundary the report groups.
    let parsed = syn::parse_file(text).unwrap();
    let context = syn_canon::SourceContext::new(core::iter::once((&[][..], &parsed)));
    let left = parsed
        .items
        .iter()
        .find_map(|item| match item {
            syn::Item::Mod(mod_item) if mod_item.ident == "left" => Some(mod_item),
            _ => None,
        })
        .unwrap();
    let forms = |name: &str| {
        context
            .function(&pick(left, name).sig, &pick(left, name).block)
            .unwrap()
            .canonicalize()
    };
    let first = forms("first");
    let same = forms("same");
    let changed = forms("changed");
    assert_eq!(
        first, same,
        "the dead binding erases around the retained helper calls"
    );
    assert_eq!(first.body_units(), 14);
    assert_ne!(first, changed);
}

#[test]
fn helper_ws_fixture_boundaries_and_reversed_sites() {
    let text = include_str!("fixtures/helperws/src/lib.rs");
    let mut target = target(text);
    let base = scan(text, 0, 0, 2);
    let groups = function_groups(&base);
    assert_eq!(groups.len(), 2);
    for group in &groups {
        let included = scan(text, 14, 0, 2);
        let excluded = scan(text, 15, 0, 2);
        assert!(
            included.groups.iter().any(|member| member.id == group.id),
            "the floor at the body metric still includes the group"
        );
        assert!(
            !excluded.groups.iter().any(|member| member.id == group.id),
            "the floor past the body metric drops the group"
        );
    }
    let included = scan(text, 0, DOCTEST_TOKENS, 2);
    let excluded = scan(text, 0, DOCTEST_TOKENS + 1, 2);
    assert!(
        included
            .groups
            .iter()
            .any(|group| group.kind == Kind::Doctest)
    );
    assert!(
        !excluded
            .groups
            .iter()
            .any(|group| group.kind == Kind::Doctest)
    );
    let above = scan(text, 0, 0, 3);
    assert!(
        above.groups.is_empty(),
        "no group reaches the raised threshold"
    );
    target.files[0].parsed.items.reverse();
    let reversed_scan = Dejadoc::default().functions().fn_min_tokens(0).run_targets(
        "",
        std::slice::from_ref(&target),
        &|_, _| None,
    );
    assert_eq!(group_ids(&base), group_ids(&reversed_scan));
    assert_eq!(
        members(&base.groups),
        members(&reversed_scan.groups),
        "reversing the item order keeps every membership"
    );
    let doctest_sites: Vec<DocTest> = base
        .groups
        .iter()
        .filter(|group| group.kind == Kind::Doctest)
        .flat_map(|group| group.sites.iter().cloned())
        .collect();
    assert_eq!(doctest_sites.len(), 2);
    let reversed_sites: Vec<DocTest> = doctest_sites.iter().rev().cloned().collect();
    let included = group(&doctest_sites, 2, 0);
    let reversed = group(&reversed_sites, 2, 0);
    assert_eq!(members(&included.groups), members(&reversed.groups));
    assert_eq!(included.groups[0].id, reversed.groups[0].id);
}

#[test]
fn helper_ws_changed_member_keeps_the_census() {
    let text = include_str!("fixtures/helperws/src/lib.rs");
    let base = scan(text, 0, 0, 2);
    let changed = text.replacen(
        "let b = high(input); let a = low(input); (a, b)",
        "let b = low(input); let a = low(input); (a, b)",
        1,
    );
    let report = scan(&changed, 0, 0, 2);
    assert_eq!(
        members(&report.groups),
        [
            vec!["helperws", "helperws"],
            vec!["helperws::left::changed", "helperws::left::same"],
            vec!["helperws::right::first", "helperws::right::same"],
        ]
    );
    assert_eq!(report.functions, base.functions);
    assert_eq!(report.unique_functions, base.unique_functions);
    assert_eq!(report.total, base.total);
    assert_eq!(report.unique, base.unique);
    let before: std::collections::BTreeSet<_> =
        members(&base.groups).into_iter().flatten().collect();
    let after: std::collections::BTreeSet<_> =
        members(&report.groups).into_iter().flatten().collect();
    assert_eq!(
        before.difference(&after).copied().collect::<Vec<_>>(),
        ["helperws::left::first"]
    );
    assert_eq!(
        after.difference(&before).copied().collect::<Vec<_>>(),
        ["helperws::left::changed"]
    );
}
