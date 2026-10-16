//! Public relations for independently executed boolean normalization witnesses.

#[derive(serde::Deserialize)]
struct Case {
    id: String,
    a: String,
    b: String,
    expected: String,
}

#[test]
fn native_boolean_sources_preserve_their_public_relations() {
    let mut cases: Vec<Case> =
        serde_json::from_str(include_str!("fixtures/dependency/boolean_cases.json")).unwrap();
    cases.extend(
        serde_json::from_str::<Vec<Case>>(include_str!(
            "fixtures/dependency/boolean_scope_cases.json"
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

#[test]
fn boolean_guards_keep_the_non_admitted_inversions_separate() {
    let parse = |source: &str| syn::parse_str::<syn::File>(source).unwrap();
    let separate = |id: &str, left: &str, right: &str| {
        assert_ne!(
            syn_canon::canonicalize(parse(left)),
            syn_canon::canonicalize(parse(right)),
            "{id}"
        );
        assert_ne!(
            syn_canon::canonicalize_failing(parse(left)),
            syn_canon::canonicalize_failing(parse(right)),
            "{id}"
        );
    };
    let parity = "fn f(c: bool) -> u8 { if !!c { 1u8 } else { 2u8 } }";
    let parity_kept = "fn f(c: bool) -> u8 { if c { 1u8 } else { 2u8 } }";
    let parity_swapped = "fn f(c: bool) -> u8 { if c { 2u8 } else { 1u8 } }";
    assert_eq!(
        syn_canon::canonicalize(parse(parity)),
        syn_canon::canonicalize(parse(parity_kept)),
        "even negation parity keeps the branch order"
    );
    separate(
        "even negation parity never swaps the branches",
        parity,
        parity_swapped,
    );
    separate(
        "a call negation blocks the inversion",
        "fn make() -> bool { true } fn f() -> u8 { if !make() { 1u8 } else { 2u8 } }",
        "fn make() -> bool { true } fn f() -> u8 { if make() { 2u8 } else { 1u8 } }",
    );
    separate(
        "an overloaded negation blocks the inversion",
        "#[derive(Clone, Copy)] struct Flag { v: bool } impl core::ops::Not for Flag { type Output = bool; fn not(self) -> bool { !self.v } } fn f(x: Flag) -> u8 { if !x { 1u8 } else { 2u8 } }",
        "#[derive(Clone, Copy)] struct Flag { v: bool } impl core::ops::Not for Flag { type Output = bool; fn not(self) -> bool { !self.v } } fn f(x: Flag) -> u8 { if x.v { 2u8 } else { 1u8 } }",
    );
    separate(
        "an active attribute blocks the inversion",
        "#[obs] fn f(c: bool) -> u8 { if !c { 1u8 } else { 2u8 } }",
        "#[obs] fn f(c: bool) -> u8 { if c { 2u8 } else { 1u8 } }",
    );
}
