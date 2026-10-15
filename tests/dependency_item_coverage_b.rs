//! Item partition owners, label remaps, and observation-scalar boundaries.

fn form(source: &str) -> syn_canon::CanonicalForm {
    syn_canon::canonicalize(syn::parse_file(source).unwrap())
}

fn failing(source: &str) -> syn_canon::CanonicalForm {
    syn_canon::canonicalize_failing(syn::parse_file(source).unwrap())
}

/// A context item behind a trait, then a two-item region in both orders.
fn region_pair(context: &str) -> (String, String) {
    (
        format!("{context} trait T {{}} fn one() -> u32 {{ 1 }} fn two() -> u32 {{ 2 }}"),
        format!("{context} trait T {{}} fn two() -> u32 {{ 2 }} fn one() -> u32 {{ 1 }}"),
    )
}

/// A two-helper block region in both orders, with one context let.
fn block_pair(let_x: &str) -> (String, String) {
    (
        format!(
            "fn run() -> u32 {{ fn h0(v: u32) -> u32 {{ v & 15u32 }} fn h1(v: u32) -> u32 {{ v & 7u32 }} {let_x} x }}"
        ),
        format!(
            "fn run() -> u32 {{ fn h1(v: u32) -> u32 {{ v & 7u32 }} fn h0(v: u32) -> u32 {{ v & 15u32 }} {let_x} x }}"
        ),
    )
}

#[test]
fn impl_over_array_self_type_keeps_the_member_order() {
    let a = "impl [u8; 3] { fn b() {} fn a() {} }";
    let b = "impl [u8; 3] { fn a() {} fn b() {} }";
    assert_ne!(failing(a), failing(b));
    assert_ne!(form(a), form(b));
}

#[test]
fn impl_over_qualified_self_type_keeps_the_member_order() {
    let a = "trait Tr { type Item; } struct S; struct Item; impl <S as Tr>::Item { fn b() {} fn a() {} }";
    let b = "trait Tr { type Item; } struct S; struct Item; impl <S as Tr>::Item { fn a() {} fn b() {} }";
    assert_ne!(failing(a), failing(b));
    assert_ne!(form(a), form(b));
}

#[test]
fn self_prefixed_owner_keeps_the_region_schedulable() {
    let a = "struct S; impl self::S { fn two() -> u32 { 2 } fn one() -> u32 { 1 } }";
    let b = "struct S; impl self::S { fn one() -> u32 { 1 } fn two() -> u32 { 2 } }";
    assert_ne!(failing(a), failing(b));
    assert_eq!(form(a), form(b));
    let plain = "struct S; impl S { fn one() -> u32 { 1 } fn two() -> u32 { 2 } }";
    assert_ne!(form(b), form(plain));
}

#[test]
fn associated_type_member_sorts_with_the_function_member() {
    let a = "struct S; impl S { type X = u32; fn f() -> u32 { 1 } }";
    let b = "struct S; impl S { fn f() -> u32 { 1 } type X = u32; }";
    assert_ne!(failing(a), failing(b));
    assert_eq!(form(a), form(b));
}

#[test]
fn negated_and_compared_producers_remap_their_definitions() {
    let context = "fn h() -> bool { let c = 1u32 == 0u32; let x = !c; x }";
    let (a, b) = region_pair(context);
    assert_ne!(failing(&a), failing(&b));
    assert_eq!(form(&a), form(&b));
}

#[test]
fn comparison_with_literal_true_folds_to_the_operand() {
    let context = "fn h() -> bool { let d = (1u32 == 0u32) == true; d }";
    let (a, b) = region_pair(context);
    assert_ne!(failing(&a), failing(&b));
    assert_eq!(form(&a), form(&b));
}

#[test]
fn reversed_comparison_swaps_the_ordered_operands() {
    let context = "fn h() -> bool { let d = 9u32 > 5u32; d }";
    let (a, b) = region_pair(context);
    assert_ne!(failing(&a), failing(&b));
    assert_eq!(form(&a), form(&b));
}

#[test]
fn forward_comparison_builds_the_ordered_label() {
    let context = "fn h() -> bool { let d = 5u32 < 9u32; d }";
    let (a, b) = region_pair(context);
    assert_ne!(failing(&a), failing(&b));
    assert_eq!(form(&a), form(&b));
}

#[test]
fn if_else_over_block_branches_proves_the_branch_value() {
    let context =
        "fn h() -> bool { let c = 1u32 == 0u32; let x = if c { true } else { false }; x }";
    let (a, b) = region_pair(context);
    assert_ne!(failing(&a), failing(&b));
    assert_eq!(form(&a), form(&b));
}

