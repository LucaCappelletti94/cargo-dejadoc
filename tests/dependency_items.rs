//! Closed item ordering and compatible inherent partitions.

#[path = "support/function.rs"]
mod functions;
#[path = "support/method_form.rs"]
mod method_form;

use functions::function;
use method_form::method_form;

#[derive(serde::Deserialize)]
struct Pair {
    id: String,
    a: String,
    b: String,
}

fn original(id: &str) -> Pair {
    serde_json::from_str::<Vec<Pair>>(include_str!("fixtures/dependency/cases.json"))
        .unwrap()
        .into_iter()
        .find(|pair| pair.id == id)
        .unwrap()
}

fn form(source: &str) -> syn_canon::CanonicalForm {
    syn_canon::canonicalize(syn::parse_file(source).unwrap())
}

fn failing(source: &str) -> syn_canon::CanonicalForm {
    syn_canon::canonicalize_failing(syn::parse_file(source).unwrap())
}

#[test]
fn items_independent_order() {
    let pair = original("A13_item_order");
    assert_ne!(failing(&pair.a), failing(&pair.b));
    assert_eq!(form(&pair.a), form(&pair.b));
}

#[test]
fn items_inherent_impl_partition() {
    let pair = original("A14_split_impl");
    assert_ne!(failing(&pair.a), failing(&pair.b));
    assert_eq!(form(&pair.a), form(&pair.b));
}

#[derive(serde::Deserialize)]
struct Ledger {
    id: String,
    family: String,
    a: String,
    b: String,
    expected: String,
}

fn ledger() -> Vec<Ledger> {
    serde_json::from_str(include_str!("fixtures/dependency/cases.json")).unwrap()
}

/// The positive candidates, every `same` pair outside the control families.
fn positive(case: &Ledger) -> bool {
    case.expected == "same" && case.family != "control" && case.family != "m1-control"
}

/// The negative controls, every pair expected to stay separate.
fn negative(case: &Ledger) -> bool {
    case.expected != "same"
}

#[test]
fn ledger_positive_candidates_equal_in_passing_mode() {
    for case in ledger().iter().filter(|case| positive(case)) {
        assert_eq!(form(&case.a), form(&case.b), "{}", case.id);
    }
}

#[test]
fn ledger_positive_candidates_separate_in_failing_mode() {
    for case in ledger().iter().filter(|case| positive(case)) {
        assert_ne!(failing(&case.a), failing(&case.b), "{}", case.id);
    }
}

#[test]
fn ledger_negative_controls_separate_in_both_modes() {
    for case in ledger().iter().filter(|case| negative(case)) {
        assert_ne!(form(&case.a), form(&case.b), "{} passing", case.id);
        assert_ne!(failing(&case.a), failing(&case.b), "{} failing", case.id);
    }
}

#[test]
fn ledger_alpha_control_c01_equal_in_both_modes() {
    let c01 = ledger()
        .into_iter()
        .find(|case| case.id == "C01_alpha_control")
        .unwrap();
    assert_eq!(form(&c01.a), form(&c01.b));
    assert_eq!(failing(&c01.a), failing(&c01.b));
}

fn all_permutations(items: Vec<String>) -> Vec<Vec<String>> {
    let mut items = items;
    let mut out = Vec::new();
    permute(&mut items, 0, &mut out);
    out
}

fn permute(items: &mut [String], depth: usize, out: &mut Vec<Vec<String>>) {
    if depth == items.len() {
        out.push(items.to_vec());
        return;
    }
    for index in depth..items.len() {
        items.swap(depth, index);
        permute(items, depth + 1, out);
        items.swap(depth, index);
    }
}

fn permutation_sources(declarations: &[&str]) -> Vec<String> {
    all_permutations(declarations.iter().map(|text| (*text).to_owned()).collect())
        .into_iter()
        .map(|order| order.join(""))
        .collect()
}

fn assert_permutations_fold(declarations: &[&str]) {
    let sources = permutation_sources(declarations);
    let mut forms = Vec::new();
    let mut failing_forms = Vec::new();
    for source in &sources {
        forms.push(form(source));
        failing_forms.push(failing(source));
    }
    let base = &forms[0];
    for (source, form) in sources.iter().zip(&forms).skip(1) {
        assert_eq!(base, form, "permutation: {source}");
    }
    assert!(
        failing_forms.iter().any(|form| form != base),
        "failing mode keeps a permutation distinct: {sources:?}"
    );
}

