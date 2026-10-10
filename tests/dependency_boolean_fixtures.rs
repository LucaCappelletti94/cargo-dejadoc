//! Original boolean witnesses through public comparison.

#[path = "support/dependency.rs"]
mod support;

fn equal_fixture(id: &str) {
    let case = support::cases()
        .iter()
        .find(|case| case.id == id && case.family == "boolean" && case.expected == "same")
        .unwrap();
    let form = |source: &str| syn_canon::canonicalize(syn::parse_str(source).unwrap());
    let failing = |source: &str| syn_canon::canonicalize_failing(syn::parse_str(source).unwrap());
    let left = form(&case.a);
    let right = form(&case.b);
    assert_eq!(left, right, "{id}");
    assert_eq!(left.leaf_tokens(), right.leaf_tokens());
    assert_eq!(left.body_units(), right.body_units());
    assert_ne!(failing(&case.a), failing(&case.b));
}

#[test]
fn boolean_double_negation() {
    equal_fixture("A04_double_not");
}

#[test]
fn boolean_value_branches() {
    equal_fixture("A05_boolean_tail");
}

#[test]
fn boolean_demorgan() {
    equal_fixture("A06_demorgan");
}

#[test]
fn boolean_compare_true() {
    equal_fixture("A11_compare_true");
}

#[test]
fn boolean_branch_inversion() {
    equal_fixture("A16_branch_inversion");
}

#[test]
fn boolean_literal_condition() {
    equal_fixture("A19_literal_condition");
}

#[test]
fn boolean_inherited_negative_fixtures() {
    for case in support::cases()
        .iter()
        .filter(|case| case.expected != "same")
    {
        let parsed = |source: &str| syn::parse_str(source).unwrap();
        assert_ne!(
            syn_canon::canonicalize(parsed(&case.a)),
            syn_canon::canonicalize(parsed(&case.b)),
            "{}",
            case.id
        );
        assert_ne!(
            syn_canon::canonicalize_failing(parsed(&case.a)),
            syn_canon::canonicalize_failing(parsed(&case.b)),
            "{}",
            case.id
        );
    }
}

#[test]
fn boolean_adjacent_proof_rejections() {
    for (left, right) in [
        ("fn f(a: u8) { !!a }", "fn f(a: u8) { a }"),
        ("fn f(a: Custom) { !!a }", "fn f(a: Custom) { a }"),
        (
            "fn f() { if read(&D) { true } else { false } }",
            "fn f() { read(&D) }",
        ),
        (
            "fn f() { !(left() && right()) }",
            "fn f() { !left() || !right() }",
        ),
        ("fn f(a: Custom) { a == true }", "fn f(a: Custom) { a }"),
        (
            "fn f(a: Custom,b: Custom) { (a == b) == true }",
            "fn f(a: Custom,b: Custom) { a == b }",
        ),
        (
            "fn f() { if !read(&D) { left() } else { right() } }",
            "fn f() { if read(&D) { right() } else { left() } }",
        ),
        ("fn f() { if true { 0 } else { 0u8 } }", "fn f() { 0 }"),
        (
            "fn f() { if true { &[] } else { &[1u8] } }",
            "fn f() { &[] }",
        ),
        ("fn f(a: bool) { &!!a }", "fn f(a: bool) { &a }"),
    ] {
        assert_ne!(
            syn_canon::canonicalize(syn::parse_str(left).unwrap()),
            syn_canon::canonicalize(syn::parse_str(right).unwrap())
        );
        assert_ne!(
            syn_canon::canonicalize_failing(syn::parse_str(left).unwrap()),
            syn_canon::canonicalize_failing(syn::parse_str(right).unwrap())
        );
    }
}
