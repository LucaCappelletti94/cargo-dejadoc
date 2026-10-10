//! Public relations for independently executed primitive algebra witnesses.

#[derive(serde::Deserialize)]
struct Case {
    id: String,
    a: String,
    b: String,
    expected: String,
}

#[test]
fn native_algebra_sources_preserve_their_public_relations() {
    let mut cases: Vec<Case> =
        serde_json::from_str(include_str!("fixtures/dependency/algebra_cases.json")).unwrap();
    cases.extend(
        serde_json::from_str::<Vec<Case>>(include_str!(
            "fixtures/dependency/algebra_signed_cases.json"
        ))
        .unwrap(),
    );
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
