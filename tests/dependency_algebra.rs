//! Primitive algebra through passing and failing public comparison.

#[path = "support/dependency.rs"]
mod support;

fn form(source: &str) -> syn_canon::CanonicalForm {
    syn_canon::canonicalize(syn::parse_str(source).unwrap())
}

fn failing(source: &str) -> syn_canon::CanonicalForm {
    syn_canon::canonicalize_failing(syn::parse_str(source).unwrap())
}

fn same(left: &str, right: &str) {
    let a = form(left);
    let b = form(right);
    assert_eq!(a, b, "{left}\n{right}");
    assert_eq!(a.body_units(), b.body_units());
    assert_eq!(a.leaf_tokens(), b.leaf_tokens());
    assert_ne!(failing(left), failing(right));
}

fn separate(left: &str, right: &str) {
    assert_ne!(form(left), form(right), "{left}\n{right}");
    assert_ne!(failing(left), failing(right));
}

const TYPES: [&str; 11] = [
    "bool", "u8", "u16", "u32", "u64", "u128", "i8", "i16", "i32", "i64", "i128",
];

#[test]
fn algebra_bitwise_commutation() {
    for ty in TYPES {
        for op in ["&", "|", "^"] {
            same(
                &format!("fn f(a: {ty}, b: {ty}) -> {ty} {{ a {op} b }}"),
                &format!("fn g(x: {ty}, y: {ty}) -> {ty} {{ y {op} x }}"),
            );
        }
    }
}

#[test]
fn algebra_bitwise_association() {
    for ty in TYPES {
        for op in ["&", "|", "^"] {
            same(
                &format!("fn f(a: {ty}, b: {ty}, c: {ty}) -> {ty} {{ (a {op} b) {op} c }}"),
                &format!("fn g(x: {ty}, y: {ty}, z: {ty}) -> {ty} {{ z {op} (y {op} x) }}"),
            );
        }
    }
    same(
        "fn f(x: u32) -> u32 { (x ^ 1u32) ^ 2u32 }",
        "fn f(x: u32) -> u32 { x ^ (1u32 ^ 2u32) }",
    );
}

#[test]
fn algebra_comparison_direction() {
    for ty in TYPES {
        for (forward, reverse) in [(">", "<"), (">=", "<=")] {
            same(
                &format!("fn f(a: {ty}, b: {ty}) -> bool {{ a {forward} b }}"),
                &format!("fn g(x: {ty}, y: {ty}) -> bool {{ y {reverse} x }}"),
            );
        }
    }
}

#[test]
fn algebra_bitwise_idempotence() {
    for ty in TYPES {
        for op in ["&", "|"] {
            same(
                &format!("fn f(a: {ty}) -> {ty} {{ a {op} a }}"),
                &format!("fn g(x: {ty}) -> {ty} {{ x }}"),
            );
        }
    }
}

#[test]
fn algebra_original_fixtures_and_inherited_negatives() {
    for case in support::cases() {
        if case.family == "algebra"
            && [
                "A01_commute_bitwise",
                "A02_associate_bitwise",
                "A03_reverse_comparison",
                "A12_bitwise_idempotence",
            ]
            .contains(&case.id.as_str())
        {
            same(&case.a, &case.b);
        } else if case.expected != "same" {
            separate(&case.a, &case.b);
        }
    }
}

#[test]
fn algebra_effects_and_unstable_reads_remain_ordered() {
    for (left, right) in [
        ("left() & right()", "right() & left()"),
        ("read() | read()", "read()"),
        ("*a & *a", "*a"),
        ("a.get() | a.get()", "a.get()"),
        (
            "(left() ^ right()) ^ third()",
            "left() ^ (right() ^ third())",
        ),
        ("left() > right()", "right() < left()"),
    ] {
        separate(
            &format!("fn f(a: &u32) -> u32 {{ {left} }}"),
            &format!("fn f(a: &u32) -> u32 {{ {right} }}"),
        );
    }
}

#[test]
fn algebra_overloads_and_inference_remain_opaque() {
    for ty in ["Custom", "T", "f32", "usize"] {
        for (left, right) in [
            ("a & b", "b & a"),
            ("(a ^ b) ^ c", "a ^ (b ^ c)"),
            ("a > b", "b < a"),
            ("a | a", "a"),
        ] {
            separate(
                &format!("fn f<T>(a: {ty}, b: {ty}, c: {ty}) {{ {left} }}"),
                &format!("fn f<T>(a: {ty}, b: {ty}, c: {ty}) {{ {right} }}"),
            );
        }
    }
    separate("fn f() { 1 & 2 }", "fn f() { 2 & 1 }");
    separate(
        "fn f(a: u8, b: u16) { a & b }",
        "fn f(a: u8, b: u16) { b & a }",
    );
    separate(
        "type u32 = Custom; fn f(a: u32, b: u32) { a > b }",
        "type u32 = Custom; fn f(a: u32, b: u32) { b < a }",
    );
}

#[test]
fn algebra_storage_boundaries_preserve_expression_results() {
    separate(
        "fn f(a: u32) { let result = &(a & a); consume(result, &a); }",
        "fn f(a: u32) { let result = &a; consume(result, &a); }",
    );
    separate(
        "fn f(a: u32) { let x = a & a; let y = a; consume(&x, &y); }",
        "fn f(a: u32) { let x = a; consume(&x, &x); }",
    );
    separate(
        "fn f() { let mut a = 1u32; let x = a & a; a = 2u32; (x, a) }",
        "fn f() { let mut a = 1u32; let x = a & a; a = 2u32; (a, x) }",
    );
}

#[test]
fn algebra_observers_and_contextual_uncertainty_preserve_syntax() {
    for prefix in ["#[observe]", "#[cfg(feature = \"unknown\")]"] {
        separate(
            &format!("{prefix} fn f(a: u32, b: u32) {{ a & b }}"),
            &format!("{prefix} fn f(a: u32, b: u32) {{ b & a }}"),
        );
    }
    separate(
        "fn f(a: u32, b: u32) { observe!(); a & b }",
        "fn f(a: u32, b: u32) { observe!(); b & a }",
    );
    separate(
        "use unknown::*; fn f(a: u32, b: u32) { a & b }",
        "use unknown::*; fn f(a: u32, b: u32) { b & a }",
    );
    separate(
        "fn f(a: u32, b: u32) { stringify!(a & b) }",
        "fn f(a: u32, b: u32) { stringify!(b & a) }",
    );
}

#[test]
fn algebra_operator_kinds_and_output_positions_remain_distinct() {
    for (left, right) in [
        ("a & b", "a | b"),
        ("a ^ a", "a"),
        ("a > b", "a >= b"),
        ("a == b", "a != b"),
        ("a > b", "a < b"),
        ("(a & b, a)", "(a, b & a)"),
        ("(a & b) | c", "a & (b | c)"),
        ("(a + b) + c", "a + (b + c)"),
    ] {
        separate(
            &format!("fn f(a: u32, b: u32, c: u32) {{ {left} }}"),
            &format!("fn f(a: u32, b: u32, c: u32) {{ {right} }}"),
        );
    }
}
