//! Opaque comparison, canonical size boundaries, and reference metadata.

use core::sync::atomic::{AtomicUsize, Ordering};
use dejadoc::{DocTest, group};
use syn_canon::{ContextKind, SourceContext};

fn site(item: &str, code: &str) -> DocTest {
    DocTest {
        file: format!("{item}.rs"),
        line: 1,
        end: None,
        item: item.to_owned(),
        info: Vec::new(),
        code: code.to_owned(),
        allow: false,
        self_type: None,
        public: false,
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

fn context_form(file: &syn::File, kind: ContextKind) -> syn_canon::CanonicalForm {
    let mut forms: Vec<(ContextKind, syn_canon::CanonicalForm)> = Vec::new();
    syn_canon::contexts(file, &mut |context, form| forms.push((context.kind, form)));
    forms
        .into_iter()
        .find(|(seen, _)| *seen == kind)
        .map(|(_, form)| form)
        .unwrap()
}

#[test]
fn canonical_sizes_exclude_framing_and_preserve_both_metrics() {
    let a = syn_canon::canonicalize(syn::parse_str("fn a() -> u32 { return ((1u32)); }").unwrap());
    let b = syn_canon::canonicalize(syn::parse_str("fn b() -> u32 { 1u32 }").unwrap());
    assert_eq!(a, b);
    assert_eq!(a.leaf_tokens(), 6);
    assert_eq!(a.body_units(), 1);
    assert_eq!(b.leaf_tokens(), 6);
    assert_eq!(b.body_units(), 1);
}

#[test]
fn canonical_size_filtering_is_independent_of_original_syntax_and_site_order() {
    let a = site("expanded", "fn a() -> u32 { return ((1u32)); }");
    let b = site("compact", "fn b() -> u32 { 1u32 }");
    for sites in [[a.clone(), b.clone()], [b.clone(), a.clone()]] {
        let included = group(&sites, 2, 8);
        let mut members: Vec<_> = included
            .groups
            .iter()
            .flat_map(|group| {
                assert_eq!(group.tokens, 8);
                group.sites.iter().map(|site| site.item.as_str())
            })
            .collect();
        members.sort_unstable();
        assert_eq!(members, ["compact", "expanded"]);
        assert_eq!(group(&sites, 2, 9).groups, []);
    }
}

#[test]
fn a_text_fallback_cannot_impersonate_a_parsed_comparison_key() {
    let source = "fn a() -> u32 { 1u32 }";
    let form = syn_canon::canonicalize(syn::parse_str(source).unwrap());
    let report = group(&[site("parsed", source), site("forged", form.key())], 1, 0);
    let mut members: Vec<_> = report
        .groups
        .iter()
        .map(|group| {
            group
                .sites
                .iter()
                .map(|site| site.item.as_str())
                .collect::<Vec<_>>()
        })
        .collect();
    members.sort_unstable();
    assert_eq!(members, vec![vec!["forged"], vec!["parsed"]]);
}

#[test]
fn opaque_macro_punctuation_spacing_remains_observable() {
    let joint = syn::parse_str("fn f() { observer!(a >> b); }").unwrap();
    let separate = syn::parse_str("fn f() { observer!(a > > b); }").unwrap();
    assert_ne!(
        syn_canon::canonicalize(joint),
        syn_canon::canonicalize(separate)
    );
    let joint = syn::parse_str("fn f() { observer!(a >> b); }").unwrap();
    let separate = syn::parse_str("fn f() { observer!(a > > b); }").unwrap();
    assert_ne!(
        syn_canon::canonicalize_failing(joint),
        syn_canon::canonicalize_failing(separate)
    );
}

#[test]
fn three_char_operators_count_as_one_body_unit() {
    let shift = syn_canon::canonicalize(syn::parse_str("fn f(mut x: i32) { x <<= 1; }").unwrap());
    let assign = syn_canon::canonicalize(syn::parse_str("fn f(mut x: i32) { x = 1; }").unwrap());
    let range = syn_canon::canonicalize(syn::parse_str("fn f() { let _ = 0..=3; }").unwrap());
    assert_eq!(shift.body_units(), 3);
    assert_eq!(shift.body_units(), assign.body_units());
    assert_eq!(range.body_units(), 7);
}

#[test]
fn opaque_macro_punct_runs_keep_their_body_units() {
    let triple = syn_canon::canonicalize(syn::parse_str("fn f() { m!(x === y); }").unwrap());
    let double = syn_canon::canonicalize(syn::parse_str("fn f() { m!(x == y); }").unwrap());
    let lean = syn_canon::canonicalize(syn::parse_str("fn f() { m!(x <== y); }").unwrap());
    let short = syn_canon::canonicalize(syn::parse_str("fn f() { m!(x <= y); }").unwrap());
    assert_eq!(triple.body_units(), 6);
    assert_ne!(triple.body_units(), double.body_units());
    assert_eq!(lean.body_units(), 6);
    assert_ne!(lean.body_units(), short.body_units());
}

#[test]
fn body_unit_boundaries_count_lifetimes_paths_and_groups_as_one() {
    let lifetime =
        syn_canon::canonicalize(syn::parse_str("fn f() { let x: &'static i32 = &1; x; }").unwrap());
    let path = syn_canon::canonicalize(
        syn::parse_str("fn f() { let _ = std::f32::consts::PI; }").unwrap(),
    );
    let group = syn_canon::canonicalize(syn::parse_str("fn f() { let _ = [1, 2]; }").unwrap());
    assert_eq!(lifetime.body_units(), 11);
    assert_eq!(path.body_units(), 5);
    assert_eq!(group.body_units(), 6);
}

#[test]
fn function_body_contexts_keep_macro_input_resolution() {
    let plain: syn::File = syn::parse_str("const a: i32 = 1; fn f() { let _ = m!(a); }").unwrap();
    let shadowed: syn::File = syn::parse_str("struct a; fn f() { let _ = m!(a); }").unwrap();
    assert_ne!(
        context_form(&plain, ContextKind::FunctionBody),
        context_form(&shadowed, ContextKind::FunctionBody),
    );
}

#[test]
fn closure_contexts_keep_macro_input_resolution() {
    let plain: syn::File =
        syn::parse_str("const a: i32 = 1; fn f() { let g = || m!(a); }").unwrap();
    let shadowed: syn::File = syn::parse_str("struct a; fn f() { let g = || m!(a); }").unwrap();
    assert_ne!(
        context_form(&plain, ContextKind::Closure),
        context_form(&shadowed, ContextKind::Closure),
    );
}

#[test]
fn arm_contexts_keep_macro_input_resolution() {
    let plain: syn::File =
        syn::parse_str("const a: i32 = 1; fn f(v: i32) { match v { _ => m!(a) } }").unwrap();
    let shadowed: syn::File =
        syn::parse_str("struct a; fn f(v: i32) { match v { _ => m!(a) } }").unwrap();
    assert_ne!(
        context_form(&plain, ContextKind::Arm),
        context_form(&shadowed, ContextKind::Arm),
    );
}

#[test]
fn primitive_parameter_views_stay_distinct_from_struct_shadows() {
    for prim in [
        "i8", "i16", "i32", "i64", "i128", "u8", "u16", "u64", "u128", "bool",
    ] {
        let plain: syn::File = syn::parse_str(&format!("fn f(x: {prim}) {{ x; }}")).unwrap();
        let shadowed: syn::File =
            syn::parse_str(&format!("struct {prim}; fn f(x: {prim}) {{ x; }}")).unwrap();
        let plain_context = SourceContext::new(core::iter::once((&[][..], &plain)));
        let shadowed_context = SourceContext::new(core::iter::once((&[][..], &shadowed)));
        let plain = function(&plain, "f");
        let shadowed = function(&shadowed, "f");
        assert_ne!(
            plain_context
                .function(&plain.sig, &plain.block)
                .unwrap()
                .canonicalize(),
            shadowed_context
                .function(&shadowed.sig, &shadowed.block)
                .unwrap()
                .canonicalize(),
            "primitive {prim} must keep its proven identity against a struct shadow"
        );
    }
}

#[test]
fn fixed_width_scalar_parameters_survive_canonicalize() {
    let low = syn_canon::canonicalize(syn::parse_str("fn f(x: u16) { x; }").unwrap());
    let high = syn_canon::canonicalize(syn::parse_str("fn f(x: u128) { x; }").unwrap());
    assert_ne!(low, high);
}

static HASH_EQ_CALLS: AtomicUsize = AtomicUsize::new(0);

#[derive(Debug)]
struct Keyed(syn_canon::CanonicalForm);

impl core::hash::Hash for Keyed {
    fn hash<H: core::hash::Hasher>(&self, state: &mut H) {
        self.0.hash(state);
    }
}

impl core::cmp::PartialEq for Keyed {
    fn eq(&self, other: &Self) -> bool {
        HASH_EQ_CALLS.fetch_add(1, Ordering::Relaxed);
        self.0 == other.0
    }
}

impl core::cmp::Eq for Keyed {}

#[test]
fn canonical_form_hash_spreads_distinct_keys_through_collection_lookups() {
    const FORMS: usize = 64;
    let sources: Vec<String> = (0..FORMS)
        .map(|index| format!("fn f() -> u32 {{ {index}u32 }}"))
        .collect();
    let forms: Vec<syn_canon::CanonicalForm> = sources
        .iter()
        .map(|source| syn_canon::canonicalize(syn::parse_str(source).unwrap()))
        .collect();
    let mut map =
        std::collections::HashMap::<_, _, _>::with_hasher(std::hash::BuildHasherDefault::<
            std::collections::hash_map::DefaultHasher,
        >::default());
    for (index, form) in forms.into_iter().enumerate() {
        map.insert(Keyed(form), index);
    }
    let baseline = HASH_EQ_CALLS.load(Ordering::Relaxed);
    let mut hits = 0;
    for (index, source) in sources.iter().enumerate() {
        let probe = Keyed(syn_canon::canonicalize(syn::parse_str(source).unwrap()));
        if map.get(&probe) == Some(&index) {
            hits += 1;
        }
    }
    assert_eq!(hits, FORMS);
    let calls = HASH_EQ_CALLS.load(Ordering::Relaxed) - baseline;
    assert!(
        calls < 8 * FORMS,
        "a key-blind hash collapses every lookup into a linear scan"
    );
}
