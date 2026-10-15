//! Item observation roots and bounded original-producer projections.

use std::fmt::Write as _;

fn form(source: &str) -> syn_canon::CanonicalForm {
    syn_canon::canonicalize(syn::parse_file(source).unwrap())
}

fn failing(source: &str) -> syn_canon::CanonicalForm {
    syn_canon::canonicalize_failing(syn::parse_file(source).unwrap())
}

#[test]
fn independent_items_fold_with_a_clean_observation_context() {
    let a = "fn h()->u32{1} trait T{} fn r()->u32{1} fn s()->u32{2}";
    let b = "fn h()->u32{1} trait T{} fn s()->u32{2} fn r()->u32{1}";
    assert_ne!(failing(a), failing(b));
    assert_eq!(form(a), form(b));
}

#[test]
fn ordered_observations_fold_every_permutation_to_one_form() {
    let main = "fn main(){two();one();}";
    let a = format!("fn one()->u32{{1}} fn two()->u32{{2}} {main}");
    let b = format!("fn two()->u32{{2}} fn one()->u32{{1}} {main}");
    assert_ne!(failing(&a), failing(&b));
    assert_eq!(form(&a), form(&b));
    let items = ["fn p()->u32{1}", "fn q()->u32{2}", "fn w()->u32{3}"];
    let three = format!("{} fn main(){{p();q();w();}}", items.join(" "));
    let base = form(&three);
    let orders = [
        [0usize, 1, 2],
        [0, 2, 1],
        [1, 0, 2],
        [1, 2, 0],
        [2, 0, 1],
        [2, 1, 0],
    ];
    for order in orders {
        let source = format!(
            "{} fn main(){{p();q();w();}}",
            order
                .iter()
                .map(|&index| items[index])
                .collect::<Vec<_>>()
                .join(" ")
        );
        assert_eq!(form(&source), base, "{order:?}");
    }
}

#[test]
fn reassigned_parameter_source_invalidates_the_producer_projection() {
    let a = "fn h(mut a: u32) -> u32 { let x = a; a = 7; one(); two(); x } trait T {} fn one() -> u32 { 1 } fn two() -> u32 { 2 }";
    let b = "fn h(mut a: u32) -> u32 { let x = a; a = 7; one(); two(); x } trait T {} fn two() -> u32 { 2 } fn one() -> u32 { 1 }";
    assert_ne!(failing(a), failing(b));
    assert_ne!(form(a), form(b));
    let clean_a = "fn h(a: u32) -> u32 { let x = a; one(); two(); x } trait T {} fn one() -> u32 { 1 } fn two() -> u32 { 2 }";
    let clean_b = "fn h(a: u32) -> u32 { let x = a; one(); two(); x } trait T {} fn two() -> u32 { 2 } fn one() -> u32 { 1 }";
    assert_eq!(form(clean_a), form(clean_b));
    let no_vertex_a = "fn h(mut a: u32) -> u32 { a = 7; one(); two(); 0 } trait T {} fn one() -> u32 { 1 } fn two() -> u32 { 2 }";
    let no_vertex_b = "fn h(mut a: u32) -> u32 { a = 7; one(); two(); 0 } trait T {} fn two() -> u32 { 2 } fn one() -> u32 { 1 }";
    assert_eq!(form(no_vertex_a), form(no_vertex_b));
}

#[test]
fn mutably_borrowed_parameter_source_invalidates_the_producer_projection() {
    let a = "fn h(mut a: u32) -> u32 { let x = a; let p = &mut a; *p = 3; one(); two(); x } trait T {} fn one() -> u32 { 1 } fn two() -> u32 { 2 }";
    let b = "fn h(mut a: u32) -> u32 { let x = a; let p = &mut a; *p = 3; one(); two(); x } trait T {} fn two() -> u32 { 2 } fn one() -> u32 { 1 }";
    assert_ne!(failing(a), failing(b));
    assert_ne!(form(a), form(b));
    let no_vertex_a = "fn h(mut a: u32) -> u32 { let p = &mut a; *p = 3; one(); two(); 0 } trait T {} fn one() -> u32 { 1 } fn two() -> u32 { 2 }";
    let no_vertex_b = "fn h(mut a: u32) -> u32 { let p = &mut a; *p = 3; one(); two(); 0 } trait T {} fn two() -> u32 { 2 } fn one() -> u32 { 1 }";
    assert_eq!(form(no_vertex_a), form(no_vertex_b));
}

#[test]
fn stored_value_source_invalidates_the_producer_projection() {
    let a = "fn h(a: u32, out: &mut u32) -> u32 { let x = a; *out = x; one(); two(); 0 } trait T {} fn one() -> u32 { 1 } fn two() -> u32 { 2 }";
    let b = "fn h(a: u32, out: &mut u32) -> u32 { let x = a; *out = x; one(); two(); 0 } trait T {} fn two() -> u32 { 2 } fn one() -> u32 { 1 }";
    assert_ne!(failing(a), failing(b));
    assert_ne!(form(a), form(b));
    let no_source_a = "fn h(a: u32, out: &mut u32) -> u32 { *out = a; one(); two(); 0 } trait T {} fn one() -> u32 { 1 } fn two() -> u32 { 2 }";
    let no_source_b = "fn h(a: u32, out: &mut u32) -> u32 { *out = a; one(); two(); 0 } trait T {} fn two() -> u32 { 2 } fn one() -> u32 { 1 }";
    assert_eq!(form(no_source_a), form(no_source_b));
}