fn assert_failing_pairwise_distinct(sources: &[String]) {
    let forms: Vec<syn_canon::CanonicalForm> =
        sources.iter().map(|source| failing(source)).collect();
    let distinct: std::collections::HashSet<_> = forms.iter().collect();
    assert_eq!(distinct.len(), forms.len(), "failing forms: {sources:?}");
}

#[test]
fn item_oracle_permutes_a_closed_independent_group() {
    let declarations = [
        "fn first() -> u32 { 1 }",
        "fn second() -> u32 { 2 }",
        "fn main() { println!(\"{:?}\", (first(), second())); }",
    ];
    let sources = permutation_sources(&declarations);
    assert_eq!(sources.len(), 6);
    assert_permutations_fold(&declarations);
    assert_failing_pairwise_distinct(&sources);
}

#[test]
fn item_oracle_permutes_a_recursive_two_node_component() {
    let declarations = [
        "fn even(input: u32) -> u32 { if input == 0 { 1 } else { odd(input - 1) } }",
        "fn odd(input: u32) -> u32 { if input == 0 { 0 } else { even(input - 1) } }",
        "fn main() { println!(\"{:?}\", (even(3u32), odd(3u32))); }",
    ];
    let sources = permutation_sources(&declarations);
    assert_eq!(sources.len(), 6);
    assert_permutations_fold(&declarations);
    assert_failing_pairwise_distinct(&sources);
}

#[test]
fn item_oracle_permutation_composes_with_alpha_rename() {
    let original = [
        "fn alpha() -> u32 { 5 }",
        "fn beta() -> u32 { 6 }",
        "fn main() { println!(\"{:?}\", (alpha(), beta())); }",
    ];
    let renamed = [
        "fn gamma() -> u32 { 5 }",
        "fn delta() -> u32 { 6 }",
        "fn main() { println!(\"{:?}\", (gamma(), delta())); }",
    ];
    let sources: Vec<String> = permutation_sources(&original)
        .into_iter()
        .chain(permutation_sources(&renamed))
        .collect();
    let forms: Vec<syn_canon::CanonicalForm> = sources.iter().map(|source| form(source)).collect();
    let base = &forms[0];
    for (source, form) in sources.iter().zip(&forms).skip(1) {
        assert_eq!(base, form, "source: {source}");
    }
}

#[test]
fn item_oracle_identical_helper_bodies_keep_every_origin() {
    let declarations = [
        "fn left() -> u32 { 1 }",
        "fn right() -> u32 { 1 }",
        "fn main() { println!(\"{:?}\", (left(), right())); }",
    ];
    assert_permutations_fold(&declarations);
}

#[test]
fn item_oracle_output_positions_pick_the_read_origin() {
    let ordered = concat!(
        "fn first() -> u32 { 1 }",
        "fn second() -> u32 { 2 }",
        "fn main() { println!(\"{:?}\", (first(), second())); }"
    );
    let swapped = concat!(
        "fn first() -> u32 { 1 }",
        "fn second() -> u32 { 2 }",
        "fn main() { println!(\"{:?}\", (second(), first())); }"
    );
    assert_ne!(form(ordered), form(swapped));
    assert_ne!(failing(ordered), failing(swapped));
}

#[test]
fn item_oracle_shared_and_distinct_references_separate() {
    let shared = concat!(
        "fn source() -> u32 { 3 }",
        "fn first() -> u32 { source() }",
        "fn second() -> u32 { source() }",
        "fn fixed() -> u32 { 3 }",
        "fn main() { println!(\"{:?}\", (first(), second())); }"
    );
    let distinct = concat!(
        "fn left() -> u32 { 3 }",
        "fn right() -> u32 { 3 }",
        "fn first() -> u32 { left() }",
        "fn second() -> u32 { right() }",
        "fn main() { println!(\"{:?}\", (first(), second())); }"
    );
    assert_ne!(form(shared), form(distinct));
    assert_ne!(failing(shared), failing(distinct));
}

fn folded(left: &str, right: &str) {
    assert_eq!(form(left), form(right), "{left}\n{right}");
    assert_ne!(failing(left), failing(right), "{left}\n{right}");
}

fn distinct(left: &str, right: &str) {
    assert_ne!(form(left), form(right), "{left}\n{right}");
    assert_ne!(failing(left), failing(right), "{left}\n{right}");
}

fn failing_distinct(left: &str, right: &str) {
    assert_ne!(failing(left), failing(right), "{left}\n{right}");
}

