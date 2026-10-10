//! Public guards for the algebra laws beyond the four named cases.

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

#[test]
fn comparison_kinds_and_operands_stay_ordered() {
    for (left, right) in [
        ("a == b", "y == x"),
        ("a != b", "y != x"),
        ("a > b", "y > x"),
        ("a >= b", "y >= x"),
        ("a == b", "y < x"),
        ("a < b", "y == x"),
    ] {
        separate(
            &format!("fn f(a: u32, b: u32) -> bool {{ {left} }}"),
            &format!("fn g(x: u32, y: u32) -> bool {{ {right} }}"),
        );
    }
}

#[test]
fn idempotence_folds_only_the_exact_origin() {
    same(
        "fn f(a: u32) -> u32 { let x = a; let y = a; x | x }",
        "fn g(p: u32) -> u32 { let u = p; let v = p; u }",
    );
    same(
        "fn f(a: u32, b: u32) -> (u32, u32) { (a & a, b | b) }",
        "fn g(x: u32, y: u32) -> (u32, u32) { (x, y) }",
    );
    separate(
        "fn f(a: u32, b: u32) -> u32 { a | b }",
        "fn g(x: u32, y: u32) -> u32 { x | x }",
    );
    separate(
        "fn f(a: u32, b: u32) -> u32 { let x = a; let y = b; x | y }",
        "fn g(a: u32, b: u32) -> u32 { let x = a; let y = b; x | x }",
    );
}

#[test]
fn literal_subset_folds_converge_on_signed_results() {
    same(
        "fn f(a: u32) -> u32 { a & (1u32 & 2u32) }",
        "fn g(x: u32) -> u32 { x & 0u32 }",
    );
    same(
        "fn f(a: u32) -> u32 { a | (1u32 & 2u32) }",
        "fn g(x: u32) -> u32 { x | 0u32 }",
    );
    separate(
        "fn f(a: u32) -> u32 { a & 0u32 }",
        "fn g(x: u32) -> u32 { x | 0u32 }",
    );
    same(
        "fn f(a: i32) -> i32 { a | -2147483648i32 | 2147483647i32 }",
        "fn g(x: i32) -> i32 { x | 2147483647i32 | -2147483648i32 }",
    );
    same(
        "fn f(a: i32) -> i32 { a ^ -2147483648i32 ^ -2147483648i32 }",
        "fn g(x: i32) -> i32 { x ^ 0i32 }",
    );
    separate(
        "fn f(a: i32) -> i32 { a ^ -2147483648i32 ^ -2147483648i32 }",
        "fn g(x: i32) -> i32 { x ^ -2147483648i32 }",
    );
    separate(
        "fn f(a: i32) -> i32 { a | 4294967295i32 }",
        "fn g(x: i32) -> i32 { x | -1i32 }",
    );
}

