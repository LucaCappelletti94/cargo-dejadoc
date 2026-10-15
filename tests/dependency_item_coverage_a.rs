//! Coverage witnesses for the item graph pipeline edges.

fn form(source: &str) -> syn_canon::CanonicalForm {
    syn_canon::canonicalize(syn::parse_file(source).unwrap())
}

fn failing(source: &str) -> syn_canon::CanonicalForm {
    syn_canon::canonicalize_failing(syn::parse_file(source).unwrap())
}

#[test]
fn loop_labels_fold_like_renamed_local_markers() {
    let left =
        "fn one() -> u32 { let mut n = 0u32; 'l: while n < 4 { n += 1; } n } fn two() -> u32 { 1 }";
    let right =
        "fn one() -> u32 { let mut n = 0u32; 'k: while n < 4 { n += 1; } n } fn two() -> u32 { 1 }";
    assert_eq!(form(left), form(right));
    let control =
        "fn one() -> u32 { let mut n = 0u32; 'l: while n < 5 { n += 1; } n } fn two() -> u32 { 1 }";
    assert_ne!(form(left), form(control));
}

#[test]
fn oversized_macro_argument_keeps_the_region_unscheduled() {
    let arguments = core::iter::repeat_n("1", 16_385)
        .collect::<Vec<_>>()
        .join(", ");
    let left = format!(
        "fn one() -> u32 {{ 1 }} fn big() {{ let _ = println!({arguments}); }} fn main() {{ let _ = (one(), big()); }}"
    );
    let right = format!(
        "fn big() {{ let _ = println!({arguments}); }} fn one() -> u32 {{ 1 }} fn main() {{ let _ = (one(), big()); }}"
    );
    assert_ne!(form(&left), form(&right));
    let small_left = "fn one() -> u32 { 1 } fn big() { let _ = println!(1); } fn main() { let _ = (one(), big()); }";
    let small_right = "fn big() { let _ = println!(1); } fn one() -> u32 { 1 } fn main() { let _ = (one(), big()); }";
    assert_eq!(form(small_left), form(small_right));
}

#[test]
fn not_labels_in_context_items_allow_region_scheduling() {
    let left = "fn one() -> u32 { 1 } fn two() -> u32 { 2 } fn main() { let _ = (one(), two()); } trait T { fn m(&self); } fn helper(p: bool) { let x = !p; let _ = x; }";
    let right = "fn two() -> u32 { 2 } fn one() -> u32 { 1 } fn main() { let _ = (one(), two()); } trait T { fn m(&self); } fn helper(p: bool) { let x = !p; let _ = x; }";
    assert_eq!(form(left), form(right));
    assert_ne!(failing(left), failing(right));
}

#[test]
fn ordered_boolean_labels_in_context_items_allow_region_scheduling() {
    let left = "fn one() -> u32 { 1 } fn two() -> u32 { 2 } fn main() { let _ = (one(), two()); } trait T { fn m(&self); } fn helper(p: bool, q: bool) { let x = p && q; let _ = x; }";
    let right = "fn two() -> u32 { 2 } fn one() -> u32 { 1 } fn main() { let _ = (one(), two()); } trait T { fn m(&self); } fn helper(p: bool, q: bool) { let x = p && q; let _ = x; }";
    assert_eq!(form(left), form(right));
    assert_ne!(failing(left), failing(right));
}

#[test]
fn attribute_on_impl_header_keeps_member_scheduling() {
    let left = "struct S; #[doc = \"x\"] impl S { fn one() -> u32 { 1 } fn two() -> u32 { 2 } }";
    let right = "struct S; #[doc = \"x\"] impl S { fn two() -> u32 { 2 } fn one() -> u32 { 1 } }";
    assert_eq!(form(left), form(right));
    assert_ne!(failing(left), failing(right));
}

#[test]
fn lifetime_generics_merge_compatible_impls() {
    let split =
        "struct S; impl<'a> S { fn one() -> u32 { 1 } } impl<'b> S { fn two() -> u32 { 2 } }";
    let combined = "struct S; impl<'c> S { fn one() -> u32 { 1 } fn two() -> u32 { 2 } }";
    assert_eq!(form(split), form(combined));
    assert_ne!(failing(split), failing(combined));
}

#[test]
fn const_generics_merge_compatible_impls() {
    let split = "struct S; impl<const N: usize> S { fn one() -> usize { N } } impl<const M: usize> S { fn two() -> usize { M } }";
    let combined =
        "struct S; impl<const K: usize> S { fn one() -> usize { K } fn two() -> usize { K } }";
    assert_eq!(form(split), form(combined));
    assert_ne!(failing(split), failing(combined));
}

#[test]
fn unclosed_format_captures_follow_their_binding() {
    let left = "fn f() { let x = 1u32; let _ = println!(\"{x\"); } fn g() -> u32 { 1 }";
    let right = "fn f() { let y = 1u32; let _ = println!(\"{y\"); } fn g() -> u32 { 1 }";
    assert_eq!(form(left), form(right));
    let closed = "fn f() { let x = 1u32; let _ = println!(\"{x}\"); } fn g() -> u32 { 1 }";
    assert_ne!(form(left), form(closed));
}