#[test]
fn split_impl_blocks_fold_across_member_order() {
    folded(
        "struct S; impl S { fn one() -> u32 { 1 } } impl S { fn two() -> u32 { 2 } }",
        "struct S; impl S { fn two() -> u32 { 2 } } impl S { fn one() -> u32 { 1 } }",
    );
    folded(
        "struct S; impl S { fn one() -> u32 { 1 } } impl S { fn two() -> u32 { 2 } }",
        "struct S; impl S { fn one() -> u32 { 1 } fn two() -> u32 { 2 } }",
    );
    folded(
        "struct S; impl S { fn one() -> u32 { 1 } }",
        "impl S { fn one() -> u32 { 1 } } struct S;",
    );
}

#[test]
fn split_impl_blocks_fold_with_generic_owners() {
    folded(
        "struct Pair<T>(T); impl<T> Pair<T> { fn low() -> u32 { 1 } } impl<T> Pair<T> { fn high() -> u32 { 2 } }",
        "struct Pair<T>(T); impl<T> Pair<T> { fn low() -> u32 { 1 } fn high() -> u32 { 2 } }",
    );
}

#[test]
fn split_impl_keeps_different_owners_separate() {
    let probe =
        "fn main() { println!(\"{:?}\", (Alpha::get(), Alpha::put(), Beta::get(), Beta::put())); }";
    distinct(
        &format!(
            "{}{probe}",
            "struct Alpha; struct Beta; impl Alpha { fn get() -> u32 { 1 } fn put() -> u32 { 2 } } impl Beta { fn get() -> u32 { 3 } fn put() -> u32 { 4 } } "
        ),
        &format!(
            "{}{probe}",
            "struct Alpha; struct Beta; impl Alpha { fn get() -> u32 { 3 } fn put() -> u32 { 4 } } impl Beta { fn get() -> u32 { 1 } fn put() -> u32 { 2 } } "
        ),
    );
}

#[test]
fn split_impl_keeps_generic_environments_separate() {
    distinct(
        "struct Pair<T>(T); impl Pair<u32> { fn one() -> u32 { 1 } }",
        "struct Pair<T>(T); impl Pair<u64> { fn one() -> u32 { 1 } }",
    );
    distinct(
        "struct Pair<T>(T); impl<T> Pair<T> { fn one() -> u32 { 1 } }",
        "struct Pair<T>(T); impl Pair<u32> { fn one() -> u32 { 1 } }",
    );
}

#[test]
fn split_impl_keeps_bounds_separate() {
    distinct(
        "struct Value<T>(T); impl<T: Copy> Value<T> { fn one() -> u32 { 1 } }",
        "struct Value<T>(T); impl<T> Value<T> { fn one() -> u32 { 1 } }",
    );
    distinct(
        "struct Pair<T, U>(T, U); impl<T: Copy, U: Default> Pair<T, U> { fn one() -> u32 { 1 } }",
        "struct Pair<T, U>(T, U); impl<T: Default, U: Copy> Pair<T, U> { fn one() -> u32 { 1 } }",
    );
    distinct(
        "struct Value<T>(T); impl<T: Copy + Default> Value<T> { fn one() -> u32 { 1 } }",
        "struct Value<T>(T); impl<T: Default + Copy> Value<T> { fn one() -> u32 { 1 } }",
    );
}

#[test]
fn split_impl_keeps_member_visibility_separate() {
    distinct(
        "struct S; impl S { fn one() -> u32 { 1 } }",
        "struct S; impl S { pub fn one() -> u32 { 1 } }",
    );
}

#[test]
fn split_impl_keeps_conditional_members_separate() {
    distinct(
        "struct S; impl S { #[cfg(feature = \"one\")] fn one() -> u32 { 1 } }",
        "struct S; impl S { #[cfg(feature = \"two\")] fn one() -> u32 { 1 } }",
    );
}

#[test]
fn split_impl_keeps_duplicate_member_names_separate() {
    distinct(
        "struct S; impl S { fn one() -> u32 { 1 } } impl S { fn one() -> u32 { 2 } }",
        "struct S; impl S { fn one() -> u32 { 1 } } impl S { fn one() -> u32 { 3 } }",
    );
}

