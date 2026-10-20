//! Grouping indexed context records into duplicate groups.

use crate::context_index::IndexedContext;
use crate::{ContextGroup, ContextKind, ContextSite};
use alloc::collections::{BTreeMap, BTreeSet};
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use serde::Deserialize;

/// The grouped context blocks of `local` and `corpus` records, with the
/// selected total and the unique form count.
pub(crate) fn group(
    local: &[&IndexedContext],
    corpus: &[&IndexedContext],
    floor: usize,
    threshold: usize,
) -> (usize, usize, Vec<ContextGroup>) {
    type RangeKey = (String, u32, usize, u32, usize);
    struct Entry {
        tokens: usize,
        sites: BTreeMap<RangeKey, ContextSite>,
        anchor: bool,
    }
    let range = |record: &IndexedContext| -> RangeKey {
        (
            record.site.file.clone(),
            record.site.line,
            record.site.column,
            record.site.end,
            record.site.end_column,
        )
    };

    let mut local_ranges = BTreeSet::new();
    for record in local {
        local_ranges.insert(range(record));
    }
    let total = local_ranges.len();

    let mut local_forms = BTreeSet::new();
    for record in local {
        if record.tokens >= floor {
            local_forms.insert(record.form.clone());
        }
    }
    let unique = local_forms.len();

    let mut by_form: BTreeMap<String, Entry> = BTreeMap::new();
    for (record, anchor) in local
        .iter()
        .map(|record| (*record, true))
        .chain(corpus.iter().map(|record| (*record, false)))
    {
        if record.tokens < floor {
            continue;
        }
        let entry = by_form.entry(record.form.clone()).or_insert(Entry {
            tokens: record.tokens,
            sites: BTreeMap::new(),
            anchor: false,
        });
        entry.anchor = anchor || entry.anchor;
        entry
            .sites
            .entry(range(record))
            .or_insert_with(|| record.site.clone());
    }

    let mut groups: Vec<ContextGroup> = by_form
        .into_iter()
        .filter(|(_, entry)| entry.anchor && entry.sites.len() >= threshold)
        .map(|(form, entry)| {
            let hash = blake3::hash(form.as_bytes()).to_hex().to_string();
            ContextGroup {
                id: hash[..8].to_string(),
                hash,
                tokens: entry.tokens,
                sites: entry.sites.into_values().collect(),
            }
        })
        .collect();
    let covered: Vec<bool> = groups
        .iter()
        .enumerate()
        .map(|(index, group)| {
            groups.iter().enumerate().any(|(other, outer)| {
                index != other
                    && group
                        .sites
                        .iter()
                        .all(|site| outer.sites.iter().any(|parent| contains(parent, site)))
            })
        })
        .collect();
    groups = groups
        .into_iter()
        .zip(covered)
        .filter_map(|(group, covered)| (!covered).then_some(group))
        .collect();
    groups.sort_by(|a, b| a.hash.cmp(&b.hash));
    (total, unique, groups)
}

/// `outer` strictly encloses `inner` on one file.
fn contains(outer: &ContextSite, inner: &ContextSite) -> bool {
    let start = (outer.line, outer.column);
    let end = (outer.end, outer.end_column);
    let inner_start = (inner.line, inner.column);
    let inner_end = (inner.end, inner.end_column);
    outer.file == inner.file
        && start <= inner_start
        && inner_end <= end
        && (start, end) != (inner_start, inner_end)
}

#[expect(
    clippy::trivially_copy_pass_by_ref,
    reason = "The serializer callback requires a reference"
)]
pub(crate) fn serialize_kind<S: serde::Serializer>(
    kind: &ContextKind,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    serializer.serialize_str(kind_name(*kind))
}

pub(crate) fn deserialize_kind<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<ContextKind, D::Error> {
    let name = String::deserialize(deserializer)?;
    match name.as_str() {
        "function-body" => Ok(ContextKind::FunctionBody),
        "block" => Ok(ContextKind::Block),
        "arm" => Ok(ContextKind::Arm),
        "closure" => Ok(ContextKind::Closure),
        other => Err(serde::de::Error::unknown_variant(
            other,
            &["function-body", "block", "arm", "closure"],
        )),
    }
}

pub(crate) fn kind_name(kind: ContextKind) -> &'static str {
    match kind {
        ContextKind::FunctionBody => "function-body",
        ContextKind::Block => "block",
        ContextKind::Arm => "arm",
        ContextKind::Closure => "closure",
    }
}

