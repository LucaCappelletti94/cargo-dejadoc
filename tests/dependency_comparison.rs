//! Opaque comparison and canonical size boundaries.

use dejadoc::{DocTest, group};

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