#[test]
fn trait_impls_keep_their_original_order() {
    distinct(
        "trait First { fn get(&self) -> u32; } struct S; impl First for S { fn get(&self) -> u32 { 1 } } impl S { fn one() -> u32 { 2 } }",
        "trait First { fn get(&self) -> u32; } struct S; impl S { fn one() -> u32 { 2 } } impl First for S { fn get(&self) -> u32 { 1 } }",
    );
    distinct(
        "trait A { fn a(&self) -> u32; } trait B { fn b(&self) -> u32; } struct S; impl A for S { fn a(&self) -> u32 { 1 } } impl B for S { fn b(&self) -> u32 { 2 } }",
        "trait A { fn a(&self) -> u32; } trait B { fn b(&self) -> u32; } struct S; impl B for S { fn b(&self) -> u32 { 2 } } impl A for S { fn a(&self) -> u32 { 1 } }",
    );
}

#[test]
fn item_order_keeps_public_items_pinned() {
    distinct(
        "pub fn one() -> u32 { 1 } fn two() -> u32 { 2 }",
        "fn two() -> u32 { 2 } pub fn one() -> u32 { 1 }",
    );
}

#[test]
fn public_items_still_normalize_their_bodies() {
    let a = "fn a(x: u32) -> u32 { x & 1u32 }";
    let b = "fn b(x: u32) -> u32 { x >> 1u32 }";
    let body = |first: &str, second: &str| {
        format!("pub fn g(x: u32) -> u32 {{ {first} {second} a(x) + b(x) }}")
    };
    folded(&body(a, b), &body(b, a));
    let nested = |first: &str, second: &str| {
        format!("fn f(x: u32) -> u32 {{ {} g(x) }}", body(first, second))
    };
    folded(&nested(a, b), &nested(b, a));
}

#[test]
fn use_positions_hoist_like_inherited_drift() {
    let inline = "fn one() -> u32 { 1 } use core::cmp::max; fn two() -> u32 { max(2u32, 3u32) }";
    let hoisted = "use core::cmp::max; fn one() -> u32 { 1 } fn two() -> u32 { max(2u32, 3u32) }";
    assert_eq!(form(inline), form(hoisted));
    assert_eq!(failing(inline), failing(hoisted));
}

#[test]
fn item_order_abandons_groups_with_glob_imports() {
    distinct(
        "use core::cmp::*; fn one() -> u32 { 1 } fn two() -> u32 { 2 }",
        "fn two() -> u32 { 2 } fn one() -> u32 { 1 } use core::cmp::*;",
    );
}

#[test]
fn item_order_keeps_configuration_pinned_and_significant() {
    distinct(
        "#[cfg(feature = \"one\")] fn one() -> u32 { 1 } fn two() -> u32 { 2 }",
        "fn two() -> u32 { 2 } #[cfg(feature = \"one\")] fn one() -> u32 { 1 }",
    );
    distinct(
        "#[cfg(feature = \"one\")] fn one() -> u32 { 1 }",
        "#[cfg(feature = \"two\")] fn one() -> u32 { 1 }",
    );
}

#[test]
fn item_order_keeps_observer_attributes_pinned() {
    distinct(
        "#[track_caller] fn one() -> u32 { 1 } fn two() -> u32 { 2 }",
        "fn two() -> u32 { 2 } #[track_caller] fn one() -> u32 { 1 }",
    );
}

#[test]
fn item_order_sequential_macro_visibility_keeps_the_resolution() {
    failing_distinct(
        "macro_rules! m { () => { 1u32 } } fn one() -> u32 { m!() }",
        "fn one() -> u32 { m!() } macro_rules! m { () => { 1u32 } }",
    );
}

#[test]
fn item_order_permutes_raw_identifier_items() {
    let declarations = [
        "fn r#type() -> u32 { 1 }",
        "fn r#match() -> u32 { 2 }",
        "fn main() { println!(\"{:?}\", (r#type(), r#match())); }",
    ];
    let sources = permutation_sources(&declarations);
    assert_eq!(sources.len(), 6);
    assert_permutations_fold(&declarations);
    assert_failing_pairwise_distinct(&sources);
}

#[test]
fn items_compose_with_inherited_folds() {
    folded(
        "struct S; impl S { fn one() -> u32 { 1 } } impl S { fn two() -> u32 { 2 } } fn make() -> u32 { let base = 2u32 + 3u32; let copied = base; copied ^ 1u32 } fn main() { println!(\"{:?}\", (S::one(), S::two(), make())); }",
        "fn main() { println!(\"{:?}\", (S::one(), S::two(), make())); } struct S; fn make() -> u32 { 5u32 ^ 1u32 } impl S { fn one() -> u32 { 1 } fn two() -> u32 { 2 } }",
    );
}