#[test]
fn let_chains_in_format_arguments_fold() {
    let left = "fn f() { let _ = println!(\"{}\", if true && false { 1 } else { 2 }); let _ = println!(\"{}\", if let Some(x) = Some(0u32) && x { 1 } else { 2 }); } fn g() -> u32 { 1 }";
    let right = "fn f() { let _ = println!(\"{}\", if true && false { 1 } else { 2 }); let _ = println!(\"{}\", if let Some(y) = Some(0u32) && y { 1 } else { 2 }); } fn g() -> u32 { 1 }";
    assert_eq!(form(left), form(right));
}

#[test]
fn while_loops_in_format_arguments_fold() {
    let left = "fn f() { let _ = println!(\"{}\", { let mut i = 0u32; while i < 3 { i += 1; } i }); } fn g() -> u32 { 1 }";
    let right = "fn f() { let _ = println!(\"{}\", { let mut j = 0u32; while j < 3 { j += 1; } j }); } fn g() -> u32 { 1 }";
    assert_eq!(form(left), form(right));
}

#[test]
fn for_loops_in_format_arguments_fold() {
    let left = "fn f() { let _ = println!(\"{}\", { let mut s = 0u32; for x in 0..3 { s += x; } s }); } fn g() -> u32 { 1 }";
    let right = "fn f() { let _ = println!(\"{}\", { let mut t = 0u32; for y in 0..3 { t += y; } t }); } fn g() -> u32 { 1 }";
    assert_eq!(form(left), form(right));
}

#[test]
fn local_initializers_in_format_arguments_fold() {
    let left = "fn f() { let _ = println!(\"{}\", { let y = 1u32; y + 1 }); let _ = println!(\"{}\", { let x = Some(0u32) else { return 0; }; x }); } fn g() -> u32 { 1 }";
    let right = "fn f() { let _ = println!(\"{}\", { let z = 1u32; z + 1 }); let _ = println!(\"{}\", { let w = Some(0u32) else { return 0; }; w }); } fn g() -> u32 { 1 }";
    assert_eq!(form(left), form(right));
}

#[test]
fn nested_macro_heads_resolve_to_their_frame_binding() {
    let left = "struct S; fn one() { let _ = println!(\"{}\", { S::m!(1); 1 }); } fn two() -> u32 { 1 } fn main() { let _ = (one(), two()); }";
    let renamed = "struct T; fn one() { let _ = println!(\"{}\", { T::m!(1); 1 }); } fn two() -> u32 { 1 } fn main() { let _ = (one(), two()); }";
    assert_eq!(form(left), form(renamed));
    let dangling = "struct S; fn one() { let _ = println!(\"{}\", { T::m!(1); 1 }); } fn two() -> u32 { 1 } fn main() { let _ = (one(), two()); }";
    assert_ne!(form(left), form(dangling));
}

#[test]
fn name_value_attributes_in_format_arguments_fold() {
    let left = "fn one() -> u32 { 1 } fn f() { let _ = println!(\"{}\", { #[a = 1] fn g() -> u32 { 2 } g() }); }";
    let right = "fn one() -> u32 { 1 } fn f() { let _ = println!(\"{}\", { #[a = 1] fn h() -> u32 { 2 } h() }); }";
    assert_eq!(form(left), form(right));
}

#[test]
fn medium_macro_argument_still_schedules_the_region() {
    let arguments = core::iter::repeat_n("1", 2_000)
        .collect::<Vec<_>>()
        .join(", ");
    let left = format!(
        "fn one() -> u32 {{ 1 }} fn big() {{ let _ = println!({arguments}); }} fn main() {{ let _ = (one(), big()); }}"
    );
    let right = format!(
        "fn big() {{ let _ = println!({arguments}); }} fn one() -> u32 {{ 1 }} fn main() {{ let _ = (one(), big()); }}"
    );
    assert_eq!(form(&left), form(&right));
}

#[test]
fn plain_ifs_in_format_arguments_fold() {
    let left = "fn f() { let _ = println!(\"{}\", { let b = true; if b { 1 } b }); let _ = println!(\"{}\", { if let Some(x) = Some(1u32) && x { 2 } 3 }); }";
    let right = "fn f() { let _ = println!(\"{}\", { let y = true; if y { 1 } y }); let _ = println!(\"{}\", { if let Some(w) = Some(1u32) && w { 2 } 3 }); }";
    assert_eq!(form(left), form(right));
}

#[test]
fn uninitialized_locals_in_format_arguments_fold() {
    let left = "fn f() { let _ = println!(\"{}\", { let b: u32; let _ = b + 1; 3 }); }";
    let right = "fn f() { let _ = println!(\"{}\", { let y: u32; let _ = y + 1; 3 }); }";
    assert_eq!(form(left), form(right));
}

#[test]
fn nested_format_macro_heads_keep_their_known_names() {
    let left = "fn one() -> u32 { let _ = println!(\"{}\", format!(\"{}\", 1)); 1 } fn two() -> u32 { 2 } fn main() { let _ = (one(), two()); }";
    let right = "fn two() -> u32 { 2 } fn one() -> u32 { let _ = println!(\"{}\", format!(\"{}\", 1)); 1 } fn main() { let _ = (one(), two()); }";
    assert_eq!(form(left), form(right));
}