#[cfg(test)]
mod tests {
    use super::group;
    use crate::context_index::IndexedContext;
    use crate::reuse::ContextAccess;
    use crate::{ContextGroup, ContextKind, ContextSite, Echoes, Report, SourceFile, TargetScan};
    use alloc::string::{String, ToString};
    use alloc::vec;
    use alloc::vec::Vec;

    fn scan(code: &str, floor: usize) -> Report {
        let target = TargetScan {
            name: "fixture".into(),
            library: true,
            files: vec![SourceFile {
                path: "src/lib.rs".into(),
                segments: vec![],
                cfg: vec![],
                parsed: syn::parse_file(code).unwrap(),
                text: code.to_string(),
                rustdoc: true,
            }],
        };
        Echoes::default()
            .context_blocks()
            .context_min_tokens(floor)
            .run_targets("", &[target], &|_, _| None)
    }

    fn assert_bodies(code: &str, lines: [u32; 2], names: [&str; 2]) {
        let report = scan(code, 0);
        assert_eq!(report.context_blocks, 2);
        assert_eq!(report.unique_context_blocks, 1);
        assert_eq!(report.context_groups.len(), 1);
        let sites = &report.context_groups[0].sites;
        assert_eq!(sites.len(), 2);
        let pairs = lines.into_iter().zip(names);
        for (site, (line, name)) in sites.iter().zip(pairs) {
            assert_eq!(site.kind, ContextKind::FunctionBody);
            assert_eq!(site.item, name);
            assert_eq!(site.line, line);
            assert_eq!(site.end, line);
            let row = code
                .lines()
                .nth(usize::try_from(line - 1).unwrap())
                .unwrap();
            assert_eq!(row.get(site.column - 1..site.end_column - 1), Some("{}"));
        }
    }

    #[test]
    fn canonical_size_floor_is_inclusive() {
        let code = "fn a(x: u32) -> u32 { x + x } fn b(y: u32) -> u32 { y + y }";
        let boundary = scan(code, 3);
        assert_eq!(boundary.context_blocks, 2);
        assert_eq!(boundary.unique_context_blocks, 1);
        assert_eq!(boundary.context_groups.len(), 1);
        assert_eq!(boundary.context_groups[0].tokens, 3);
        assert_eq!(
            boundary.context_groups[0].sites[0].kind,
            ContextKind::FunctionBody
        );
        let above = scan(code, 4);
        assert_eq!(above.context_blocks, 2);
        assert_eq!(above.unique_context_blocks, 0);
        assert_eq!(above.context_groups, Vec::<ContextGroup>::new());
    }

    #[test]
    fn contained_group_is_suppressed_only_without_additional_sites() {
        let paired = "fn a(x: u32) -> u32 { { x + x } } fn b(y: u32) -> u32 { { y + y } }";
        let covered = scan(paired, 0);
        assert_eq!(covered.context_blocks, 4);
        assert_eq!(covered.context_groups.len(), 1);
        assert!(
            covered.context_groups[0]
                .sites
                .iter()
                .all(|s| s.kind == ContextKind::FunctionBody)
        );
        let extra = scan(
            &alloc::format!("{paired} fn c(z: u32) -> u32 {{ 1 + {{ z + z }} }}"),
            0,
        );
        assert_eq!(extra.context_groups.len(), 2);
        assert!(extra.context_groups.iter().any(|g| g.sites.len() == 3));
    }

    #[test]
    fn empty_contexts_keep_distinct_delimiter_ranges() {
        let report = scan("fn a() {} fn b() {}", 0);
        assert_eq!(report.context_blocks, 2);
        assert_eq!(report.unique_context_blocks, 1);
        assert_eq!(report.context_groups.len(), 1);
        let sites = &report.context_groups[0].sites;
        assert_eq!(sites.len(), 2);
        assert!(sites.iter().all(|site| site.column < site.end_column));
        assert!(sites[0].end_column < sites[1].column);
        let filtered = scan("fn a() {} fn b() {}", 1);
        assert_eq!(filtered.context_blocks, 2);
        assert_eq!(filtered.context_groups, Vec::<ContextGroup>::new());
    }

