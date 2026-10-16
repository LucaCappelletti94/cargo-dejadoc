//! Primitive value folds through the public comparison contract.

#[path = "support/dependency.rs"]
mod support;

fn original(id: &str) {
    let case = support::cases().iter().find(|case| case.id == id).unwrap();
    assert_eq!(case.family, "temporary");
    assert_eq!(case.expected, "same");
    let left = syn_canon::canonicalize(syn::parse_str(&case.a).unwrap());
    let right = syn_canon::canonicalize(syn::parse_str(&case.b).unwrap());
    assert_eq!(left, right, "{id}");
    assert_eq!(left.leaf_tokens(), right.leaf_tokens());
    assert_eq!(left.body_units(), right.body_units());
    assert_ne!(
        syn_canon::canonicalize_failing(syn::parse_str(&case.a).unwrap()),
        syn_canon::canonicalize_failing(syn::parse_str(&case.b).unwrap()),
        "{id}"
    );
}

#[test]
fn value_scalar_alias() {
    original("A07_copy_alias");
}

#[test]
fn value_scalar_temporary() {
    original("A08_inline_temporary");
}

#[test]
fn value_primitive_tuple() {
    original("A10_tuple_destructure");
}

#[test]
fn value_unused_scalar() {
    original("A18_dead_primitive");
}