#[test]
fn if_else_with_bare_expression_branch_exits_the_branch_value() {
    let context =
        "fn h() -> bool { let c = 1u32 == 0u32; let x = if c { true } else if c { false }; x }";
    let (a, b) = region_pair(context);
    assert_ne!(failing(&a), failing(&b));
    assert_eq!(form(&a), form(&b));
}

#[test]
fn attributed_else_branch_exits_the_branch_value() {
    let context = "fn h() -> bool { let c = 1u32 == 0u32; let x = if c { true } else { #[must_use] false }; x }";
    let (a, b) = region_pair(context);
    assert_ne!(failing(&a), failing(&b));
    assert_eq!(form(&a), form(&b));
}

#[test]
fn attributed_initializer_exits_the_scalar() {
    let (a, b) = block_pair("let x = #[must_use] 1u32; ");
    assert_ne!(failing(&a), failing(&b));
    assert_eq!(form(&a), form(&b));
}

#[test]
fn parenthesized_initializer_keeps_the_producer_proven() {
    let (a, b) = block_pair("let x = (h0(1u32)); ");
    assert_ne!(failing(&a), failing(&b));
    assert_eq!(form(&a), form(&b));
}

#[test]
fn parenthesized_callee_exits_the_call_projection() {
    let (a, b) = block_pair("let x = (h0)(1u32); ");
    assert_ne!(failing(&a), failing(&b));
    assert_eq!(form(&a), form(&b));
}

#[test]
fn helper_call_with_extra_argument_exits_the_call_projection() {
    let (a, b) = block_pair("let x = h0(1u32, 2u32); ");
    assert_ne!(failing(&a), failing(&b));
    assert_eq!(form(&a), form(&b));
}

#[test]
fn helper_call_with_wrong_argument_type_exits_the_call_projection() {
    let (a, b) = block_pair("let x = h0(true); ");
    assert_ne!(failing(&a), failing(&b));
    assert_eq!(form(&a), form(&b));
}

#[test]
fn duplicated_declaration_keeps_the_region_order_dependent() {
    let a = "const X: u32 = 1; const X: u32 = 2; trait T {} fn one() -> u32 { 1 } fn two() -> u32 { 2 }";
    let b = "const X: u32 = 2; const X: u32 = 1; trait T {} fn one() -> u32 { 1 } fn two() -> u32 { 2 }";
    assert_ne!(failing(a), failing(b));
    assert_ne!(form(a), form(b));
    let control_a = "const X: u32 = 1; const Y: u32 = 2; trait T {} fn one() -> u32 { 1 } fn two() -> u32 { 2 }";
    let control_b = "const Y: u32 = 2; const X: u32 = 1; trait T {} fn one() -> u32 { 1 } fn two() -> u32 { 2 }";
    assert_eq!(form(control_a), form(control_b));
}

#[test]
fn parameter_write_of_a_read_producer_freezes_the_region() {
    let write_first = "fn h(mut a: u32) -> u32 { let x = a; a = 7; one(); two(); x } trait T {} fn one() -> u32 { 1 } fn two() -> u32 { 2 }";
    let write_second = "fn h(mut a: u32) -> u32 { let x = a; a = 7; one(); two(); x } trait T {} fn two() -> u32 { 2 } fn one() -> u32 { 1 }";
    assert_ne!(failing(write_first), failing(write_second));
    assert_ne!(form(write_first), form(write_second));
    let borrow_first = "fn h(mut a: u32) -> u32 { let x = a; let p = &mut a; *p = 3; one(); two(); x } trait T {} fn one() -> u32 { 1 } fn two() -> u32 { 2 }";
    let borrow_second = "fn h(mut a: u32) -> u32 { let x = a; let p = &mut a; *p = 3; one(); two(); x } trait T {} fn two() -> u32 { 2 } fn one() -> u32 { 1 }";
    assert_ne!(failing(borrow_first), failing(borrow_second));
    assert_ne!(form(borrow_first), form(borrow_second));
    let local_first = "fn h(a: u32) -> u32 { let x = a; let mut s = 0; s = x; one(); s } trait T {} fn one() -> u32 { 1 } fn two() -> u32 { 2 }";
    let local_second = "fn h(a: u32) -> u32 { let x = a; let mut s = 0; s = x; one(); s } trait T {} fn two() -> u32 { 2 } fn one() -> u32 { 1 }";
    assert_eq!(form(local_first), form(local_second));
}