    #[test]
    fn same_line_sibling_blocks_are_distinct_source_contexts() {
        let report = scan("fn a(x: u32) { consume({ x + x }, { x + x }); }", 0);
        assert_eq!(report.context_blocks, 3);
        let group = report
            .context_groups
            .iter()
            .find(|g| g.sites.len() == 2)
            .unwrap();
        let (a, b) = (&group.sites[0], &group.sites[1]);
        assert_eq!(a.line, b.line);
        assert_ne!(a.column, b.column);
        assert!(a.end_column <= b.column);
    }

    #[test]
    fn shared_source_files_are_counted_once_across_targets() {
        let code = "fn a(x: u32) -> u32 { x + x }";
        let file = SourceFile {
            path: "src/shared.rs".into(),
            segments: vec![],
            cfg: vec![],
            parsed: syn::parse_file(code).unwrap(),
            text: code.into(),
            rustdoc: true,
        };
        let targets = [
            TargetScan {
                name: "one".into(),
                library: true,
                files: vec![file.clone()],
            },
            TargetScan {
                name: "two".into(),
                library: false,
                files: vec![file],
            },
        ];
        let report = Echoes::default()
            .context_blocks()
            .context_min_tokens(0)
            .run_targets("", &targets, &|_, _| None);
        assert_eq!(report.context_blocks, 1);
        assert_eq!(report.context_groups, Vec::<ContextGroup>::new());
    }

    #[test]
    fn renamed_captures_merge_without_merging_distinct_references() {
        let report = scan(
            "fn a(x: u32) { consume({ x + x }); } fn b(unused: u32, y: u32) { let irrelevant = 10; consume({ y + y }); } fn c(x: u32, y: u32) { consume({ x + y }); }",
            0,
        );
        assert!(report.context_groups.iter().any(|g| {
            g.sites
                .iter()
                .map(|s| s.item.as_str())
                .collect::<alloc::vec::Vec<_>>()
                == ["fixture::a", "fixture::b"]
        }));
        assert!(
            report
                .context_groups
                .iter()
                .all(|g| g.sites.iter().all(|s| s.item != "fixture::c"))
        );
    }

    #[test]
    fn unicode_context_columns_slice_original_source_bytes() {
        let code = "fn α(é: u32) -> u32 { é + é }\nfn β(ø: u32) -> u32 { ø + ø }";
        let report = scan(code, 0);
        assert_eq!(report.context_groups.len(), 1);
        let sites = &report.context_groups[0].sites;
        assert_eq!(sites.len(), 2);
        for (site, expected) in sites.iter().zip(["{ é + é }", "{ ø + ø }"]) {
            assert_eq!(site.line, site.end);
            let line = code
                .lines()
                .nth(usize::try_from(site.line - 1).unwrap())
                .unwrap();
            assert_eq!(
                line.get(site.column - 1..site.end_column - 1),
                Some(expected)
            );
        }
    }

    #[test]
    fn bom_prefix_columns_slice_original_source_bytes() {
        assert_bodies(
            "\u{feff}fn é(){}\nfn ø(){}\n",
            [1, 2],
            ["fixture::é", "fixture::ø"],
        );
        assert_bodies(
            "\u{feff}fn a(){}\nfn b(){}\n",
            [1, 2],
            ["fixture::a", "fixture::b"],
        );
    }

    #[test]
    fn shebang_prefix_columns_slice_original_source_bytes() {
        assert_bodies(
            "#!/usr/bin/env rustx\nfn é(){}\nfn ø(){}\n",
            [2, 3],
            ["fixture::é", "fixture::ø"],
        );
        assert_bodies(
            "#!/usr/bin/env rustx\nfn a(){}\nfn b(){}\n",
            [2, 3],
            ["fixture::a", "fixture::b"],
        );
        assert_bodies(
            "#!/tmp/é\nfn a(){}\nfn b(){}\n",
            [2, 3],
            ["fixture::a", "fixture::b"],
        );
    }

    #[test]
    fn bom_and_shebang_prefix_columns_slice_original_source_bytes() {
        assert_bodies(
            "\u{feff}#!/usr/bin/env rustx\nfn é(){}\nfn ø(){}\n",
            [2, 3],
            ["fixture::é", "fixture::ø"],
        );
        assert_bodies(
            "\u{feff}#!/usr/bin/env rustx\nfn a(){}\nfn b(){}\n",
            [2, 3],
            ["fixture::a", "fixture::b"],
        );
    }

    #[test]
    fn each_block_range_is_reported_once() {
        let code = "fn a(x: u32) -> u32 { let y = if x > 1 { x + x } else { x - x }; y }";
        let report = scan(code, 0);
        assert_eq!(report.context_blocks, 3);
        assert_eq!(report.unique_context_blocks, 3);
        assert_eq!(report.context_groups, Vec::<ContextGroup>::new());
    }