#[test]
fn items_compose_with_named_field_order() {
    folded(
        "struct S { x: u32, y: u32 } impl S { fn one() -> u32 { 1 } } impl S { fn two() -> u32 { 2 } } fn make(input: u32) -> (u32, u32) { let a = input & 1u32; let b = input ^ 1u32; let s = S { x: b, y: a }; (s.x, s.y) } fn main() { println!(\"{:?}\", (S::one(), S::two(), make(5))); }",
        "fn main() { println!(\"{:?}\", (S::one(), S::two(), make(5))); } struct S { x: u32, y: u32 } fn make(input: u32) -> (u32, u32) { let b = input ^ 1u32; let a = input & 1u32; let s = S { y: a, x: b }; (s.x, s.y) } impl S { fn one() -> u32 { 1 } fn two() -> u32 { 2 } }",
    );
    distinct(
        "struct S { x: u32, y: u32 } impl S { fn one() -> u32 { 1 } } fn make(input: u32) -> (u32, u32) { let a = input & 1u32; let b = input ^ 1u32; let s = S { x: a, y: b }; (s.x, s.y) } fn main() { println!(\"{:?}\", (S::one(), make(5))); }",
        "fn main() { println!(\"{:?}\", (S::one(), make(5))); } struct S { x: u32, y: u32 } fn make(input: u32) -> (u32, u32) { let b = input ^ 1u32; let a = input & 1u32; let s = S { y: a, x: b }; (s.x, s.y) } impl S { fn one() -> u32 { 1 } }",
    );
}

#[test]
fn items_compose_with_literal_alternatives() {
    folded(
        "struct S; impl S { fn one(x: u32) -> u32 { match x { 1 | 2 | 3 => 10, _ => 0 } } } impl S { fn two() -> u32 { 2 } } fn main() { println!(\"{:?}\", (S::one(1), S::two())); }",
        "fn main() { println!(\"{:?}\", (S::one(1), S::two())); } struct S; impl S { fn one(x: u32) -> u32 { match x { 3 | 2 | 1 => 10, _ => 0 } } fn two() -> u32 { 2 } }",
    );
    distinct(
        "struct S; impl S { fn one(x: u32) -> u32 { match x { 1 | 2 | 3 => 10, _ => 0 } } } fn main() { println!(\"{:?}\", S::one(1)); }",
        "fn main() { println!(\"{:?}\", S::one(1)); } struct S; impl S { fn one(x: u32) -> u32 { match x { 1 | 2 | 4 => 10, _ => 0 } } }",
    );
}

#[test]
fn items_compose_with_local_pure_helpers() {
    folded(
        "fn low(x: u32) -> u32 { x & 15u32 } struct S; impl S { fn one(input: u32) -> u32 { low(input) ^ 1u32 } } impl S { fn two() -> u32 { 2 } } fn main() { println!(\"{:?}\", (S::one(7), S::two())); }",
        "fn main() { println!(\"{:?}\", (S::one(7), S::two())); } struct S; fn low(x: u32) -> u32 { x & 15u32 } impl S { fn two() -> u32 { 2 } fn one(input: u32) -> u32 { low(input) ^ 1u32 } }",
    );
    distinct(
        "fn low(x: u32) -> u32 { x & 15u32 } struct S; impl S { fn one(input: u32) -> u32 { low(input) ^ 1u32 } } fn main() { println!(\"{:?}\", S::one(7)); }",
        "fn main() { println!(\"{:?}\", S::one(7)); } struct S; fn low(x: u32) -> u32 { x & 7u32 } impl S { fn one(input: u32) -> u32 { low(input) ^ 1u32 } }",
    );
}

#[test]
fn items_compose_with_boolean_laws() {
    folded(
        "fn gate(p: bool) -> bool { !!p } fn other() -> u32 { 1 } fn main() { println!(\"{:?}\", (gate(true), other())); }",
        "fn main() { println!(\"{:?}\", (gate(true), other())); } fn other() -> u32 { 1 } fn gate(p: bool) -> bool { p }",
    );
    distinct(
        "fn gate(p: bool) -> bool { !!p } fn other() -> u32 { 1 } fn main() { println!(\"{:?}\", (gate(true), other())); }",
        "fn main() { println!(\"{:?}\", (gate(true), other())); } fn other() -> u32 { 2 } fn gate(p: bool) -> bool { p }",
    );
}

