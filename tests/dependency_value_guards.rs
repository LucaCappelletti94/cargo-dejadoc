//! Public guards for the value folds' rejected consumers.

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
fn storage_observations_keep_the_aliased_and_temporary_producers() {
    separate(
        "fn f(input: u32) -> bool { let a = input & 15u32; let b = a; std::ptr::eq(&a, &b) }",
        "fn g(input: u32) -> bool { let a = input & 15u32; std::ptr::eq(&a, &a) }",
    );
    separate(
        "fn f(input: u32) -> u32 { let mask = input & 15u32; let seen = &mask; *seen; mask ^ 3u32 }",
        "fn g(input: u32) -> u32 { (input & 15u32) ^ 3u32 }",
    );
    separate(
        "fn f(input: u32) -> u32 { let mask = input & 15u32; let seen = &mut mask; *seen = 9u32; mask ^ 3u32 }",
        "fn g(input: u32) -> u32 { (input & 15u32) ^ 3u32 }",
    );
}

#[test]
fn reassigned_sources_keep_the_alias_declaration() {
    separate(
        "fn f() -> u32 { let mut a = 1u32; let b = a; a = 2u32; b }",
        "fn g() -> u32 { let mut a = 1u32; a = 2u32; 1u32 }",
    );
    separate(
        "fn f() -> u32 { let mut a = 1u32; let b = a; a += 3u32; b }",
        "fn g() -> u32 { let mut a = 1u32; a += 3u32; 1u32 }",
    );
}

#[test]
fn captures_and_borrowed_temps_keep_the_producer_declaration() {
    separate(
        "fn f(input: u32) -> u32 { let mask = input & 15u32; let held = || mask; held }",
        "fn g(input: u32) -> u32 { let held = || input & 15u32; held }",
    );
    separate(
        "fn f(input: u32) -> u32 { let mask = input & 15u32; let looped = (|| { let inner = mask; inner })(); looped }",
        "fn g(input: u32) -> u32 { input & 15u32 }",
    );
}

#[test]
fn format_reads_keep_the_referenced_bindings() {
    separate(
        "fn f(input: u32) -> u32 { let width = 4usize; let mask = input & 15u32; println!(\"{mask:>width$}\"); mask ^ 3u32 }",
        "fn g(input: u32) -> u32 { let width = 4usize; println!(\"{:>width$}\", input & 15u32, width = width); (input & 15u32) ^ 3u32 }",
    );
    separate(
        "fn f() -> u32 { let width = 8usize; let zero = 1u32; println!(\"{zero:width$}\"); zero }",
        "fn g() -> u32 { let zero = 1u32; println!(\"{zero:>8}\"); zero }",
    );
}

#[test]
fn unknown_macro_and_procedural_observers_keep_the_candidates() {
    separate(
        "fn f(input: u32) -> u32 { observe!(); let mask = input & 15u32; mask ^ 3u32 }",
        "fn g(input: u32) -> u32 { (input & 15u32) ^ 3u32 }",
    );
    separate(
        "fn f(input: u32) -> u32 { let mask = input & 15u32; opaque!(mask); mask ^ 3u32 }",
        "fn g(input: u32) -> u32 { (input & 15u32) ^ 3u32 }",
    );
    separate(
        "fn f() -> u32 { let unused = 17u32; opaque!(unused); 3u32 }",
        "fn g() -> u32 { 3u32 }",
    );
}