    #[test]
    fn contained_group_suppression_keeps_the_outer_sites() {
        let code = "fn a(x: u32) -> u32 { { x + x } } fn b(y: u32) -> u32 { { y + y } }";
        let report = scan(code, 0);
        assert_eq!(report.context_blocks, 4);
        assert_eq!(report.unique_context_blocks, 2);
        assert_eq!(report.context_groups.len(), 1);
        let group = &report.context_groups[0];
        assert_eq!(group.sites.len(), 2);
        assert!(
            group
                .sites
                .iter()
                .map(|s| s.item.as_str())
                .eq(["fixture::a", "fixture::b"])
        );
        for (site, expected) in group.sites.iter().zip(["{ { x + x } }", "{ { y + y } }"]) {
            assert_eq!(site.kind, ContextKind::FunctionBody);
            assert_eq!(&code[site.column - 1..site.end_column - 1], expected);
        }
    }

    #[test]
    fn match_arms_contain_their_body_blocks_to_the_same_end() {
        let code =
            "fn f(a: u8) -> (u8, u8) { (match a { _ => { a + a } }, match a { _ => { a + a } }) }";
        let report = scan(code, 0);
        assert_eq!(report.context_blocks, 5);
        assert_eq!(report.unique_context_blocks, 3);
        assert_eq!(report.context_groups.len(), 1);
        let group = &report.context_groups[0];
        assert_eq!(group.sites.len(), 2);
        let serialized = serde_json::to_value(&report).unwrap();
        let serialized_sites = serialized["context_groups"][0]["sites"].as_array().unwrap();
        assert_eq!(serialized_sites.len(), 2);
        assert!(serialized_sites.iter().all(|site| site["kind"] == "arm"));
        for site in &group.sites {
            assert_eq!(site.kind, ContextKind::Arm);
            assert_eq!(site.item, "fixture::f");
            assert_eq!(site.line, site.end);
            assert_eq!(
                &code[site.column - 1..site.end_column - 1],
                "_ => { a + a }"
            );
        }
    }

    #[test]
    fn repeated_ast_nodes_keep_one_source_context() {
        let code = "fn first(value: u32) -> u32 { value + value }";
        let mut parsed = syn::parse_file(code).unwrap();
        parsed.items.push(parsed.items[0].clone());
        let target = TargetScan {
            name: "fixture".into(),
            library: true,
            files: vec![SourceFile {
                path: "src/lib.rs".into(),
                segments: vec![],
                cfg: vec![],
                parsed,
                text: code.into(),
                rustdoc: true,
            }],
        };
        let report = Echoes::default()
            .context_blocks()
            .context_min_tokens(0)
            .run_targets("", &[target], &|_, _| None);
        assert_eq!(report.context_blocks, 1);
        assert_eq!(report.unique_context_blocks, 1);
        assert_eq!(report.context_groups, Vec::<ContextGroup>::new());
    }

