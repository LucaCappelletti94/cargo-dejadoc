//! Extended boolean proof domains through passing and failing public comparison.

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

#[test]
fn double_negation_cancels_over_stable_scalars_and_aliases() {
    same(
        "fn f(p: bool) -> bool { !!!p }",
        "fn g(x: bool) -> bool { !x }",
    );
    same(
        "fn f(a: bool, b: bool) -> bool { !!(a && b) }",
        "fn g(x: bool, y: bool) -> bool { x && y }",
    );
    same(
        "fn f(a: bool, b: bool) -> bool { !(!(a || b)) }",
        "fn g(x: bool, y: bool) -> bool { x || y }",
    );
    same(
        "type P = bool; fn f(a: P) -> P { !!a }",
        "type P = bool; fn g(x: P) -> P { x }",
    );
    same(
        "fn f(r#type: bool) -> bool { !!r#type }",
        "fn g(x: bool) -> bool { x }",
    );
}

#[test]
fn double_negation_guards_keep_single_and_foreign_negation() {
    separate(
        "fn f(p: bool) -> bool { !p }",
        "fn g(x: bool) -> bool { x }",
    );
    separate(
        "fn f(p: bool) -> bool { !!p }",
        "fn g(x: bool) -> bool { !x }",
    );
    separate("fn f(a: i32) -> i32 { -(-a) }", "fn g(x: i32) -> i32 { x }");
    separate(
        "struct Flag; impl core::ops::Not for Flag { type Output = Flag; fn not(self) -> Flag { Flag } } fn f(a: Flag) -> Flag { !!a }",
        "struct Flag; impl core::ops::Not for Flag { type Output = Flag; fn not(self) -> Flag { Flag } } fn g(x: Flag) -> Flag { x }",
    );
    separate(
        "struct Flag; type Q = Flag; fn f(a: Q) -> Q { !!a }",
        "struct Flag; type Q = Flag; fn g(x: Q) -> Q { x }",
    );
}

#[test]
fn receiver_storage_keeps_boolean_temporaries_distinct() {
    separate(
        "fn f(p: bool) { (!!p).address(); }",
        "fn f(p: bool) { p.address(); }",
    );
    separate(
        "fn f(p: bool) { (p == true).address(); }",
        "fn f(p: bool) { p.address(); }",
    );
    separate(
        "fn f(p: bool) { (if p { true } else { false }).address(); }",
        "fn f(p: bool) { p.address(); }",
    );
}

#[test]
fn value_branches_fold_only_literal_boolean_tails() {
    same(
        "fn f(p: bool) -> bool { if p { true } else { false } }",
        "fn g(x: bool) -> bool { x }",
    );
    same(
        "fn f(p: bool) -> bool { if p { false } else { true } }",
        "fn g(x: bool) -> bool { !x }",
    );
}

#[test]
fn value_branch_guards_keep_condition_and_branch_boundaries() {
    separate(
        "fn f(p: bool) -> bool { if p { let x = true; x } else { false } }",
        "fn g(x: bool) -> bool { x }",
    );
    separate(
        "fn f(p: bool, q: bool) -> bool { if p { true } else { q } }",
        "fn g(x: bool, y: bool) -> bool { x }",
    );
    separate(
        "fn f(p: bool) -> u32 { if p { 1u32 } else { 2u32 } }",
        "fn g(x: bool) -> u32 { x }",
    );
    separate(
        "fn f(p: &bool) -> bool { if *p { true } else { false } }",
        "fn g(x: &bool) -> bool { *x }",
    );
    separate(
        "fn f(p: bool) { let r = &if p { true } else { false }; consume(r); }",
        "fn g(x: bool) { let r = &x; consume(r); }",
    );
}

#[test]
fn demorgan_pushes_into_ordered_operands_of_both_kinds() {
    same(
        "fn f(a: bool, b: bool) -> bool { !(a && b) }",
        "fn g(x: bool, y: bool) -> bool { !x || !y }",
    );
    same(
        "fn f(a: bool, b: bool) -> bool { !(a || b) }",
        "fn g(x: bool, y: bool) -> bool { !x && !y }",
    );
    same(
        "fn f(a: bool, b: bool, c: bool) -> bool { !((a || b) && c) }",
        "fn g(x: bool, y: bool, z: bool) -> bool { ((!x && !y) || !z) }",
    );
    same(
        "fn f(a: bool, b: bool) -> bool { (!! (a || b)) == true }",
        "fn g(x: bool, y: bool) -> bool { x || y }",
    );
}