#[test]
fn composed_schedule_constants_and_algebra_laws() {
    same(
        "fn f(a: u32, b: u32) -> u32 { (a & b) | (1u32 & 2u32) }",
        "fn g(x: u32, y: u32) -> u32 { (2u32 & 1u32) | (y & x) }",
    );
    same(
        "fn f(a: u32, b: u32) -> u32 { (a | a) & (b & 1u32 & 2u32) }",
        "fn g(x: u32, y: u32) -> u32 { x & (y & 0u32) }",
    );
    separate(
        "fn f(a: u32, b: u32) -> u32 { a & (b ^ 0u32) }",
        "fn g(x: u32, y: u32) -> u32 { x & y }",
    );
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

fn wiped_form(source: &str, compiles: bool) -> syn_canon::CanonicalForm {
    let mut file = syn::parse_str::<syn::File>(source).unwrap();
    syn::visit_mut::visit_file_mut(&mut SpanWiper, &mut file);
    if compiles {
        syn_canon::canonicalize(file)
    } else {
        syn_canon::canonicalize_failing(file)
    }
}

#[test]
fn shared_spans_keep_the_algebra_relations() {
    let plain = "fn f(a: u32, b: u32) -> u32 { (a | b) & (1u32 & 2u32) }";
    let reordered = "fn g(x: u32, y: u32) -> u32 { (1u32 & 2u32) & (y | x) }";
    let held_a = "fn f(a: u32, b: u32) -> u32 { a & b }";
    let held_b = "fn g(x: u32, y: u32) -> u32 { x ^ y }";
    assert_eq!(wiped_form(plain, true), form(plain));
    assert_eq!(wiped_form(plain, true), wiped_form(reordered, true));
    assert_ne!(wiped_form(plain, false), wiped_form(reordered, false));
    assert_eq!(wiped_form(held_a, true), form(held_a));
    assert_ne!(wiped_form(held_a, true), wiped_form(held_b, true));
}

#[test]
fn nested_blocks_take_the_laws_independently() {
    same(
        "fn f(a: u32, b: u32) -> u32 { let t = { a & b }; t }",
        "fn g(x: u32, y: u32) -> u32 { let t = { y & x }; t }",
    );
    same(
        "fn f(a: u32, b: u32) -> u32 { if true { a & b } else { 0u32 } }",
        "fn g(x: u32, y: u32) -> u32 { if true { y & x } else { 0u32 } }",
    );
    same(
        "fn f(a: u32, b: u32) -> u32 { match a { 1u32 => { let seed = b; seed & a }, _ => { 0u32 } } }",
        "fn g(x: u32, y: u32) -> u32 { match x { 1u32 => { let seed = y; x & seed }, _ => { 0u32 } } }",
    );
    same(
        "fn f(a: u32, b: u32) { while true { b & a; break; } }",
        "fn g(x: u32, y: u32) { while true { x & y; break; } }",
    );
    same(
        "fn f(a: u32, b: u32) { let t = |x: u32| { let seed = x; seed & a }; t(b); }",
        "fn g(p: u32, b: u32) { let t = |x: u32| { let seed = x; p & seed }; t(b); }",
    );
}

#[test]
fn scoped_non_block_syntax_stays_ordered() {
    separate(
        "fn f(a: bool, b: bool) -> u32 { if a & b { 0u32 } else { 1u32 } }",
        "fn g(x: bool, y: bool) -> u32 { if y & x { 0u32 } else { 1u32 } }",
    );
    separate(
        "fn f(a: u32, b: u32) { match a & b { _ => {} } }",
        "fn g(x: u32, y: u32) { match y & x { _ => {} } }",
    );
    separate(
        "fn f(a: u32, b: u32) -> u32 { match a { 1u32 => b & a, _ => 0u32 } }",
        "fn g(x: u32, y: u32) -> u32 { match x { 1u32 => x & y, _ => 0u32 } }",
    );
    separate(
        "fn f(a: bool, b: bool) { while a & b { 0u32; } }",
        "fn g(x: bool, y: bool) { while y & x { 0u32; } }",
    );
    separate(
        "fn f(a: u32, b: u32) { let t = |x: u32| x & a; t(b); }",
        "fn g(p: u32, b: u32) { let t = |x: u32| p & x; t(b); }",
    );
}

#[test]
fn contextual_aliases_take_the_laws() {
    same(
        "type A = u32; fn f(a: A, b: A) -> A { a & b }",
        "type B = u32; fn g(x: B, y: B) -> B { y & x }",
    );
    same(
        "type A = u32; type B = A; fn f(a: B, b: B) -> bool { a > b }",
        "type C = u32; type D = C; fn g(x: D, y: D) -> bool { y < x }",
    );
    separate(
        "type A = u32; fn f(a: A, b: A) -> bool { a == b }",
        "type B = u32; fn g(x: B, y: B) -> bool { y == x }",
    );
    separate(
        "struct Point; type A = Point; fn f(a: A, b: A) { a & b }",
        "struct Dot; type B = Dot; fn g(x: B, y: B) { y & x }",
    );
}

fn deep(nestings: usize) -> String {
    let mut expr = "a".to_owned();
    for _ in 0..nestings {
        expr = format!("a & ({expr})");
    }
    format!("fn f(a: u32) -> u32 {{ {expr} }}")
}

fn balanced_body(names: &[String]) -> String {
    fn build(names: &[String], out: &mut String) {
        if names.len() == 1 {
            out.push_str(&names[0]);
            return;
        }
        let mid = names.len() / 2;
        out.push('(');
        build(&names[..mid], out);
        out.push_str(" | ");
        build(&names[mid..], out);
        out.push(')');
    }
    let mut body = String::new();
    build(names, &mut body);
    body
}

fn wide(leaves: usize) -> String {
    let names: Vec<String> = (0..leaves).map(|_| "a".to_owned()).collect();
    let body = balanced_body(&names);
    format!("fn f(a: u32) -> u32 {{ {body} }}")
}

#[test]
fn bound_exhaustion_falls_back_atomic_and_deterministic() {
    same(&deep(63), "fn g(a: u32) -> u32 { a }");
    separate(&deep(64), "fn g(a: u32) -> u32 { a }");
    same(&wide(8192), "fn g(a: u32) -> u32 { a }");
    let deep_over = deep(64);
    let wide_over = wide(8193);
    separate(&wide_over, "fn g(a: u32) -> u32 { a }");
    for (source, leaves) in [(&deep_over, 65usize), (&wide_over, 8193)] {
        let first = form(source);
        let second = form(source);
        assert_eq!(first, second, "the over-bound form is deterministic");
        assert_eq!(first.leaf_tokens(), second.leaf_tokens());
        assert_eq!(first.body_units(), second.body_units());
        assert_eq!(first.body_units(), 2 * leaves - 1);
        assert!(
            first.leaf_tokens() >= leaves,
            "the whole over-bound region is retained"
        );
    }
}

#[test]
fn scheduled_scale_forms_are_deterministic() {
    let names: Vec<String> = (0..4000).map(|index| format!("a{index}")).collect();
    let parameters = names
        .iter()
        .map(|name| format!("{name}: u32"))
        .collect::<Vec<_>>()
        .join(", ");
    let forward = format!("fn f({parameters}) -> u32 {{ {} }}", balanced_body(&names));
    let reversed: Vec<String> = names.iter().rev().cloned().collect();
    let reversed = format!(
        "fn f({parameters}) -> u32 {{ {} }}",
        balanced_body(&reversed)
    );
    same(&forward, &reversed);
    let first = form(&forward);
    let second = form(&forward);
    assert_eq!(first.leaf_tokens(), second.leaf_tokens());
    assert_eq!(first.body_units(), second.body_units());
}
