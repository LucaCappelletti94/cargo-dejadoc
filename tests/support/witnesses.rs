/// One independently executed primitive source pair.
#[derive(serde::Deserialize)]
pub struct Case {
    pub id: String,
    pub a: String,
    pub b: String,
    pub expected: String,
}

/// The same and separate cases canonicalize as expected, with distinct failing forms.
pub(super) fn relations(cases: &[Case]) {
    for case in cases {
        let parse = |source: &str| syn::parse_str::<syn::File>(source).unwrap();
        let left = syn_canon::canonicalize(parse(&case.a));
        let right = syn_canon::canonicalize(parse(&case.b));
        if case.expected == "same" {
            assert_eq!(left, right, "{}", case.id);
            assert_eq!(left.leaf_tokens(), right.leaf_tokens(), "{}", case.id);
            assert_eq!(left.body_units(), right.body_units(), "{}", case.id);
        } else {
            assert_ne!(left, right, "{}", case.id);
        }
        assert_ne!(
            syn_canon::canonicalize_failing(parse(&case.a)),
            syn_canon::canonicalize_failing(parse(&case.b)),
            "{}",
            case.id
        );
    }
}