#[test]
fn negation_chain_at_the_depth_limit_freezes_the_region() {
    fn chain(depth: usize) -> String {
        let mut expr = "c".to_string();
        for _ in 0..depth {
            expr = format!("!{expr}");
        }
        expr
    }
    fn run(depth: usize, h1_first: bool) -> String {
        let (first, second) = if h1_first {
            (
                "fn h1(v: u32) -> u32 { v & 7u32 }",
                "fn h0(v: u32) -> u32 { v & 15u32 }",
            )
        } else {
            (
                "fn h0(v: u32) -> u32 { v & 15u32 }",
                "fn h1(v: u32) -> u32 { v & 7u32 }",
            )
        };
        format!(
            "fn run() -> bool {{ {first} {second} let c = h0(1u32) == 0u32; let x = {}; x }}",
            chain(depth)
        )
    }
    assert_eq!(form(&run(63, false)), form(&run(63, true)));
    assert_ne!(failing(&run(64, false)), failing(&run(64, true)));
    assert_ne!(form(&run(64, false)), form(&run(64, true)));
}

/// Sixty group fns, each referencing five others, in both orders.
fn reference_region() -> (String, String) {
    let mut members: Vec<String> = Vec::new();
    for index in 0..60 {
        let refs: Vec<String> = (0..5)
            .map(|step| format!("one{}()", (index + step * 13) % 60))
            .collect();
        members.push(format!("fn one{index}() -> u32 {{ {} }}", refs.join(" & ")));
    }
    let region = members.join(" ");
    let reversed: Vec<&str> = members.iter().map(String::as_str).rev().collect();
    (region, reversed.join(" "))
}

#[test]
fn oversized_context_reference_stream_freezes_the_region() {
    let (region, reversed) = reference_region();
    let refs: Vec<String> = (0..8_300)
        .map(|index| format!("one{}()", index % 60))
        .collect();
    let huge = format!(
        "fn huge() -> u32 {{ let v = ({}); let t = 1u32; v.0 + t }}",
        refs.join(", ")
    );
    let a = format!("{huge} trait T {{}} {region} trait U {{}} fn tail() -> u32 {{ 9 }}");
    let b = format!("{huge} trait T {{}} {reversed} trait U {{}} fn tail() -> u32 {{ 9 }}");
    assert_ne!(failing(&a), failing(&b));
    assert_ne!(form(&a), form(&b));
}

#[test]
fn oversized_partition_labels_freeze_the_region() {
    let (region, reversed) = reference_region();
    let refs: Vec<String> = (0..7_900)
        .map(|index| format!("one{}()", index % 60))
        .collect();
    let huge = format!("fn huge() -> u32 {{ let v = ({}); v.0 }}", refs.join(", "));
    let a = format!("{huge} trait T {{}} {region} trait U {{}} fn tail() -> u32 {{ 9 }}");
    let b = format!("{huge} trait T {{}} {reversed} trait U {{}} fn tail() -> u32 {{ 9 }}");
    assert_ne!(failing(&a), failing(&b));
    assert_ne!(form(&a), form(&b));
}

#[test]
fn if_else_over_a_proven_comparison_proves_the_branch_value() {
    let context = "fn h() -> bool { let x = if 1u32 == 0u32 { true } else { false }; x }";
    let (a, b) = region_pair(context);
    assert_ne!(failing(&a), failing(&b));
    assert_eq!(form(&a), form(&b));
}

#[test]
fn nested_write_to_an_outer_parameter_keeps_the_region_schedulable() {
    let a = "fn h(a: u32) -> u32 { fn g() { a = 5; } one(); two(); a } trait T {} fn one() -> u32 { 1 } fn two() -> u32 { 2 }";
    let b = "fn h(a: u32) -> u32 { fn g() { a = 5; } one(); two(); a } trait T {} fn two() -> u32 { 2 } fn one() -> u32 { 1 }";
    assert_ne!(failing(a), failing(b));
    assert_eq!(form(a), form(b));
}

#[test]
fn write_to_a_field_from_a_trait_name_keeps_the_region_schedulable() {
    let a = "struct P(u32); trait U {} fn h() -> u32 { P(1u32).0 = U; one(); two(); 1 } trait T {} fn one() -> u32 { 1 } fn two() -> u32 { 2 }";
    let b = "struct P(u32); trait U {} fn h() -> u32 { P(1u32).0 = U; one(); two(); 1 } trait T {} fn two() -> u32 { 2 } fn one() -> u32 { 1 }";
    assert_ne!(failing(a), failing(b));
    assert_eq!(form(a), form(b));
}