#[test]
fn items_compose_with_effect_barrier() {
    folded(
        "fn other() -> u32 { 1 } fn f(input: u32) -> u32 { let a = input & 15u32; println!(\"{:?}\", a); let b = input ^ 3u32; a ^ b } fn main() { println!(\"{:?}\", (f(5), other())); }",
        "fn main() { println!(\"{:?}\", (f(5), other())); } fn other() -> u32 { 1 } fn f(input: u32) -> u32 { let a = input & 15u32; println!(\"{:?}\", a); let b = input ^ 3u32; a ^ b }",
    );
    distinct(
        "fn other() -> u32 { 1 } fn f(input: u32) -> u32 { let a = input & 15u32; println!(\"{:?}\", a); let b = input ^ 3u32; a ^ b } fn main() { println!(\"{:?}\", (f(5), other())); }",
        "fn main() { println!(\"{:?}\", (f(5), other())); } fn other() -> u32 { 1 } fn f(input: u32) -> u32 { let b = input ^ 3u32; println!(\"{:?}\", b); let a = input & 15u32; a ^ b }",
    );
}

fn function_form(
    context: &syn_canon::SourceContext<'_>,
    function: &syn::ItemFn,
) -> syn_canon::CanonicalForm {
    context
        .function(&function.sig, &function.block)
        .unwrap()
        .canonicalize()
}

#[test]
fn borrowed_views_match_the_standalone_item_order_fold() {
    let first: syn::File = syn::parse_file(
        "fn one() -> u32 { 1 } fn two() -> u32 { 2 } fn probe() -> (u32, u32) { (one(), two()) }",
    )
    .unwrap();
    let second: syn::File = syn::parse_file(
        "fn two() -> u32 { 2 } fn one() -> u32 { 1 } fn probe() -> (u32, u32) { (one(), two()) }",
    )
    .unwrap();
    let first_context = syn_canon::SourceContext::new(core::iter::once((&[][..], &first)));
    let second_context = syn_canon::SourceContext::new(core::iter::once((&[][..], &second)));
    assert_eq!(
        function_form(&first_context, function(&first, "probe")),
        function_form(&second_context, function(&second, "probe"))
    );
}

#[test]
fn borrowed_views_match_the_standalone_partition_fold() {
    let split: syn::File = syn::parse_file(
        "struct S; impl S { fn one() -> u32 { 1 } } impl S { fn two() -> u32 { 2 } } fn probe() -> (u32, u32) { (S::one(), S::two()) }",
    )
    .unwrap();
    let combined: syn::File = syn::parse_file(
        "struct S; impl S { fn one() -> u32 { 1 } fn two() -> u32 { 2 } } fn probe() -> (u32, u32) { (S::one(), S::two()) }",
    )
    .unwrap();
    let split_context = syn_canon::SourceContext::new(core::iter::once((&[][..], &split)));
    let combined_context = syn_canon::SourceContext::new(core::iter::once((&[][..], &combined)));
    assert_eq!(
        function_form(&split_context, function(&split, "probe")),
        function_form(&combined_context, function(&combined, "probe"))
    );
}

#[test]
fn borrowed_views_fold_local_helper_order() {
    let first: syn::File = syn::parse_file(
        "fn low(input: u32) -> u32 { input & 15u32 } fn high(input: u32) -> u32 { input >> 2u32 } fn probe(input: u32) -> u32 { low(input) ^ high(input) }",
    )
    .unwrap();
    let second: syn::File = syn::parse_file(
        "fn high(input: u32) -> u32 { input >> 2u32 } fn probe(input: u32) -> u32 { low(input) ^ high(input) } fn low(input: u32) -> u32 { input & 15u32 }",
    )
    .unwrap();
    let first_context = syn_canon::SourceContext::new(core::iter::once((&[][..], &first)));
    let second_context = syn_canon::SourceContext::new(core::iter::once((&[][..], &second)));
    assert_eq!(
        function_form(&first_context, function(&first, "probe")),
        function_form(&second_context, function(&second, "probe"))
    );
}

