//! Dependency canonicalization baseline through both public modes.

#[path = "support/dependency.rs"]
mod support;

fn parse(source: &str) -> syn::File {
    syn::parse_str(source).unwrap_or_else(|error| panic!("{source}: {error}"))
}

fn passing(source: &str) -> String {
    syn_canon::canonicalize(parse(source)).to_string()
}

fn failing(source: &str) -> String {
    syn_canon::canonicalize_failing(parse(source)).to_string()
}

fn expect_same(id: &str, a: &str, b: &str) {
    assert_eq!(passing(a), passing(b), "passing forms of {id} must match");
    assert_eq!(failing(a), failing(b), "failing forms of {id} must match");
}

fn expect_separate(id: &str, a: &str, b: &str) {
    assert_ne!(passing(a), passing(b), "passing forms of {id} must differ");
    assert_ne!(failing(a), failing(b), "failing forms of {id} must differ");
}

fn c01() -> &'static support::Case {
    support::cases()
        .iter()
        .find(|case| case.id == "C01_alpha_control")
        .expect("the fixture names C01_alpha_control")
}

#[test]
fn negative_controls_stay_separate_in_passing_mode() {
    for case in support::cases()
        .iter()
        .filter(|case| case.expected != "same")
    {
        assert_ne!(
            passing(&case.a),
            passing(&case.b),
            "passing forms of {} must differ",
            case.id
        );
    }
}

#[test]
fn negative_controls_stay_separate_in_failing_mode() {
    for case in support::cases()
        .iter()
        .filter(|case| case.expected != "same")
    {
        assert_ne!(
            failing(&case.a),
            failing(&case.b),
            "failing forms of {} must differ",
            case.id
        );
    }
}

#[test]
fn alpha_control_c01_stays_equal_in_passing_mode() {
    let case = c01();
    assert_eq!(
        passing(&case.a),
        passing(&case.b),
        "passing forms of {} must match",
        case.id
    );
}

#[test]
fn alpha_control_c01_stays_equal_in_failing_mode() {
    let case = c01();
    assert_eq!(
        failing(&case.a),
        failing(&case.b),
        "failing forms of {} must match",
        case.id
    );
}

#[test]
fn positive_candidates_stay_separate_in_failing_mode() {
    for case in support::cases()
        .iter()
        .filter(|case| case.expected == "same" && case.family != "control")
    {
        assert_ne!(
            failing(&case.a),
            failing(&case.b),
            "failing forms of {} must differ",
            case.id
        );
    }
}

#[test]
fn macro_and_attribute_literal_payloads_stay_opaque() {
    let macro_a = "fn main() { m!(0x10u32); }";
    let macro_b = "fn main() { m!(16u32); }";
    expect_separate("a macro literal payload", macro_a, macro_b);

    let outside_a = "fn g(value: u32) {} fn main() { let x = 0x10u32; g(x); }";
    let outside_b = "fn g(value: u32) {} fn main() { let x = 16u32; g(x); }";
    expect_same("a literal outside a macro", outside_a, outside_b);

    let attribute_a = "#[m(0x10)] fn g() {} fn main() { g(); }";
    let attribute_b = "#[m(16)] fn g() {} fn main() { g(); }";
    expect_separate("an attribute literal payload", attribute_a, attribute_b);
}

#[test]
fn references_keep_the_binding_and_the_ampersand() {
    let a = "fn main() { let x = 1u32; g(&x); }";
    let b = "fn main() { let y = 1u32; g(&y); }";
    expect_same("a reference occurrence", a, b);

    let c = "fn main() { let x = 1u32; g(x); }";
    expect_separate("the ampersand", a, c);
}

#[test]
fn raw_identifiers_rename_as_binders_and_stay_as_fields() {
    let binder_a = "fn f(r#type: u32) -> u32 { r#type }";
    let binder_b = "fn f(r#match: u32) -> u32 { r#match }";
    expect_same("a raw identifier binder", binder_a, binder_b);

    let field_a = "struct S { r#type: u32, r#match: u32 } fn f(s: &S) -> u32 { s.r#type }";
    let field_b = "struct S { r#type: u32, r#match: u32 } fn f(s: &S) -> u32 { s.r#match }";
    expect_separate("a raw identifier field", field_a, field_b);
}

#[test]
fn lexical_shadowing_keeps_the_two_origins() {
    let a = "fn f() -> u32 { let x = 1u32; let x = x + 1u32; x }";
    let b = "fn f() -> u32 { let y = 1u32; let y = y + 1u32; y }";
    expect_same("the two shadow origins", a, b);

    let c = "fn f() -> u32 { let x = 1u32; let x = 3u32; x }";
    expect_separate("the shadow initializer", a, c);
}

#[test]
fn initializers_resolve_before_their_binding() {
    let a = "fn f(x: u32) -> u32 { let x = x + 1u32; x }";
    let b = "fn f(y: u32) -> u32 { let y = y + 1u32; y }";
    expect_same("an initializer reading its shadowed parameter", a, b);

    let c = "fn f(x: u32) -> u32 { let x = x; x }";
    expect_separate("the initializer", a, c);
}

#[test]
fn generated_name_lookalikes_stay_in_their_positions() {
    let literal_a = "fn main() { let s = \"_canon_0\"; g(s); }";
    let literal_b = "fn main() { let s = \"_canon_1\"; g(s); }";
    expect_separate(
        "a generated-name-looking string literal",
        literal_a,
        literal_b,
    );

    let item_a = "fn _canon_0() {} fn main() { _canon_0(); }";
    let item_b = "fn plain() {} fn main() { plain(); }";
    expect_same("a generated-name-looking item name", item_a, item_b);
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

fn wiped(file: syn::File) -> syn::File {
    let mut file = file;
    syn::visit_mut::visit_file_mut(&mut SpanWiper, &mut file);
    file
}

#[test]
fn shared_spans_do_not_change_the_canonical_form() {
    let source = "fn f(x: u32) -> u32 { let y = x + 1u32; let y = y + 1u32; y }";
    let (parsed, shared) = (parse(source), wiped(parse(source)));
    assert_eq!(
        syn_canon::canonicalize(shared).to_string(),
        syn_canon::canonicalize(parsed).to_string(),
        "a constructed file sharing every token span keeps the passing form"
    );

    let (parsed, shared) = (parse(source), wiped(parse(source)));
    assert_eq!(
        syn_canon::canonicalize_failing(shared).to_string(),
        syn_canon::canonicalize_failing(parsed).to_string(),
        "a constructed file sharing every token span keeps the failing form"
    );
}