    #[test]
    fn foreign_static_type_blocks_report_the_crate_location() {
        let report = scan(
            "unsafe extern \"C\" {
                 static FIRST: [u8; { 1 + 1 }];
                 static SECOND: [u8; { 1 + 1 }];
             }",
            0,
        );
        assert_eq!(report.context_groups.len(), 1);
        assert_eq!(report.context_groups[0].sites.len(), 2);
        for site in &report.context_groups[0].sites {
            assert_eq!(site.item, "fixture");
            assert_eq!(site.kind, ContextKind::Block);
        }
    }

    fn record(
        form: &str,
        tokens: usize,
        file: &str,
        line: u32,
        column: usize,
        end_column: usize,
    ) -> IndexedContext {
        IndexedContext {
            form: form.to_string(),
            tokens,
            site: ContextSite {
                file: file.to_string(),
                line,
                column,
                end: line,
                end_column,
                item: String::new(),
                kind: ContextKind::FunctionBody,
            },
            module: "app".to_string(),
            crate_name: "app".to_string(),
            crate_root: "src/lib.rs".to_string(),
            access: ContextAccess::Unresolved,
            api_path: None,
            conditions: Vec::new(),
            extraction_notes: Vec::new(),
        }
    }

    fn refs(records: &[IndexedContext]) -> Vec<&IndexedContext> {
        records.iter().collect()
    }

    #[test]
    fn group_requires_a_local_anchor() {
        let local = vec![record("a", 5, "app/src/lib.rs", 1, 1, 9)];
        let corpus = vec![
            record("b", 5, "other/src/lib.rs", 1, 1, 9),
            record("b", 5, "other/src/lib.rs", 9, 1, 9),
        ];
        let (total, unique, groups) = group(&refs(&local), &refs(&corpus), 0, 2);
        assert_eq!(total, 1);
        assert_eq!(unique, 1);
        assert_eq!(groups, Vec::<ContextGroup>::new());
        let anchored = vec![record("b", 5, "app/src/lib.rs", 1, 1, 9)];
        let (_, _, groups) = group(&refs(&anchored), &refs(&corpus), 0, 2);
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].sites.len(), 3);
    }

    #[test]
    fn group_counts_physical_ranges_once() {
        let local = vec![
            record("a", 5, "app/src/lib.rs", 1, 1, 9),
            record("a", 5, "app/src/lib.rs", 1, 1, 9),
        ];
        let (total, _, groups) = group(&refs(&local), &[], 0, 2);
        assert_eq!(total, 1);
        assert_eq!(groups, Vec::<ContextGroup>::new());
        let corpus = vec![record("a", 5, "app/src/lib.rs", 1, 1, 9)];
        let (_, _, groups) = group(&refs(&local), &refs(&corpus), 0, 1);
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].sites.len(), 1);
    }

    #[test]
    fn group_floor_filters_records_not_totals() {
        let local = vec![
            record("big", 10, "app/src/lib.rs", 1, 1, 9),
            record("small", 1, "app/src/lib.rs", 3, 1, 5),
        ];
        let corpus = vec![record("big", 10, "other/src/lib.rs", 1, 1, 9)];
        let (total, unique, groups) = group(&refs(&local), &refs(&corpus), 5, 2);
        assert_eq!(total, 2);
        assert_eq!(unique, 1);
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].tokens, 10);
        assert_eq!(groups[0].sites.len(), 2);
    }

    #[test]
    fn group_suppresses_contained_groups() {
        let local = vec![
            record("outer", 10, "app/src/lib.rs", 1, 1, 50),
            record("outer", 10, "app/src/lib.rs", 9, 1, 50),
        ];
        let corpus = vec![
            record("outer", 10, "other/src/lib.rs", 1, 1, 50),
            record("inner", 5, "app/src/lib.rs", 1, 5, 20),
            record("inner", 5, "app/src/lib.rs", 9, 5, 20),
            record("inner", 5, "other/src/lib.rs", 1, 5, 20),
        ];
        let (_, _, groups) = group(&refs(&local), &refs(&corpus), 0, 2);
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].tokens, 10);
        assert_eq!(groups[0].sites.len(), 3);
    }

    #[test]
    fn group_threshold_counts_local_and_corpus_sites() {
        let local = vec![record("a", 5, "app/src/lib.rs", 1, 1, 9)];
        let corpus = vec![record("a", 5, "other/src/lib.rs", 1, 1, 9)];
        let (_, _, groups) = group(&refs(&local), &refs(&corpus), 0, 2);
        assert_eq!(groups.len(), 1);
        let (_, _, none) = group(&refs(&local), &refs(&corpus), 0, 3);
        assert_eq!(none, Vec::<ContextGroup>::new());
    }

    #[derive(serde::Deserialize)]
    struct KindWrapper {
        #[serde(deserialize_with = "super::deserialize_kind")]
        kind: ContextKind,
    }

    #[test]
    fn deserialize_kind_round_trips_the_wire_names() {
        let parsed: KindWrapper = serde_json::from_str(r#"{"kind":"function-body"}"#).unwrap();
        assert_eq!(parsed.kind, ContextKind::FunctionBody);
        let parsed: KindWrapper = serde_json::from_str(r#"{"kind":"block"}"#).unwrap();
        assert_eq!(parsed.kind, ContextKind::Block);
        let parsed: KindWrapper = serde_json::from_str(r#"{"kind":"arm"}"#).unwrap();
        assert_eq!(parsed.kind, ContextKind::Arm);
        let parsed: KindWrapper = serde_json::from_str(r#"{"kind":"closure"}"#).unwrap();
        assert_eq!(parsed.kind, ContextKind::Closure);
        assert!(serde_json::from_str::<KindWrapper>(r#"{"kind":"weird"}"#).is_err());
    }
}