#[test]
fn compound_write_of_a_value_source_invalidates_the_producer_projection() {
    let a = "fn h(a: u32, out: &mut u32) -> u32 { let x = a; *out += x; one(); two(); 0 } trait T {} fn one() -> u32 { 1 } fn two() -> u32 { 2 }";
    let b = "fn h(a: u32, out: &mut u32) -> u32 { let x = a; *out += x; one(); two(); 0 } trait T {} fn two() -> u32 { 2 } fn one() -> u32 { 1 }";
    assert_ne!(failing(a), failing(b));
    assert_ne!(form(a), form(b));
    let no_source_a = "fn h(a: u32, out: &mut u32) -> u32 { *out += a; one(); two(); 0 } trait T {} fn one() -> u32 { 1 } fn two() -> u32 { 2 }";
    let no_source_b = "fn h(a: u32, out: &mut u32) -> u32 { *out += a; one(); two(); 0 } trait T {} fn two() -> u32 { 2 } fn one() -> u32 { 1 }";
    assert_eq!(form(no_source_a), form(no_source_b));
}

#[test]
fn immutable_borrow_of_a_value_source_keeps_the_projection() {
    let a = "fn h(a: u32) -> u32 { let x = a; let p = &a; *p + x } trait T {} fn one() -> u32 { 1 } fn two() -> u32 { 2 }";
    let b = "fn h(a: u32) -> u32 { let x = a; let p = &a; *p + x } trait T {} fn two() -> u32 { 2 } fn one() -> u32 { 1 }";
    assert_eq!(form(a), form(b));
}

#[test]
fn local_target_write_of_a_value_source_keeps_the_projection() {
    let a = "fn h(a: u32) -> u32 { let x = a; let mut s = 0; s = x; one(); s } trait T {} fn one() -> u32 { 1 } fn two() -> u32 { 2 }";
    let b = "fn h(a: u32) -> u32 { let x = a; let mut s = 0; s = x; one(); s } trait T {} fn two() -> u32 { 2 } fn one() -> u32 { 1 }";
    assert_eq!(form(a), form(b));
}

#[test]
fn oversized_appended_producer_graph_freezes_the_region() {
    let mut body = String::with_capacity(4_000 * 32);
    for index in 0..4_000 {
        write!(body, "let p{index} = h{}({index}u32); ", index % 2).unwrap();
    }
    let a = format!(
        "fn run() -> u32 {{ fn h0(v: u32) -> u32 {{ v & 15u32 }} fn h1(v: u32) -> u32 {{ v & 7u32 }} {body}p0 ^ p3999 }}"
    );
    let b = format!(
        "fn run() -> u32 {{ fn h1(v: u32) -> u32 {{ v & 7u32 }} fn h0(v: u32) -> u32 {{ v & 15u32 }} {body}p0 ^ p3999 }}"
    );
    assert_ne!(failing(&a), failing(&b));
    assert_ne!(form(&a), form(&b));
}

#[test]
fn small_appended_producer_graph_keeps_the_region_schedulable() {
    let a = "fn run() -> u32 { fn h0(v: u32) -> u32 { v & 15u32 } fn h1(v: u32) -> u32 { v & 7u32 } let p0 = h0(1u32); let p1 = h1(2u32); let p2 = h0(3u32); let p3 = h1(4u32); p0 ^ p1 ^ p2 ^ p3 }";
    let b = "fn run() -> u32 { fn h1(v: u32) -> u32 { v & 7u32 } fn h0(v: u32) -> u32 { v & 15u32 } let p0 = h0(1u32); let p1 = h1(2u32); let p2 = h0(3u32); let p3 = h1(4u32); p0 ^ p1 ^ p2 ^ p3 }";
    assert_eq!(form(a), form(b));
}

#[test]
fn large_unused_context_keeps_the_region_schedulable() {
    let mut context = String::with_capacity(1200 * 32);
    for index in 0..1200 {
        write!(context, "fn c{index}() -> u32 {{ 0 }} ").unwrap();
    }
    let a = format!("{context} trait T {{}} fn one() -> u32 {{ 1 }} fn two() -> u32 {{ 2 }}");
    let b = format!("{context} trait T {{}} fn two() -> u32 {{ 2 }} fn one() -> u32 {{ 1 }}");
    assert_eq!(form(&a), form(&b));
}

#[test]
fn nested_call_context_respects_the_projection_depth_limit() {
    let pair = |calls: usize| {
        let value = (0..calls).fold("3u32".to_owned(), |value, index| {
            format!("h{}({value})", index % 2)
        });
        let head = "fn h0(v: u32) -> u32 { v & 15u32 } fn h1(v: u32) -> u32 { v & 7u32 }";
        let reverse = "fn h1(v: u32) -> u32 { v & 7u32 } fn h0(v: u32) -> u32 { v & 15u32 }";
        (
            format!("fn run() -> u32 {{ {head} let value = {value}; value }}"),
            format!("fn run() -> u32 {{ {reverse} let value = {value}; value }}"),
        )
    };
    let (left, right) = pair(62);
    assert_eq!(form(&left), form(&right));
    let (left, right) = pair(63);
    assert_ne!(form(&left), form(&right));
    assert_ne!(failing(&left), failing(&right));
}
