//! Boolean grouping membership, canonical sizes and context views.

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

const DOMAINS: [(&str, &str); 6] = [
    (
        "fn f(p: bool) -> bool { !!p }",
        "fn g(x: bool) -> bool { x }",
    ),
    (
        "fn f(p: bool) -> bool { if p { true } else { false } }",
        "fn g(x: bool) -> bool { x }",
    ),
    (
        "fn f(a: bool, b: bool) -> bool { !(a && b) }",
        "fn g(x: bool, y: bool) -> bool { !x || !y }",
    ),
    (
        "fn f(p: bool) -> bool { p == true }",
        "fn g(x: bool) -> bool { x }",
    ),
    (
        "fn f(p: bool) -> u32 { if !p { 1u32 } else { 2u32 } }",
        "fn g(x: bool) -> u32 { if x { 2u32 } else { 1u32 } }",
    ),
    (
        "fn f() -> u32 { if true { 5u32 } else { 6u32 } }",
        "fn g() -> u32 { 5u32 }",
    ),
];

#[test]
fn boolean_grouping_keeps_exact_passing_members_and_excludes_failing_variants() {
    for (left, right) in DOMAINS {
        let sites = [
            site("pass_a", left, &[]),
            site("pass_b", right, &[]),
            site("fail_a", left, &["compile_fail"]),
            site("fail_b", right, &["compile_fail"]),
        ];
        let report = group(&sites, 2, 0);
        assert_eq!(
            members(&report),
            [vec!["pass_a", "pass_b"]],
            "{left}\n{right}"
        );
        assert!(report.groups[0].sites.iter().all(|member| {
            member.line == 7 && member.end == Some(11) && member.info.is_empty()
        }));
    }
}

#[test]
fn boolean_inherited_negatives_keep_four_singletons() {
    for case in support::cases().iter().filter(|case| {
        case.expected != "same"
            && case.family == "negative"
            && [
                "N12_short_circuit",
                "N14_temporary_drop_scope",
                "N15_discarded_branch_inference",
            ]
            .contains(&case.id.as_str())
    }) {
        let sites = [
            site("pass_a", &case.a, &[]),
            site("pass_b", &case.b, &[]),
            site("fail_a", &case.a, &["compile_fail"]),
            site("fail_b", &case.b, &["compile_fail"]),
        ];
        let report = group(&sites, 1, 0);
        assert_eq!(
            members(&report),
            [
                vec!["fail_a"],
                vec!["fail_b"],
                vec!["pass_a"],
                vec!["pass_b"]
            ],
            "{}",
            case.id
        );
    }
}

#[test]
fn boolean_later_batch_controls_stay_separate_in_groups() {
    for (left, right) in [
        (
            "fn f() -> u32 { if true { mark(); 1u32 } else { 2u32 } }",
            "fn g() -> u32 { mark(); 1u32 }",
        ),
        (
            "fn f() -> u32 { if 1u32 == 2u32 { 3u32 } else { 4u32 } }",
            "fn g() -> u32 { 4u32 }",
        ),
    ] {
        let sites = [site("first", left, &[]), site("second", right, &[])];
        let report = group(&sites, 1, 0);
        assert_eq!(
            members(&report),
            [vec!["first"], vec!["second"]],
            "{left}\n{right}"
        );
    }
}

#[test]
fn boolean_threshold_uses_one_canonical_size_in_both_site_orders() {
    for (left, right) in [
        (
            "fn f(p: bool) -> bool { if p { true } else { false } }",
            "fn g(x: bool) -> bool { x }",
        ),
        (
            "fn f(p: bool) -> bool { !!p }",
            "fn g(x: bool) -> bool { x }",
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
        let reversed = [site("second", right, &[]), site("first", left, &[])];
        let included = group(&sites, 2, a.leaf_tokens());
        let reversed_report = group(&reversed, 2, a.leaf_tokens());
        assert_eq!(members(&included), [vec!["first", "second"]]);
        assert_eq!(members(&reversed_report), members(&included));
        assert_eq!(included.groups[0].id, reversed_report.groups[0].id);
        assert_eq!(included.groups[0].tokens, a.leaf_tokens());
        assert_eq!(group(&sites, 2, a.leaf_tokens() + 1).groups.len(), 0);
    }
}

fn function<'a>(file: &'a syn::File, name: &str) -> &'a syn::ItemFn {
    file.items
        .iter()
        .find_map(|item| match item {
            syn::Item::Fn(function) if function.sig.ident == name => Some(function),
            _ => None,
        })
        .unwrap()
}