#[test]
fn borrowed_method_views_fold_across_partitions() {
    let split: syn::File = syn::parse_file(
        "struct S; impl S { fn one() -> u32 { 1 } } impl S { fn two() -> u32 { 2 } }",
    )
    .unwrap();
    let combined: syn::File =
        syn::parse_file("struct S; impl S { fn one() -> u32 { 1 } fn two() -> u32 { 2 } }")
            .unwrap();
    let split_context = syn_canon::SourceContext::new(core::iter::once((&[][..], &split)));
    let combined_context = syn_canon::SourceContext::new(core::iter::once((&[][..], &combined)));
    assert_eq!(
        method_form(&split_context, &split, "one"),
        method_form(&combined_context, &combined, "one")
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

fn wiped(file: syn::File) -> syn::File {
    let mut file = file;
    syn::visit_mut::visit_file_mut(&mut SpanWiper, &mut file);
    file
}

#[test]
fn equal_span_constructed_files_fold_like_parsed_files() {
    let left = "struct S; impl S { fn one() -> u32 { 1 } } impl S { fn two() -> u32 { 2 } }";
    let right = "struct S; impl S { fn two() -> u32 { 2 } } impl S { fn one() -> u32 { 1 } }";
    assert_eq!(
        syn_canon::canonicalize(wiped(syn::parse_file(left).unwrap())),
        syn_canon::canonicalize(wiped(syn::parse_file(right).unwrap()))
    );
    assert_ne!(
        syn_canon::canonicalize_failing(wiped(syn::parse_file(left).unwrap())),
        syn_canon::canonicalize_failing(wiped(syn::parse_file(right).unwrap()))
    );
}

struct KeyNodeCounter {
    nodes: usize,
}

impl KeyNodeCounter {
    fn count(file: &syn::File) -> usize {
        let mut counter = KeyNodeCounter { nodes: 0 };
        syn::visit::visit_file(&mut counter, file);
        counter.nodes
    }
}

impl<'ast> syn::visit::Visit<'ast> for KeyNodeCounter {
    fn visit_item(&mut self, item: &'ast syn::Item) {
        self.nodes += 1;
        syn::visit::visit_item(self, item);
    }

    fn visit_item_fn(&mut self, item: &'ast syn::ItemFn) {
        self.nodes += 1;
        syn::visit::visit_item_fn(self, item);
    }

    fn visit_signature(&mut self, signature: &'ast syn::Signature) {
        self.nodes += 1;
        syn::visit::visit_signature(self, signature);
    }

    fn visit_ident(&mut self, ident: &'ast proc_macro2::Ident) {
        self.nodes += 1;
        syn::visit::visit_ident(self, ident);
    }

    fn visit_generics(&mut self, generics: &'ast syn::Generics) {
        self.nodes += 1;
        syn::visit::visit_generics(self, generics);
    }

    fn visit_return_type(&mut self, return_type: &'ast syn::ReturnType) {
        self.nodes += 1;
        syn::visit::visit_return_type(self, return_type);
    }

    fn visit_type(&mut self, ty: &'ast syn::Type) {
        self.nodes += 1;
        syn::visit::visit_type(self, ty);
    }

    fn visit_type_path(&mut self, ty: &'ast syn::TypePath) {
        self.nodes += 1;
        syn::visit::visit_type_path(self, ty);
    }

    fn visit_path(&mut self, path: &'ast syn::Path) {
        self.nodes += 1;
        syn::visit::visit_path(self, path);
    }

    fn visit_path_segment(&mut self, segment: &'ast syn::PathSegment) {
        self.nodes += 1;
        syn::visit::visit_path_segment(self, segment);
    }

    fn visit_block(&mut self, block: &'ast syn::Block) {
        self.nodes += 1;
        syn::visit::visit_block(self, block);
    }

    fn visit_stmt(&mut self, stmt: &'ast syn::Stmt) {
        self.nodes += 1;
        syn::visit::visit_stmt(self, stmt);
    }

    fn visit_expr(&mut self, expr: &'ast syn::Expr) {
        self.nodes += 1;
        syn::visit::visit_expr(self, expr);
    }

    fn visit_expr_lit(&mut self, expr: &'ast syn::ExprLit) {
        self.nodes += 1;
        syn::visit::visit_expr_lit(self, expr);
    }

    fn visit_lit(&mut self, lit: &'ast syn::Lit) {
        self.nodes += 1;
        syn::visit::visit_lit(self, lit);
    }
}

fn literal_item_orders(count: u32, extra_node: bool) -> (String, String) {
    let mut bodies: Vec<String> = (0..count)
        .map(|index| {
            let value = if extra_node && index + 1 == count {
                format!("({index})")
            } else {
                index.to_string()
            };
            format!("fn f{index}() -> u32 {{ {value} }}")
        })
        .collect();
    let original = bodies.join("");
    let last = bodies.len() - 1;
    bodies.swap(last, last - 1);
    (original, bodies.join(""))
}

#[test]
fn item_order_keeps_over_node_budget_groups_atomic() {
    let (original, swapped) = literal_item_orders(4000, false);
    let parsed: syn::File = syn::parse_file(&original).unwrap();
    assert!(KeyNodeCounter::count(&parsed) > 16384);
    distinct(&original, &swapped);
}

#[test]
fn item_order_keeps_at_node_budget_groups_folded() {
    let (original, swapped) = literal_item_orders(1024, false);
    let parsed: syn::File = syn::parse_file(&original).unwrap();
    assert_eq!(KeyNodeCounter::count(&parsed), 16384);
    folded(&original, &swapped);
}

#[test]
fn item_order_keeps_just_past_node_budget_groups_atomic() {
    let (original, swapped) = literal_item_orders(1024, true);
    assert_eq!(
        KeyNodeCounter::count(&syn::parse_file(&original).unwrap()),
        16385
    );
    distinct(&original, &swapped);
}

#[test]
fn item_order_keeps_within_node_budget_groups_folded() {
    let (original, swapped) = literal_item_orders(500, false);
    assert!(KeyNodeCounter::count(&syn::parse_file(&original).unwrap()) < 16384);
    folded(&original, &swapped);
}

#[test]
fn item_order_keeps_over_depth_budget_groups_atomic() {
    // Debug builds of `syn` need more than the default test stack for 64 nested generics.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(120);
    let worker = std::thread::Builder::new()
        .name("deep-generics".to_owned())
        .stack_size(8 * 1024 * 1024)
        .spawn(|| {
            let deep = format!("type Deep = {}u32{};", "Box<".repeat(64), ">".repeat(64));
            let original = format!("{deep}fn one() -> u32 {{ 1 }} fn two() -> u32 {{ 2 }}");
            let swapped = format!("{deep}fn two() -> u32 {{ 2 }} fn one() -> u32 {{ 1 }}");
            distinct(&original, &swapped);
        })
        .expect("the deep-generics thread can start");
    while !worker.is_finished() && std::time::Instant::now() < deadline {
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
    assert!(
        worker.is_finished(),
        "the deep-generics check completes before its deadline"
    );
    worker.join().expect("the deep-generics check passes");
}

#[test]
fn item_order_keeps_at_depth_budget_groups_folded() {
    let deep = format!(
        "type Deep = {}{}u32{};",
        "&'static ".repeat(3),
        "Box<".repeat(9),
        ">".repeat(9)
    );
    let original = format!("{deep}fn one() -> u32 {{ 1 }} fn two() -> u32 {{ 2 }}");
    let swapped = format!("{deep}fn two() -> u32 {{ 2 }} fn one() -> u32 {{ 1 }}");
    folded(&original, &swapped);
}

#[test]
fn item_order_keeps_just_past_depth_budget_groups_atomic() {
    let deep = format!(
        "type Deep = {}{}u32{};",
        "&'static ".repeat(4),
        "Box<".repeat(9),
        ">".repeat(9)
    );
    let original = format!("{deep}fn one() -> u32 {{ 1 }} fn two() -> u32 {{ 2 }}");
    let swapped = format!("{deep}fn two() -> u32 {{ 2 }} fn one() -> u32 {{ 1 }}");
    distinct(&original, &swapped);
}

fn recursive_pairs(count: usize, reordered: bool) -> String {
    let mut declarations: Vec<String> = (0..count * 2)
        .map(|index| format!("fn f{index}() -> u32 {{ f{}() }}", index ^ 1))
        .collect();
    if reordered {
        declarations.swap(1, 2);
    }
    declarations.join("")
}

#[test]
fn item_search_labels_ambiguous_recursive_components() {
    folded(&recursive_pairs(4, false), &recursive_pairs(4, true));
}

#[test]
fn item_search_exhaustion_retains_ambiguous_components_atomically() {
    distinct(&recursive_pairs(5, false), &recursive_pairs(5, true));
}

fn differently_colored_cycles(second_count: usize, reordered: bool) -> String {
    let mut declarations = Vec::new();
    for (prefix, count, value) in [("a", 63usize, 1u32), ("b", second_count, 2u32)] {
        for index in 0..count {
            declarations.push(format!(
                "fn {prefix}{index}(remaining: u32) -> u32 {{ \
                 if remaining == 0 {{ {value}u32 }} \
                 else {{ {prefix}{}(remaining - 1) }} }}",
                (index + 1) % count
            ));
        }
    }
    if reordered {
        declarations.swap(1, 63);
    }
    declarations.join("")
}

#[test]
fn item_search_accepts_exact_state_limit() {
    folded(
        &differently_colored_cycles(64, false),
        &differently_colored_cycles(64, true),
    );
}

#[test]
fn item_search_retains_partition_after_state_limit() {
    distinct(
        &differently_colored_cycles(65, false),
        &differently_colored_cycles(65, true),
    );
}