#[test]
fn demorgan_guards_keep_order_structure_and_operand_proof() {
    separate(
        "fn f(a: bool, b: bool) -> bool { !(a && b) }",
        "fn f(a: bool, b: bool) -> bool { !(b && a) }",
    );
    separate(
        "fn f(a: bool, b: bool, c: bool) -> bool { !(a || (b && c)) }",
        "fn f(a: bool, b: bool, c: bool) -> bool { !a || (!b || !c) }",
    );
    separate(
        "fn f(a: bool, b: bool) -> bool { !(a && b) }",
        "fn g(x: bool, y: bool) -> bool { x || y }",
    );
    separate(
        "fn f<T>(a: T, b: T) -> T { !(a && b) }",
        "fn g<T>(x: T, y: T) -> T { !x || !y }",
    );
}

#[test]
fn compare_true_removes_literal_equality_in_both_positions() {
    same(
        "fn f(p: bool) -> bool { p == true }",
        "fn g(x: bool) -> bool { x }",
    );
    same(
        "fn f(p: bool) -> bool { true == p }",
        "fn g(x: bool) -> bool { x }",
    );
    same(
        "fn f(p: bool, a: u32) -> (bool, u32) { (p == true, a & 0u32) }",
        "fn g(x: bool, b: u32) -> (bool, u32) { (x, b & 0u32) }",
    );
}

#[test]
fn compare_true_guards_keep_foreign_and_non_literal_equalities() {
    separate(
        "fn f(p: bool) -> bool { p == false }",
        "fn g(x: bool) -> bool { !x }",
    );
    separate(
        "fn f(p: bool) -> bool { p != true }",
        "fn g(x: bool) -> bool { !x }",
    );
    separate(
        "fn f(p: bool, q: bool) -> bool { p == q }",
        "fn g(p: bool, q: bool) -> bool { p }",
    );
    separate(
        "struct Flag; impl PartialEq<bool> for Flag { fn eq(&self, _: &bool) -> bool { true } } fn f(a: Flag) -> bool { a == true }",
        "struct Flag; impl PartialEq<bool> for Flag { fn eq(&self, _: &bool) -> bool { true } } fn g(x: Flag) -> bool { x }",
    );
    separate(
        "fn f<T: PartialEq>(a: T, b: T) -> bool { (a == b) == true }",
        "fn g<T: PartialEq>(x: T, y: T) -> bool { x == y }",
    );
}

#[test]
fn branch_inversion_moves_whole_blocks_by_negation_parity() {
    same(
        "fn f(p: bool) -> u32 { if !!p { 1u32 } else { 2u32 } }",
        "fn g(x: bool) -> u32 { if x { 1u32 } else { 2u32 } }",
    );
    same(
        "fn f(p: bool) -> u32 { if !!!p { 1u32 } else { 2u32 } }",
        "fn g(x: bool) -> u32 { if x { 2u32 } else { 1u32 } }",
    );
    same(
        "fn f(p: bool) -> u32 { if !p { let v = 1u32; v } else { let w = 2u32; w } }",
        "fn g(x: bool) -> u32 { if x { let w = 2u32; w } else { let v = 1u32; v } }",
    );
    same(
        "fn f(p: bool) { if !p { mark(); 1u32; } else { other(); 2u32; } }",
        "fn g(x: bool) { if x { other(); 2u32; } else { mark(); 1u32; } }",
    );
    same(
        "fn f(r#type: bool) -> u32 { if !r#type { 1u32 } else { 2u32 } }",
        "fn g(x: bool) -> u32 { if x { 2u32 } else { 1u32 } }",
    );
}

#[test]
fn branch_inversion_guards_keep_affected_structures_intact() {
    separate(
        "fn f(p: bool, q: bool) -> u32 { if !p { 1u32 } else if q { 2u32 } else { 3u32 } }",
        "fn f(p: bool, q: bool) -> u32 { if p { 3u32 } else if !q { 2u32 } else { 1u32 } }",
    );
    separate(
        "fn f(p: bool) -> u32 { if !check(p) { 1u32 } else { 2u32 } }",
        "fn g(x: bool) -> u32 { if x { 2u32 } else { 1u32 } }",
    );
    separate(
        "#[observe] fn f(p: bool) -> u32 { if !p { 1u32 } else { 2u32 } }",
        "#[observe] fn g(p: bool) -> u32 { if p { 2u32 } else { 1u32 } }",
    );
    separate(
        "fn f(p: bool) -> u32 { if !p { #[cfg(all())] 1u32 } else { 2u32 } }",
        "fn g(x: bool) -> u32 { if x { 2u32 } else { #[cfg(all())] 1u32 } }",
    );
}

