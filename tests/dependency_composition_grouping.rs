//! A single combined-family workspace exercises every M1-M8 fold at once.

use dejadoc::{Dejadoc, Kind, Remedy, SourceFile, TargetScan};

fn target(text: &str) -> TargetScan {
    TargetScan {
        name: "composition".to_owned(),
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
fn composition_cli_fixture_groups_every_combined_family() {
    let text = include_str!("fixtures/compositionws/src/lib.rs");
    let target = target(text);
    let report = Dejadoc::default().functions().fn_min_tokens(0).run_targets(
        "",
        std::slice::from_ref(&target),
        &|_, _| None,
    );
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
            vec![
                "composition::helpers_and_impls::first",
                "composition::helpers_and_impls::same",
            ],
            vec![
                "composition::literal_and_boolean::first",
                "composition::literal_and_boolean::same",
            ],
        ]
    );
    for group in &function_groups {
        assert_eq!(group.remedy, Some(Remedy::Delete));
        assert!(
            group
                .sites
                .iter()
                .all(|site| site.file == "src/lib.rs" && site.line > 0 && site.end.is_some())
        );
    }
    assert!(
        !function_members
            .iter()
            .flatten()
            .any(|item| item.ends_with("::different"))
    );
    let doctest_groups: Vec<_> = report
        .groups
        .iter()
        .filter(|group| group.kind == Kind::Doctest)
        .collect();
    assert_eq!(doctest_groups.len(), 1);
    assert_eq!(doctest_groups[0].sites.len(), 2);
    assert!(doctest_groups[0].remedy.is_none());
    assert!(
        doctest_groups[0]
            .sites
            .iter()
            .all(|site| site.item == "composition" && site.file == "src/lib.rs")
    );
}
