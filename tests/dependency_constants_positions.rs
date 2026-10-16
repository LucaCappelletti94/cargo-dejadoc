//! Fixed-width constant boundaries and contextual positions.

fn form(source: &str) -> syn_canon::CanonicalForm {
    syn_canon::canonicalize(syn::parse_str(source).unwrap())
}

fn same(left: &str, right: &str) {
    assert_eq!(form(left), form(right));
    assert_ne!(
        syn_canon::canonicalize_failing(syn::parse_str(left).unwrap()),
        syn_canon::canonicalize_failing(syn::parse_str(right).unwrap())
    );
}

#[test]
fn fixed_width_operations_fold_inside_const_and_array_constraints() {
    for (left, right) in [
        ("const N: u32 = 2u32 + 3u32;", "const N: u32 = 5u32;"),
        (
            "fn f() -> u32 { const { 2u32 + 3u32 } }",
            "fn f() -> u32 { 5u32 }",
        ),
        (
            "fn f() -> [u8; (2u32 + 3u32) as usize] { [0; (2u32 + 3u32) as usize] }",
            "fn f() -> [u8; 5u32 as usize] { [0; 5u32 as usize] }",
        ),
        (
            "fn f() -> [u8; const { (2u32 + 3u32) as usize }] { [0; const { (2u32 + 3u32) as usize }] }",
            "fn f() -> [u8; const { 5u32 as usize }] { [0; const { 5u32 as usize }] }",
        ),
        (
            "struct Owner<const N: u32 = { 2u32 + 3u32 }>;",
            "struct Owner<const N: u32 = { 5u32 }>;",
        ),
        (
            "struct Owner<const N: u32>; fn f() -> Owner<{ 2u32 + 3u32 }> { Owner }",
            "struct Owner<const N: u32>; fn f() -> Owner<5u32> { Owner }",
        ),
        (
            "trait Owner { const N: u32 = 2 + 3; }",
            "trait Owner { const N: u32 = 5u32; }",
        ),
        (
            "struct Owner; impl Owner { const N: u32 = 2 + 3; }",
            "struct Owner; impl Owner { const N: u32 = 5u32; }",
        ),
        (
            "fn f() -> u32 { { type Word = u32; let n: Word = 2 + 3; n } }",
            "fn f() -> u32 { { type Word = u32; let n: Word = 5u32; n } }",
        ),
    ] {
        same(left, right);
    }
}

#[test]
fn unresolved_array_and_const_widths_keep_their_constraints() {
    for (left, right) in [
        (
            "fn f() -> [u8; 2 + 3] { [0; 2 + 3] }",
            "fn f() -> [u8; 5] { [0; 5] }",
        ),
        (
            "const N: usize = 2usize + 3usize;",
            "const N: usize = 5usize;",
        ),
        (
            "const N: isize = 2isize + 3isize;",
            "const N: isize = 5isize;",
        ),
        (
            "fn f() -> [u8; (2u32 + 3u16) as usize] { [0; (2u32 + 3u16) as usize] }",
            "fn f() -> [u8; 5u32 as usize] { [0; 5u32 as usize] }",
        ),
        (
            "#[cfg(all())] const N: u32 = 2u32 + 3u32;",
            "const N: u32 = 5u32;",
        ),
        (
            "fn f() { let n: u8 = 2u32 + 3u32; }",
            "fn f() { let n: u8 = 5u32; }",
        ),
        (
            "struct Word; fn f() { let n: Word = 2u32 + 3u32; }",
            "struct Word; fn f() { let n: Word = 5u32; }",
        ),
        ("const N: bool = 2 + 3;", "const N: bool = 5;"),
    ] {
        assert_ne!(form(left), form(right));
    }
}