#[test]
fn literal_condition_selects_independently_typed_tails() {
    same(
        "fn f(a: u32, b: u32) -> u32 { if true { a } else { b } }",
        "fn g(a: u32, b: u32) -> u32 { a }",
    );
    same(
        "fn f() -> u32 { if false { 5u32 } else { 6u32 } }",
        "fn g() -> u32 { 6u32 }",
    );
    same(
        "fn f() -> u8 { if true { 1u8 } else { 2u8 } }",
        "fn g() -> u8 { 1u8 }",
    );
}

#[test]
fn literal_condition_guards_keep_discarded_constraints_and_later_batches() {
    separate(
        "fn f() -> u32 { if true { mark(); 1u32 } else { 2u32 } }",
        "fn g() -> u32 { mark(); 1u32 }",
    );
    separate(
        "fn f() -> u32 { if 1u32 == 2u32 { 3u32 } else { 4u32 } }",
        "fn g() -> u32 { 4u32 }",
    );
    separate(
        "fn f() -> u32 { if true { let v = 1u32; v } else { 2u32 } }",
        "fn g() -> u32 { 1u32 }",
    );
    separate(
        "fn f() -> u32 { if true { 1u32 } else if false { 2u32 } else { 3u32 } }",
        "fn g() -> u32 { 1u32 }",
    );
    separate(
        "fn f(a: u32, b: u32) -> &u32 { if true { &a } else { &b } }",
        "fn g(a: u32, b: u32) -> &u32 { &a }",
    );
    separate(
        "fn f() { let n = if true { 0 } else { 0u8 }; consume(&n); }",
        "fn g() { let n = 0; consume(&n); }",
    );
}

#[test]
fn combined_folds_retain_m1_m2_and_m3_laws() {
    same(
        "fn f(p: bool) -> (bool, bool) { let t = !!p; (t, t) }",
        "fn g(x: bool) -> (bool, bool) { let u = x; (u, u) }",
    );
    same(
        "fn f(p: bool, a: u32) -> (bool, u32) { (!!p, a | (1u32 & 2u32)) }",
        "fn g(x: bool, b: u32) -> (bool, u32) { (x, b | 0u32) }",
    );
    same(
        "fn f(a: bool, b: bool) -> bool { !(a && (b | b)) }",
        "fn g(x: bool, y: bool) -> bool { !x || !y }",
    );
    same(
        "fn f(p: bool, a: u32) -> (bool, u32) { (if p { true } else { false }, a & (1u32 & 2u32)) }",
        "fn g(x: bool, b: u32) -> (bool, u32) { (x, b & 0u32) }",
    );
}

#[test]
fn inherited_short_circuit_and_temporary_negatives_keep_both_modes() {
    for case in support::cases().iter().filter(|case| {
        case.expected != "same"
            && case.family == "negative"
            && [
                "N12_short_circuit",
                "N14_temporary_drop_scope",
                "N15_discarded_branch_inference",
            ]
            .contains(&case.id.as_str())
    }) {
        separate(&case.a, &case.b);
    }
}

#[test]
fn boolean_future_regions_preserve_exact_sharing_and_ordered_roots() {
    let original = "fn f(p:bool,q:bool)->(bool,bool){let a=!!p;let b=p==true;println!(\"barrier\");let c=!(a&&q);let d=if b{true}else{false};(c,d)}";
    for declarations in ["let r#type=p;let r#match=p;", "let r#match=p;let r#type=p;"] {
        for future in [
            "let c=!r#type||!q;let d=r#match;",
            "let d=r#match;let c=!r#type||!q;",
        ] {
            same(
                original,
                &format!(
                    "fn f(p:bool,q:bool)->(bool,bool){{{declarations}println!(\"barrier\");{future}(c,d)}}"
                ),
            );
        }
    }
    separate(
        original,
        "fn f(p:bool,q:bool)->(bool,bool){let a=p;let b=p;println!(\"barrier\");let c=!a||!q;let d=a;(c,d)}",
    );
    separate(
        original,
        "fn f(p:bool,q:bool)->(bool,bool){let a=p;let b=p;println!(\"barrier\");let c=!a||!q;let d=b;(d,c)}",
    );
}
