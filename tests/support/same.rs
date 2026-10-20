use syn_canon::CanonicalForm;

/// The canonical view of one parsed source.
fn form(source: &str) -> CanonicalForm {
    syn_canon::canonicalize(syn::parse_str(source).unwrap())
}

/// The canonical view of one parsed source, with opaque macro bodies.
fn failing(source: &str) -> CanonicalForm {
    syn_canon::canonicalize_failing(syn::parse_str(source).unwrap())
}

/// The two sources canonicalize identically, keep equal sizes, and differ as failing forms.
pub(super) fn same(left: &str, right: &str) {
    let a = form(left);
    let b = form(right);
    assert_eq!(a, b, "{left}\n{right}");
    assert_eq!(a.body_units(), b.body_units());
    assert_eq!(a.leaf_tokens(), b.leaf_tokens());
    assert_ne!(failing(left), failing(right));
}
