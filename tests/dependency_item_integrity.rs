//! Original origins across item moves and rejected ancestor groups.

use std::fmt::Write as _;

fn form(source: &str) -> syn_canon::CanonicalForm {
    syn_canon::canonicalize(syn::parse_file(source).unwrap())
}

#[test]
fn coincident_private_helpers_allow_a_consistent_port_bijection() {
    let left = "fn first() -> u32 { 1u32 } fn second() -> u32 { 1u32 } fn main() { let a = first(); let b = second(); println!(\"{:?}\", (a, b)); }";
    let right = "fn first() -> u32 { 1u32 } fn second() -> u32 { 1u32 } fn main() { let b = second(); let a = first(); println!(\"{:?}\", (b, a)); }";
    assert_eq!(form(left), form(right));
}

#[test]
fn local_tied_helpers_follow_ordered_observation_origins() {
    let left = "fn main() { fn first() -> u32 { 1u32 } fn second() -> u32 { 1u32 } println!(\"{:?}\", (first(), second())); }";
    let right = "fn main() { fn second() -> u32 { 1u32 } fn first() -> u32 { 1u32 } println!(\"{:?}\", (first(), second())); }";
    assert_eq!(form(left), form(right));
}

#[test]
fn local_tied_helpers_compose_with_independent_call_scheduling() {
    let left = "fn main() { fn first() -> u32 { 1u32 } fn second() -> u32 { 1u32 } let a = first(); let b = second(); println!(\"{:?}\", (a, b)); }";
    let right = "fn main() { fn second() -> u32 { 1u32 } fn first() -> u32 { 1u32 } let b = second(); let a = first(); println!(\"{:?}\", (a, b)); }";
    assert_eq!(form(left), form(right));
}

#[test]
fn local_tied_helpers_compose_with_total_expression_scheduling() {
    let left = "fn main() { fn first() -> u32 { 1u32 } fn second() -> u32 { 1u32 } let a = first() ^ 3u32; let b = second() ^ 4u32; println!(\"{:?}\", (a, b)); }";
    let right = "fn main() { fn second() -> u32 { 1u32 } fn first() -> u32 { 1u32 } let b = second() ^ 4u32; let a = first() ^ 3u32; println!(\"{:?}\", (a, b)); }";
    assert_eq!(form(left), form(right));
}

#[test]
fn local_tied_helpers_compose_with_parameter_copies() {
    let copied = "fn run(input: u32) { fn first(value: u32) -> u32 { value } fn second(value: u32) -> u32 { value } let copy = input; let a = first(copy); let b = second(input); println!(\"{:?}\", (a, b)); }";
    let direct = "fn run(input: u32) { fn second(value: u32) -> u32 { value } fn first(value: u32) -> u32 { value } let b = second(input); let a = first(input); println!(\"{:?}\", (a, b)); }";
    assert_eq!(form(copied), form(direct));
}

#[test]
fn split_generic_blocks_rebind_only_the_original_impl_ports() {
    let split = "struct Pair<T>(T); impl<T: Copy> Pair<T> { fn one(value: T) -> T { value } } impl<U: Copy> Pair<U> { fn two<V: Copy>(value: U, other: V) -> U { let _ = other; value } }";
    let combined = "struct Pair<T>(T); impl<T: Copy> Pair<T> { fn one(value: T) -> T { value } fn two<V: Copy>(value: T, other: V) -> T { let _ = other; value } }";
    assert_eq!(form(split), form(combined));
    let shadowed = "struct Pair<T>(T); impl<T: Copy> Pair<T> { fn one(value: T) -> T { value } } impl<U: Copy> Pair<U> { fn two<U: Copy>(value: U, other: U) -> U { let _ = other; value } }";
    assert_ne!(form(split), form(shadowed));
}

#[test]
fn compatible_blocks_preserve_public_member_visibility() {
    let split = "struct Owner; impl Owner { pub fn one() -> u32 { 1u32 } } impl Owner { pub fn two() -> u32 { 2u32 } }";
    let combined =
        "struct Owner; impl Owner { pub fn two() -> u32 { 2u32 } pub fn one() -> u32 { 1u32 } }";
    assert_eq!(form(split), form(combined));
}

#[test]
fn duplicate_member_fallback_keeps_nested_item_groups_opaque() {
    let left = "struct Owner; impl Owner { fn one() -> u32 { fn first() -> u32 { 1u32 } fn second() -> u32 { 2u32 } first() } } impl Owner { fn one() -> u32 { 3u32 } }";
    let right = "struct Owner; impl Owner { fn one() -> u32 { fn second() -> u32 { 2u32 } fn first() -> u32 { 1u32 } first() } } impl Owner { fn one() -> u32 { 3u32 } }";
    assert_ne!(form(left), form(right));
}

#[test]
fn exhausted_ancestor_search_keeps_nested_item_groups_opaque() {
    let mut shared = String::new();
    for index in 0..5 {
        write!(shared, "fn a{index}(n: u32) -> u32 {{ if n == 0 {{ 1u32 }} else {{ b{index}(n - 1) }} }} fn b{index}(n: u32) -> u32 {{ if n == 0 {{ 1u32 }} else {{ a{index}(n - 1) }} }} ").unwrap();
    }
    let left = format!(
        "{shared} fn container() -> u32 {{ fn first() -> u32 {{ 10u32 }} fn second() -> u32 {{ 20u32 }} first() }}"
    );
    let right = format!(
        "{shared} fn container() -> u32 {{ fn second() -> u32 {{ 20u32 }} fn first() -> u32 {{ 10u32 }} first() }}"
    );
    assert_ne!(form(&left), form(&right));
}