#[test]
fn tuple_modes_and_element_positions_keep_the_tuple_declaration() {
    separate(
        "fn f(input: u32) -> (u32, u32) { let (ref a, b) = (input & 15u32, input >> 4u32); (*a, b) }",
        "fn g(input: u32) -> (u32, u32) { let a = input & 15u32; let b = input >> 4u32; (a, b) }",
    );
    separate(
        "fn f(input: u32) -> (u32, u32) { let (a @ x, b) = (input & 15u32, input >> 4u32); (x, b) }",
        "fn g(input: u32) -> (u32, u32) { let a = input & 15u32; let b = input >> 4u32; (a, b) }",
    );
    separate(
        "fn f(input: u32) -> u32 { let (a, _) = (input & 15u32, input >> 4u32); a ^ 3u32 }",
        "fn g(input: u32) -> u32 { (input & 15u32) ^ 3u32 }",
    );
    separate(
        "fn next() -> u32 { 7u32 } fn f(input: u32) -> (u32, u32) { let (a, b) = (input & 15u32, next()); (a, b) }",
        "fn next() -> u32 { 7u32 } fn g(input: u32) -> (u32, u32) { let a = input & 15u32; let b = next(); (a, b) }",
    );
}

#[test]
fn effectful_panic_initializers_and_owned_values_keep_the_declarations() {
    separate(
        "fn f(input: u32, cell: [u32; 4]) -> u32 { let probe = cell[input]; probe ^ 3u32 }",
        "fn g(input: u32, cell: [u32; 4]) -> u32 { cell[input] ^ 3u32 }",
    );
    separate(
        "fn f() -> u32 { let probe = 1u32 / 0u32; probe }",
        "fn g() -> u32 { 1u32 / 0u32 }",
    );
    separate(
        "fn f() -> u32 { struct D; impl Drop for D { fn drop(&mut self) {} } let unused = D; 1u32 }",
        "fn g() -> u32 { struct D; impl Drop for D { fn drop(&mut self) {} } 1u32 }",
    );
    separate(
        "fn f() -> u32 { struct Owned; let unused = Owned; 1u32 }",
        "fn g() -> u32 { struct Owned; 1u32 }",
    );
}

#[test]
fn duplicate_evaluation_of_unknown_producers_keeps_the_call_shape() {
    separate(
        "fn next() -> u32 { 1u32 } fn f() -> (u32, u32) { let a = next(); (a, a) }",
        "fn next() -> u32 { 1u32 } fn g() -> (u32, u32) { (next(), next()) }",
    );
    separate(
        "fn next() -> u32 { 1u32 } fn f() -> u32 { let a = next(); let b = a; b ^ 3u32 }",
        "fn next() -> u32 { 1u32 } fn g() -> u32 { let a = next(); a ^ 3u32 }",
    );
    separate(
        "fn next() -> u32 { 1u32 } fn f() -> (u32, u32) { let a = next(); (a ^ 1u32, a | 2u32) }",
        "fn next() -> u32 { 1u32 } fn g() -> (u32, u32) { (next() ^ 1u32, next() | 2u32) }",
    );
}

#[test]
fn value_fold_anchors_match_the_approved_domains() {
    same(
        "fn f(input: u32) -> u32 { let mask = input & 15u32; let copied = mask; copied ^ 3u32 }",
        "fn g(input: u32) -> u32 { let mask = input & 15u32; mask ^ 3u32 }",
    );
    same(
        "fn f(input: u32) -> u32 { let mask = input & 15u32; mask ^ 3u32 }",
        "fn g(input: u32) -> u32 { (input & 15u32) ^ 3u32 }",
    );
    same(
        "fn f(input: u32) -> (u32, u32) { let mask = input & 15u32; (mask ^ 3u32, mask | 4u32) }",
        "fn g(input: u32) -> (u32, u32) { ((input & 15u32) ^ 3u32, (input & 15u32) | 4u32) }",
    );
    same(
        "fn f(input: u32) -> (u32, u32) { let (a, b) = (input & 15u32, input >> 4u32); (a, b) }",
        "fn g(input: u32) -> (u32, u32) { let a = input & 15u32; let b = input >> 4u32; (a, b) }",
    );
    same(
        "fn f(input: u32) -> u32 { let unused = 17u32; input }",
        "fn g(input: u32) -> u32 { input }",
    );
}
