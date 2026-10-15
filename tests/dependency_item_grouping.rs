//! Item grouping membership, canonical sizes and thresholds.

use dejadoc::{Dejadoc, DocTest, Kind, Report, SourceFile, TargetScan, group};

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

fn function_ids(report: &Report) -> Vec<&str> {
    let mut ids: Vec<&str> = report
        .groups
        .iter()
        .filter(|group| group.kind == Kind::Function)
        .map(|group| group.id.as_str())
        .collect();
    ids.sort_unstable();
    ids
}

fn target(text: &str) -> TargetScan {
    TargetScan {
        name: "item".to_owned(),
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

fn doc_blocks(text: &str) -> Vec<String> {
    let mut blocks: Vec<String> = Vec::new();
    let mut current: Option<String> = None;
    for raw in text.lines() {
        let line = raw.strip_prefix("//!").unwrap_or(raw);
        if line.trim_start().starts_with("```") {
            if let Some(block) = current.take() {
                blocks.push(block);
            } else {
                current = Some(String::new());
            }
        } else if let Some(block) = current.as_mut() {
            block.push_str(line);
            block.push('\n');
        }
    }
    blocks
}

fn assert_function_groups(report: &Report) {
    let function_groups: Vec<_> = report
        .groups
        .iter()
        .filter(|group| group.kind == Kind::Function)
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
            vec!["item::Owner::first", "item::Owner::same"],
            vec!["item::left::first", "item::left::same"],
            vec!["item::right::first", "item::right::same"],
        ]
    );
    assert!(function_groups.iter().all(|group| group.tokens >= 30));
    for group in &function_groups {
        assert!(group.remedy.is_some());
        assert!(group.sites.iter().all(|site| {
            site.file == "src/lib.rs" && site.line > 0 && site.end.is_some() && site.public
        }));
    }
}

#[test]
fn item_cli_fixture_preserves_exact_function_and_doctest_members() {
    let text = include_str!("fixtures/itemws/src/lib.rs");
    let blocks = doc_blocks(text);
    assert_eq!(blocks.len(), 5);
    let a13 = syn_canon::canonicalize(syn::parse_file(&blocks[0]).unwrap());
    let a14 = syn_canon::canonicalize(syn::parse_file(&blocks[2]).unwrap());
    assert_ne!(a13, a14);
    assert_eq!(
        a13,
        syn_canon::canonicalize(syn::parse_file(&blocks[1]).unwrap())
    );
    assert_eq!(
        a14,
        syn_canon::canonicalize(syn::parse_file(&blocks[3]).unwrap())
    );
    let target = target(text);
    for threshold in [0, 30] {
        let report = Dejadoc::default()
            .functions()
            .fn_min_tokens(threshold)
            .run_targets("", std::slice::from_ref(&target), &|_, _| None);
        assert_eq!(report.total, 5);
        assert_eq!(report.unique, 3);
        assert_eq!(report.functions, 9);
        assert_function_groups(&report);
        let doctest_groups: Vec<_> = report
            .groups
            .iter()
            .filter(|group| group.kind == Kind::Doctest)
            .collect();
        assert_eq!(doctest_groups.len(), 2);
        let positions: Vec<(usize, u32, u32)> = doctest_groups
            .iter()
            .map(|group| {
                let lines: Vec<u32> = group.sites.iter().map(|site| site.line).collect();
                (
                    group.tokens,
                    *lines.iter().min().unwrap(),
                    *lines.iter().max().unwrap(),
                )
            })
            .collect();
        assert!(
            positions
                .iter()
                .any(|entry| *entry == (a13.leaf_tokens(), 1, 7)),
            "{positions:?}"
        );
        assert!(
            positions
                .iter()
                .any(|entry| *entry == (a14.leaf_tokens(), 15, 22)),
            "{positions:?}"
        );
        for group in &doctest_groups {
            assert!(group.remedy.is_none());
            assert!(group.sites.iter().all(|site| {
                site.item == "item" && site.info.is_empty() && site.file == "src/lib.rs"
            }));
        }
        let a13_group = doctest_groups
            .iter()
            .find(|group| group.sites.iter().any(|site| site.line == 1))
            .unwrap();
        assert!(
            a13_group
                .sites
                .iter()
                .any(|site| site.line == 1 && site.end == Some(5))
        );
        assert!(
            a13_group
                .sites
                .iter()
                .any(|site| site.line == 7 && site.end == Some(13))
        );
        let a14_group = doctest_groups
            .iter()
            .find(|group| group.sites.iter().any(|site| site.line == 15))
            .unwrap();
        assert!(
            a14_group
                .sites
                .iter()
                .any(|site| site.line == 15 && site.end == Some(20))
        );
        assert!(
            a14_group
                .sites
                .iter()
                .any(|site| site.line == 22 && site.end == Some(30))
        );
        assert!(!report.groups.iter().any(|group| {
            group
                .sites
                .iter()
                .any(|site| site.item == "item" && site.line == 32)
        }));
    }
}

#[test]
fn item_cli_fixture_boundaries_and_reversed_sites() {
    let text = include_str!("fixtures/itemws/src/lib.rs");
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
        .filter(|group| group.kind == Kind::Function)
        .collect();
    assert_eq!(function_groups.len(), 3);
    for group in &function_groups {
        let included = scan(group.tokens);
        let excluded = scan(group.tokens + 1);
        assert!(included.groups.iter().any(|member| member.id == group.id));
        assert!(!excluded.groups.iter().any(|member| member.id == group.id));
    }
    for group in base
        .groups
        .iter()
        .filter(|group| group.kind == Kind::Doctest)
    {
        let included = Dejadoc::default().min_tokens(group.tokens).run_targets(
            "",
            std::slice::from_ref(&target),
            &|_, _| None,
        );
        let excluded = Dejadoc::default().min_tokens(group.tokens + 1).run_targets(
            "",
            std::slice::from_ref(&target),
            &|_, _| None,
        );
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
        .filter(|group| group.kind == Kind::Doctest)
        .flat_map(|group| group.sites.iter().cloned())
        .collect();
    let reversed_sites: Vec<DocTest> = doctest_sites.iter().rev().cloned().collect();
    let included = group(&doctest_sites, 1, 0);
    let reversed = group(&reversed_sites, 1, 0);
    assert_eq!(included.groups.len(), 2);
    assert_eq!(members(&included), members(&reversed));
    let included_ids: Vec<&str> = included
        .groups
        .iter()
        .map(|group| group.id.as_str())
        .collect();
    let reversed_ids: Vec<&str> = reversed
        .groups
        .iter()
        .map(|group| group.id.as_str())
        .collect();
    assert_eq!(included_ids, reversed_ids);
    let mut flagged: Vec<DocTest> = doctest_sites
        .into_iter()
        .filter(|site| matches!(site.line, 1 | 7))
        .collect();
    assert_eq!(flagged.len(), 2);
    flagged[1].info.push("should_panic".to_owned());
    assert_eq!(group(&flagged, 2, 0).groups, []);
    flagged[1].info = vec!["compile_fail".to_owned()];
    assert_eq!(group(&flagged, 2, 0).groups, []);
}
