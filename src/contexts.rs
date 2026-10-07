//! Qualifying lexical context forms and their source locations.

use crate::{ContextGroup, ContextKind, ContextSite, SourceFile};
use alloc::collections::{BTreeMap, BTreeSet};
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

type Position = (usize, usize);
type Range = (Position, Position);

pub(crate) struct Forms<'a> {
    floor: usize,
    seen: BTreeMap<&'a str, BTreeSet<Range>>,
    by_form: BTreeMap<String, (usize, Vec<ContextSite>)>,
}

impl<'a> Forms<'a> {
    pub(crate) fn new(floor: usize) -> Self {
        Self {
            floor,
            seen: BTreeMap::new(),
            by_form: BTreeMap::new(),
        }
    }

    pub(crate) fn file(&mut self, prefix: &str, file: &'a SourceFile, root: &str) {
        let path = file
            .path
            .strip_prefix(root)
            .and_then(|p| p.strip_prefix('/'))
            .unwrap_or(&file.path);
        let seen = match self.seen.entry(path) {
            alloc::collections::btree_map::Entry::Occupied(_) => return,
            alloc::collections::btree_map::Entry::Vacant(entry) => entry.insert(BTreeSet::new()),
        };
        let ascii = file.text.is_ascii();
        syn_canon::contexts(&file.parsed, &mut |context, canonical| {
            let (start, end) = (context.span.start(), context.span.end());
            let range = ((start.line, start.column), (end.line, end.column));
            if !seen.insert(range) {
                return;
            }
            let tokens = crate::normalize::units(canonical.clone());
            if tokens < self.floor {
                return;
            }
            let (column, end_column) = if ascii {
                (start.column + 1, end.column + 1)
            } else {
                let bytes = context.span.byte_range();
                (
                    byte_column(&file.text, bytes.start),
                    byte_column(&file.text, bytes.end),
                )
            };
            let form = canonical.to_string();
            let site = ContextSite {
                file: path.to_string(),
                line: u32::try_from(start.line).unwrap_or(u32::MAX),
                column,
                end: u32::try_from(end.line).unwrap_or(u32::MAX),
                end_column,
                item: if context.item.is_empty() {
                    prefix.to_string()
                } else {
                    format!("{prefix}::{}", context.item)
                },
                kind: context.kind,
            };
            self.by_form
                .entry(form)
                .or_insert_with(|| (tokens, Vec::new()))
                .1
                .push(site);
        });
    }

    pub(crate) fn finish(self, threshold: usize) -> (usize, usize, Vec<ContextGroup>) {
        let total = self.seen.values().map(BTreeSet::len).sum();
        let unique = self.by_form.len();
        let mut groups: Vec<_> = self
            .by_form
            .into_iter()
            .filter(|(_, (_, sites))| sites.len() >= threshold)
            .map(|(form, (tokens, mut sites))| {
                sites.sort_by(|a, b| {
                    (&a.file, a.line, a.column, a.end, a.end_column).cmp(&(
                        &b.file,
                        b.line,
                        b.column,
                        b.end,
                        b.end_column,
                    ))
                });
                let hash = blake3::hash(form.as_bytes()).to_hex().to_string();
                ContextGroup {
                    id: hash[..8].to_string(),
                    hash,
                    tokens,
                    sites,
                }
            })
            .collect();
        let covered: Vec<_> = groups
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
}

fn byte_column(text: &str, offset: usize) -> usize {
    offset - text[..offset].rfind('\n').map_or(0, |newline| newline + 1) + 1
}

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
    use crate::{ContextKind, Dejadoc, Report, SourceFile, TargetScan};
    use alloc::string::ToString;
    use alloc::vec;

    fn scan(code: &str, floor: usize) -> Report {
        let target = TargetScan {
            name: "fixture".into(),
            library: true,
            files: vec![SourceFile {
                path: "src/lib.rs".into(),
                segments: vec![],
                parsed: syn::parse_file(code).unwrap(),
                text: code.to_string(),
                rustdoc: true,
            }],
        };
        Dejadoc::default()
            .context_blocks()
            .context_min_tokens(floor)
            .run_targets("", &[target], &|_, _| None)
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
        assert_eq!(above.context_groups.len(), 0);
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
        assert_eq!(filtered.context_groups.len(), 0);
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
        let report = Dejadoc::default()
            .context_blocks()
            .context_min_tokens(0)
            .run_targets("", &targets, &|_, _| None);
        assert_eq!(report.context_blocks, 1);
        assert_eq!(report.context_groups.len(), 0);
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
    fn each_block_range_is_reported_once() {
        let code = "fn a(x: u32) -> u32 { let y = if x > 1 { x + x } else { x - x }; y }";
        let report = scan(code, 0);
        assert_eq!(report.context_blocks, 3);
        assert_eq!(report.unique_context_blocks, 3);
        assert_eq!(report.context_groups.len(), 0);
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
                parsed,
                text: code.into(),
                rustdoc: true,
            }],
        };
        let report = Dejadoc::default()
            .context_blocks()
            .context_min_tokens(0)
            .run_targets("", &[target], &|_, _| None);
        assert_eq!(report.context_blocks, 1);
        assert_eq!(report.unique_context_blocks, 1);
        assert_eq!(report.context_groups.len(), 0);
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
}