fn impl_method<'a>(source: &'a syn::File, name: &str) -> &'a syn::ImplItemFn {
    source
        .items
        .iter()
        .find_map(|item| match item {
            syn::Item::Impl(item) => item.items.iter().find_map(|member| match member {
                syn::ImplItem::Fn(function) if function.sig.ident == name => Some(function),
                _ => None,
            }),
            _ => None,
        })
        .unwrap()
}

#[test]
fn boolean_function_views_preserve_contextual_relations() {
    let source: syn::File = syn::parse_str(
        "
        fn first(input: bool) -> bool { if input { true } else { false } }
        fn same(value: bool) -> bool { value }
        fn held(gate: bool) -> bool { !gate }
        struct Owner;
        impl Owner {
            fn inverted(p: bool) -> u32 { if !p { 1u32 } else { 2u32 } }
            fn flipped(x: bool) -> u32 { if x { 2u32 } else { 1u32 } }
            fn kept(p: bool) -> u32 { if !p { 1u32 } else { 3u32 } }
        }
        ",
    )
    .unwrap();
    let context = syn_canon::SourceContext::new(core::iter::once((&[][..], &source)));
    let canonical = |name: &str| {
        let function = function(&source, name);
        context
            .function(&function.sig, &function.block)
            .unwrap()
            .canonicalize()
    };
    assert_eq!(canonical("first"), canonical("same"));
    assert_ne!(canonical("first"), canonical("held"));
    let method = |name: &str| {
        let member = impl_method(&source, name);
        context
            .function(&member.sig, &member.block)
            .unwrap()
            .canonicalize()
    };
    assert_eq!(method("inverted"), method("flipped"));
    assert_ne!(method("inverted"), method("kept"));
}

struct SpanWiper;

impl syn::visit_mut::VisitMut for SpanWiper {
    fn visit_ident_mut(&mut self, ident: &mut syn::Ident) {
        ident.set_span(proc_macro2::Span::call_site());
    }

    fn visit_lit_mut(&mut self, lit: &mut syn::Lit) {
        lit.set_span(proc_macro2::Span::call_site());
    }
}

fn wiped_form(source: &str, compiles: bool) -> syn_canon::CanonicalForm {
    let mut file = syn::parse_str::<syn::File>(source).unwrap();
    syn::visit_mut::visit_file_mut(&mut SpanWiper, &mut file);
    if compiles {
        syn_canon::canonicalize(file)
    } else {
        syn_canon::canonicalize_failing(file)
    }
}

fn form(source: &str) -> syn_canon::CanonicalForm {
    syn_canon::canonicalize(syn::parse_str(source).unwrap())
}

#[test]
fn constructed_equal_spans_preserve_boolean_relations() {
    let plain = "fn f(p: bool) -> bool { !!p }";
    let folded = "fn g(x: bool) -> bool { x }";
    let held_a = "fn f(p: bool) -> bool { !p }";
    let held_b = "fn g(x: bool) -> bool { x }";
    assert_eq!(wiped_form(plain, true), form(plain));
    assert_eq!(wiped_form(plain, true), wiped_form(folded, true));
    assert_ne!(wiped_form(plain, false), wiped_form(folded, false));
    assert_eq!(wiped_form(held_a, true), form(held_a));
    assert_ne!(wiped_form(held_a, true), wiped_form(held_b, true));
}

fn negated(depth: usize) -> String {
    let mut expr = "a && b".to_owned();
    for _ in 0..depth {
        expr = format!("!({expr})");
    }
    format!("fn f(a: bool, b: bool) -> bool {{ {expr} }}")
}