#[test]
fn checked_subtraction_and_multiplication_keep_bad_intermediates() {
    for width in ["u8", "u16", "u32", "u64", "u128"] {
        let underflow = format!("fn f() -> {width} {{ (0{width} - 1{width}) + 1{width} }}");
        let zero = format!("fn f() -> {width} {{ 0{width} }}");
        assert_ne!(form(&underflow), form(&zero));
    }
    for (width, maximum) in [
        ("i8", "127"),
        ("i16", "32767"),
        ("i32", "2147483647"),
        ("i64", "9223372036854775807"),
        ("i128", "170141183460469231731687303715884105727"),
        ("u8", "255"),
        ("u16", "65535"),
        ("u32", "4294967295"),
        ("u64", "18446744073709551615"),
        ("u128", "340282366920938463463374607431768211455"),
    ] {
        let overflow = format!("fn f() -> {width} {{ ({maximum}{width} * 2{width}) / 2{width} }}");
        let literal = format!("fn f() -> {width} {{ {maximum}{width} }}");
        assert_ne!(form(&overflow), form(&literal));
        same(
            &format!("fn f() -> {width} {{ {maximum}{width} + 0{width} }}"),
            &literal,
        );
    }
}

#[test]
fn signed_shifts_and_truncated_left_shifts_follow_rust_widths() {
    for (width, bits, minimum) in [
        ("i8", 8, "-128"),
        ("i16", 16, "-32768"),
        ("i32", 32, "-2147483648"),
        ("i64", 64, "-9223372036854775808"),
        ("i128", 128, "-170141183460469231731687303715884105728"),
    ] {
        same(
            &format!("fn f() -> {width} {{ -4{width} >> 1u8 }}"),
            &format!("fn f() -> {width} {{ -2{width} }}"),
        );
        same(
            &format!("fn f() -> {width} {{ 1{width} << {}u8 }}", bits - 1),
            &format!("fn f() -> {width} {{ {minimum}{width} }}"),
        );
        same(
            &format!("fn f() -> {width} {{ {minimum}{width} << 1u8 }}"),
            &format!("fn f() -> {width} {{ 0{width} }}"),
        );
    }
    for (width, maximum) in [
        ("u8", "255"),
        ("u16", "65535"),
        ("u32", "4294967295"),
        ("u64", "18446744073709551615"),
        ("u128", "340282366920938463463374607431768211455"),
    ] {
        let result = maximum.parse::<u128>().unwrap() - 1;
        same(
            &format!("fn f() -> {width} {{ {maximum}{width} << 1u8 }}"),
            &format!("fn f() -> {width} {{ {result}{width} }}"),
        );
    }
}

#[test]
fn expression_and_attribute_payloads_remain_observed() {
    for (left, right) in [
        (
            "fn f() -> u32 { #[inspect] (2u32 + 3u32) }",
            "fn f() -> u32 { #[inspect] 5u32 }",
        ),
        (
            "#[inspect(value = 2u32 + 3u32)] fn f() {}",
            "#[inspect(value = 5u32)] fn f() {}",
        ),
        (
            "#[inspect = 2u32 + 3u32] fn f() {}",
            "#[inspect = 5u32] fn f() {}",
        ),
        (
            "#[inspect] impl Owner { fn f() -> u32 { 2u32 + 3u32 } }",
            "#[inspect] impl Owner { fn f() -> u32 { 5u32 } }",
        ),
        (
            "#[inspect] trait Owner { fn f() -> u32 { 2u32 + 3u32 } }",
            "#[inspect] trait Owner { fn f() -> u32 { 5u32 } }",
        ),
    ] {
        assert_ne!(form(left), form(right));
    }
}

#[test]
fn bounded_constant_trees_retain_the_complete_expression_on_exhaustion() {
    let tree = |terms: usize| format!("fn f() -> u32 {{ {} }}", vec!["1u32"; terms].join(" + "));
    same(&tree(64), "fn f() -> u32 { 64u32 }");
    assert_ne!(form(&tree(65)), form("fn f() -> u32 { 65u32 }"));
    assert_ne!(form(&tree(65)), form(&tree(64)));
}