#[test]
fn bounded_boolean_fallback_remains_atomic_and_deterministic() {
    let under = negated(48);
    let plain = "fn g(a: bool, b: bool) -> bool { a && b }";
    let a = syn_canon::canonicalize(syn::parse_str(&under).unwrap());
    let b = syn_canon::canonicalize(syn::parse_str(plain).unwrap());
    assert_eq!(a, b, "{under}\n{plain}");
    assert_eq!(a.body_units(), b.body_units());
    assert_eq!(a.leaf_tokens(), b.leaf_tokens());
    let over = negated(96);
    let over_first = syn_canon::canonicalize(syn::parse_str(&over).unwrap());
    let over_second = syn_canon::canonicalize(syn::parse_str(&over).unwrap());
    assert_ne!(over_first, b, "{over}\n{plain}");
    assert_ne!(
        syn_canon::canonicalize_failing(syn::parse_str(&over).unwrap()),
        syn_canon::canonicalize_failing(syn::parse_str(plain).unwrap())
    );
    assert_eq!(
        over_first, over_second,
        "the over-bound form is deterministic"
    );
    assert_eq!(over_first.leaf_tokens(), over_second.leaf_tokens());
    assert_eq!(over_first.body_units(), over_second.body_units());
    assert!(
        over_first.body_units() >= 96,
        "the whole over-bound region is retained"
    );
}

fn target(text: &str) -> TargetScan {
    TargetScan {
        name: "boolean".to_owned(),
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
fn boolean_cli_fixture_preserves_exact_function_and_doctest_members() {
    let text = include_str!("fixtures/booleanws/src/lib.rs");
    let target = target(text);
    let doctest = syn_canon::canonicalize(
        syn::parse_str(
            "fn main() { let input = true; let value = if input { true } else { false }; assert_eq!(value, true); }",
        )
        .unwrap(),
    );
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
                vec!["boolean::Owner::first", "boolean::Owner::same"],
                vec!["boolean::left::first", "boolean::left::same"],
                vec!["boolean::right::first", "boolean::right::same"],
            ]
        );
        assert!(function_groups.iter().all(|group| group.tokens >= 30));
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
        assert!(sites.iter().all(|site| site.item == "boolean"
            && site.info.is_empty()
            && site.file == "src/lib.rs"));
        assert_ne!(sites[0].line, sites[1].line);
        assert_eq!(doctest_groups[0].tokens, doctest.leaf_tokens());
        let grouped: Vec<&str> = report
            .groups
            .iter()
            .flat_map(|group| group.sites.iter())
            .map(|site| site.item.as_str())
            .collect();
        assert!(!grouped.iter().any(|item| item.contains("different")));
        assert_eq!(grouped.iter().filter(|item| **item == "boolean").count(), 2);
    }
}

fn logical_tree(out: &mut String, leaves: usize, op: &str, leaf: &str) {
    if leaves == 1 {
        out.push_str(leaf);
    } else {
        out.push('(');
        logical_tree(out, leaves / 2, op, leaf);
        out.push_str(op);
        logical_tree(out, leaves - leaves / 2, op, leaf);
        out.push(')');
    }
}

#[test]
fn boolean_expansion_node_bound_retains_the_complete_region() {
    for leaves in [5_461, 5_462] {
        let mut conjunction = String::new();
        logical_tree(&mut conjunction, leaves, "&&", "a");
        let mut disjunction = String::new();
        logical_tree(&mut disjunction, leaves, "||", "!a");
        let source = format!("fn f(a:bool,b:bool)->(bool,bool){{(!!b,!({conjunction}))}}");
        let normalized = format!("fn f(a:bool,b:bool)->(bool,bool){{(b,{disjunction})}}");
        let original = form(&source);
        if leaves == 5_461 {
            assert_eq!(original, form(&normalized));
        } else {
            assert_ne!(original, form(&normalized));
            let partial = format!("fn f(a:bool,b:bool)->(bool,bool){{(b,!({conjunction}))}}");
            assert_ne!(original, form(&partial));
            assert_eq!(original, form(&source));
        }
    }
}
