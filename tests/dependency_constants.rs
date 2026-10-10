//! Checked fixed-width constants through public comparison.

#[path = "support/dependency.rs"]
mod support;

fn form(source: &str) -> syn_canon::CanonicalForm {
    syn_canon::canonicalize(syn::parse_str(source).unwrap())
}

fn failing(source: &str) -> syn_canon::CanonicalForm {
    syn_canon::canonicalize_failing(syn::parse_str(source).unwrap())
}

fn plus_one(value: &str) -> String {
    if value.starts_with('-') {
        (value.parse::<i128>().unwrap() + 1).to_string()
    } else {
        (value.parse::<u128>().unwrap() + 1).to_string()
    }
}

fn fn_ref<'a>(item: &'a syn::Item, name: &str) -> Option<(&'a syn::Signature, &'a syn::Block)> {
    match item {
        syn::Item::Fn(function) if function.sig.ident == name => {
            Some((&function.sig, &function.block))
        }
        syn::Item::Mod(module) => module
            .content
            .as_ref()
            .and_then(|(_, items)| items.iter().find_map(|item| fn_ref(item, name))),
        syn::Item::Impl(impl_) => impl_.items.iter().find_map(|member| match member {
            syn::ImplItem::Fn(method) if method.sig.ident == name => {
                Some((&method.sig, &method.block))
            }
            _ => None,
        }),
        _ => None,
    }
}

fn context_form(file: &syn::File, name: &str) -> syn_canon::CanonicalForm {
    let context = syn_canon::SourceContext::new(core::iter::once((&[][..], file)));
    let (sig, block) = file
        .items
        .iter()
        .find_map(|item| fn_ref(item, name))
        .unwrap();
    context.function(sig, block).unwrap().canonicalize()
}

const WIDTHS: &[&str] = &[
    "i8", "i16", "i32", "i64", "i128", "u8", "u16", "u32", "u64", "u128",
];

const OPERATORS: &[(&str, &str, &str, &str)] = &[
    ("+", "3", "4", "7"),
    ("-", "9", "4", "5"),
    ("*", "3", "4", "12"),
    ("/", "12", "4", "3"),
    ("%", "13", "4", "1"),
    ("&", "10", "6", "2"),
    ("|", "10", "6", "14"),
    ("^", "10", "6", "12"),
    ("<<", "1", "4", "16"),
    (">>", "64", "3", "8"),
];

const UNARY_NOT: &[(&str, &str)] = &[
    ("i8", "-6"),
    ("i16", "-6"),
    ("i32", "-6"),
    ("i64", "-6"),
    ("i128", "-6"),
    ("u8", "250"),
    ("u16", "65530"),
    ("u32", "4294967290"),
    ("u64", "18446744073709551610"),
    ("u128", "340282366920938463463374607431768211450"),
];

const OVERFLOW_PAIRS: &[(&str, &str, &str)] = &[
    ("u8", "200", "100"),
    ("u16", "60000", "10000"),
    ("u32", "3000000000", "2000000000"),
    ("u64", "9500000000000000000", "9500000000000000000"),
    (
        "u128",
        "170141183460469231731687303715884105728",
        "170141183460469231731687303715884105728",
    ),
    ("i8", "127", "1"),
    ("i16", "32767", "1"),
    ("i32", "2147483647", "1"),
    ("i64", "9223372036854775807", "1"),
    ("i128", "170141183460469231731687303715884105727", "1"),
];

const SIGNED_MINIMA: &[(&str, &str, &str)] = &[
    ("i8", "64", "127"),
    ("i16", "16384", "32767"),
    ("i32", "1073741824", "2147483647"),
    ("i64", "4611686018427387904", "9223372036854775807"),
    (
        "i128",
        "85070591730234615865843651857942052864",
        "170141183460469231731687303715884105727",
    ),
];

#[test]
fn constant_fixed_width_expressions() {
    let case = support::cases()
        .iter()
        .find(|case| case.id == "A09_constant_eval")
        .unwrap();
    assert_eq!(case.family, "constant");
    assert_eq!(case.expected, "same");
    assert_ne!(failing(&case.a), failing(&case.b));
    assert_eq!(form(&case.a), form(&case.b));
}

#[test]
fn constant_failing_mode_preserves_every_literal_distinction() {
    assert_ne!(
        failing("fn f() -> u32 { 2u32 + 3u32 }"),
        failing("fn f() -> u32 { 5u32 }")
    );
    assert_ne!(
        failing("fn f() -> u32 { 4u32 / 2u32 }"),
        failing("fn f() -> u32 { 4u32 - 2u32 }")
    );
    assert_ne!(
        failing("const N: u32 = 2 + 3;"),
        failing("const N: u32 = 5u32;")
    );
    assert_ne!(
        failing("fn f() -> u32 { let n: u32 = 2 + 3; n }"),
        failing("fn f() -> u32 { let n: u32 = 5u32; n }")
    );
    assert_ne!(
        failing("fn f() -> u32 { 1u32 / 0u32 }"),
        failing("fn f() -> u32 { 0u32 }")
    );
    assert_ne!(
        failing("fn f() -> i8 { ((0i8 - 127i8) - 1i8) / -1i8 }"),
        failing("fn f() -> i8 { 127i8 }")
    );
}

#[test]
fn constant_intermediate_overflow_retains_the_whole_tree() {
    for (width, x, y) in OVERFLOW_PAIRS {
        let tree = format!("fn f() -> {width} {{ ({x}{width} + {y}{width}) - {y}{width} }}");
        let literal = format!("fn f() -> {width} {{ {x}{width} }}");
        assert_ne!(form(&tree), form(&literal), "overflow retained {width}");
        assert_ne!(
            failing(&tree),
            failing(&literal),
            "failing overflow {width}"
        );
    }
    assert_ne!(
        form("fn f() -> u8 { 255u8 + 0u8 }"),
        form("fn f() -> u8 { ((255u8 + 1u8) - 1u8) }")
    );
}

#[test]
fn constant_zero_divisors_remain_retained() {
    for divisor in ["/", "%"] {
        assert_ne!(
            form(&format!("fn f() -> u32 {{ 1u32 {divisor} 0u32 }}")),
            form("fn f() -> u32 { 0u32 }"),
            "zero divisor {divisor}"
        );
    }
    assert_ne!(
        form("fn f() -> u32 { 0u32 / 0u32 }"),
        form("fn f() -> u32 { 0u32 }")
    );
    assert_ne!(
        form("fn f() -> u32 { 0u32 % 0u32 }"),
        form("fn f() -> u32 { 0u32 }")
    );
}

#[test]
fn constant_invalid_negation_and_minimum_division_remain_retained() {
    for (width, half, max) in SIGNED_MINIMA {
        let invalid =
            format!("fn f() -> {width} {{ -((0{width} - {half}{width}) - {half}{width}) }}");
        let minimum = format!("fn f() -> {width} {{ 0{width} - {max}{width} - 1{width} }}");
        assert_ne!(form(&invalid), form(&minimum), "invalid negation {width}");
        assert_ne!(
            failing(&invalid),
            failing(&minimum),
            "failing invalid negation {width}"
        );
        let alternate = format!("fn f() -> {width} {{ -(0{width} - {max}{width} - 1{width}) }}");
        assert_ne!(
            form(&invalid),
            form(&alternate),
            "whole tree retention {width}"
        );
        // MIN / -1 overflows even where the remainder fits.
        let division = format!(
            "fn f() -> {width} {{ ((0{width} - {half}{width}) - {half}{width}) / -1{width} }}"
        );
        assert_ne!(form(&division), form(&minimum), "minimum division {width}");
    }
}

#[test]
fn constant_suffix_mismatches_and_unproven_types_remain_retained() {
    let mixed = "fn f() -> u32 { 3u8 + 4u16 }";
    assert_ne!(form(mixed), form("fn f() -> u32 { 7u8 }"));
    assert_ne!(form(mixed), form("fn f() -> u32 { 7u16 }"));
    assert_ne!(form(mixed), form("fn f() -> u32 { 7u32 }"));
    assert_ne!(failing(mixed), failing("fn f() -> u32 { 7u32 }"));
    assert_ne!(
        form("fn f() -> u32 { 1u32 << 4 }"),
        form("fn f() -> u32 { 16u32 }")
    );
    assert_ne!(
        form("fn f() -> u32 { 1u32 << 32u32 }"),
        form("fn f() -> u32 { 0u32 }")
    );
    assert_ne!(
        form("fn f() -> u32 { 1u32 << -1i32 }"),
        form("fn f() -> u32 { 1u32 }")
    );
    // Return types give no inference to the tail tree.
    assert_ne!(
        form("fn f() -> u32 { 2 + 3 }"),
        form("fn f() -> u32 { 5u32 }")
    );
    assert_ne!(
        failing("fn f() -> u32 { 2 + 3 }"),
        failing("fn f() -> u32 { 5u32 }")
    );
    // Pointer-sized widths have no target-independent proof.
    assert_ne!(
        form("fn f() -> usize { 2usize + 3usize }"),
        form("fn f() -> usize { 5usize }")
    );
    assert_ne!(
        failing("fn f() -> usize { 2usize + 3usize }"),
        failing("fn f() -> usize { 5usize }")
    );
    assert_ne!(
        form("fn f() -> isize { 2isize + 3isize }"),
        form("fn f() -> isize { 5isize }")
    );
    // Array lengths keep the usize constraint, no fixed-width proof.
    assert_ne!(
        form("fn f() { let a = [0u8; 2 + 3]; a }"),
        form("fn f() { let a = [0u8; 5]; a }")
    );
}

#[test]
fn constant_macro_payloads_never_fold() {
    let payload = "fn f() { m!(2u32 + 3u32); }";
    let literal = "fn f() { m!(5u32); }";
    assert_ne!(form(payload), form(literal));
    assert_ne!(failing(payload), failing(literal));
}

#[test]
fn constant_observer_ancestry_keeps_literal_spellings_distinct() {
    let nested = "macro_rules! observer { () => {} } fn f() -> u32 { let n = 2u32 + 3u32; { observer!(); } n }";
    let literal =
        "macro_rules! observer { () => {} } fn f() -> u32 { let n = 5u32; { observer!(); } n }";
    assert_ne!(form(nested), form(literal));
    assert_ne!(failing(nested), failing(literal));
    let shadowed =
        "mod plugin; use plugin::assert; fn f() -> u32 { let n = 2u32 + 3u32; assert!(true); n }";
    let shadowed_literal =
        "mod plugin; use plugin::assert; fn f() -> u32 { let n = 5u32; assert!(true); n }";
    assert_ne!(form(shadowed), form(shadowed_literal));
    let observed: syn::File = syn::parse_str(nested).unwrap();
    let plain: syn::File = syn::parse_str("fn f() -> u32 { let n = 2u32 + 3u32; n }").unwrap();
    let plain_literal: syn::File = syn::parse_str("fn f() -> u32 { let n = 5u32; n }").unwrap();
    assert_ne!(context_form(&observed, "f"), context_form(&plain, "f"));
    assert_ne!(
        context_form(&observed, "f"),
        context_form(&plain_literal, "f")
    );
    let attributed = "fn f() -> u32 { #[cfg(unix)] let n = 2u32 + 3u32; n }";
    let attributed_literal = "fn f() -> u32 { #[cfg(unix)] let n = 5u32; n }";
    assert_ne!(form(attributed), form(attributed_literal));
    assert_ne!(failing(attributed), failing(attributed_literal));
}

#[test]
fn ancestor_live_attributes_disable_constant_folds() {
    let anchors = [
        "#![observer::inspect]\nfn left() -> u32 { let n = 2u32 + 3u32; n }\nfn right() -> u32 { let n = 5u32; n }",
        "#[observer::inspect]\nmod observed {\n    fn left() -> u32 { let n = 2u32 + 3u32; n }\n    fn right() -> u32 { let n = 5u32; n }\n}",
        "struct Bag;\n#[observer::inspect]\nimpl Bag {\n    fn left() -> u32 { let n = 2u32 + 3u32; n }\n    fn right() -> u32 { let n = 5u32; n }\n}",
    ];
    for anchor in anchors {
        let file: syn::File = syn::parse_str(anchor).unwrap();
        assert_ne!(
            context_form(&file, "left"),
            context_form(&file, "right"),
            "{anchor} anchor"
        );
    }
    let file: syn::File = syn::parse_str(
        "#[observer::inspect]\ntrait observed {\n    fn left() -> u32 { let n = 2u32 + 3u32; n }\n    fn right() -> u32 { let n = 5u32; n }\n}",
    )
    .unwrap();
    let syn::Item::Trait(trait_) = &file.items[0] else {
        unreachable!()
    };
    let context = syn_canon::SourceContext::new(core::iter::once((&[][..], &file)));
    let forms: Vec<_> = trait_
        .items
        .iter()
        .filter_map(|item| match item {
            syn::TraitItem::Fn(method) => Some(
                context
                    .function(&method.sig, method.default.as_ref().unwrap())
                    .unwrap()
                    .canonicalize(),
            ),
            _ => None,
        })
        .collect();
    assert_ne!(forms[0], forms[1]);
}

#[test]
fn constant_alias_and_shadow_separations() {
    assert_ne!(
        form("type Word = u32; fn f() -> u32 { let n: Word = 2u32 + 3; n }"),
        form("type Word = u64; fn f() -> u32 { let n: Word = 2u32 + 3; n }")
    );
    assert_ne!(
        form("mod plugin; use plugin::Word; fn f() -> u32 { let n: Word = 2u32 + 3; n }"),
        form("type Word = u32; fn f() -> u32 { let n: Word = 2u32 + 3; n }")
    );
    let glob = "mod plugin; use plugin::*; fn f() -> u32 { let n: u32 = 2 + 3; n }";
    assert_ne!(form(glob), form("fn f() -> u32 { let n: u32 = 2 + 3; n }"));
    assert_ne!(
        form(glob),
        form("mod plugin; use plugin::Word; fn f() -> u32 { let n: u32 = 2 + 3; n }")
    );
    assert_ne!(
        form("struct u32; fn f() { let n: u32 = 2u32 + 3; }"),
        form("fn f() { let n: u32 = 2u32 + 3; }")
    );
    assert_ne!(
        form("struct u32; fn f() { let n: u32 = 2u32 + 3; }"),
        form("struct u32; fn f() { let n: u32 = 5u32; }")
    );
}

#[test]
fn constant_casts_keep_their_operation_and_constraint() {
    assert_ne!(
        form("fn f() -> u32 { 5u16 as u32 }"),
        form("fn f() -> u32 { 5u32 }")
    );
    assert_ne!(
        failing("fn f() -> u32 { 5u16 as u32 }"),
        failing("fn f() -> u32 { 5u32 }")
    );
    assert_ne!(
        form("fn f() -> u64 { 2u32 + (3u32 as u64) }"),
        form("fn f() -> u64 { 5u64 }")
    );
}

#[test]
fn constant_fold_neighbours_stay_distinct() {
    for width in WIDTHS.iter().copied() {
        for (operator, left, right, result) in OPERATORS.iter().copied() {
            let tree = format!("fn f() -> {width} {{ {left}{width} {operator} {right}{width} }}");
            let neighbour = format!("fn f() -> {width} {{ {}{width} }}", plus_one(result));
            assert_ne!(
                form(&tree),
                form(&neighbour),
                "value neighbour {operator} {width}"
            );
        }
        let value = UNARY_NOT.iter().find(|entry| entry.0 == width).unwrap().1;
        let negated = format!("fn f() -> {width} {{ !5{width} }}");
        let neighbour = format!("fn f() -> {width} {{ {}{width} }}", plus_one(value));
        assert_ne!(
            form(&negated),
            form(&neighbour),
            "value neighbour ! {width}"
        );
        let position = WIDTHS.iter().position(|entry| *entry == width).unwrap();
        for other in WIDTHS.iter().copied().skip(position + 1) {
            let narrow = format!("fn f() -> {width} {{ 3{width} + 4{width} }}");
            let wide = format!("fn f() -> {other} {{ 3{other} + 4{other} }}");
            assert_ne!(
                form(&narrow),
                form(&wide),
                "width neighbour {width} {other}"
            );
        }
    }
}

#[test]
fn constant_folds_check_every_operator_at_every_width() {
    for width in WIDTHS.iter().copied() {
        for (operator, left, right, result) in OPERATORS {
            let tree = format!("fn f() -> {width} {{ {left}{width} {operator} {right}{width} }}");
            let literal = format!("fn f() -> {width} {{ {result}{width} }}");
            assert_eq!(form(&tree), form(&literal), "fold {operator} {width}");
            assert_ne!(
                failing(&tree),
                failing(&literal),
                "failing {operator} {width}"
            );
        }
        let value = UNARY_NOT.iter().find(|entry| entry.0 == width).unwrap().1;
        let negated = format!("fn f() -> {width} {{ !5{width} }}");
        let literal = format!("fn f() -> {width} {{ {value}{width} }}");
        assert_eq!(form(&negated), form(&literal), "fold ! {width}");
        assert_ne!(failing(&negated), failing(&literal), "failing ! {width}");
    }
    assert_ne!(
        form("fn f() -> u32 { -0u32 }"),
        form("fn f() -> u32 { 0u32 }")
    );
    assert_ne!(
        failing("fn f() -> u32 { -0u32 }"),
        failing("fn f() -> u32 { 0u32 }")
    );
}

#[test]
fn constant_minimum_magnitude_chains_fold() {
    for (width, half, max) in SIGNED_MINIMA {
        let chain_a = format!("fn f() -> {width} {{ 0{width} - {half}{width} - {half}{width} }}");
        let chain_b = format!("fn f() -> {width} {{ 0{width} - {max}{width} - 1{width} }}");
        assert_eq!(form(&chain_a), form(&chain_b), "minimum {width}");
        assert_ne!(
            failing(&chain_a),
            failing(&chain_b),
            "failing minimum {width}"
        );
        let magnitude = max.parse::<u128>().unwrap() + 1;
        let minimum = format!("fn f() -> {width} {{ -{magnitude}{width} }}");
        assert_eq!(form(&chain_a), form(&minimum), "minimum magnitude {width}");
        assert_ne!(failing(&chain_a), failing(&minimum));
        let positive = format!("fn f() -> {width} {{ {magnitude}{width} }}");
        assert_ne!(
            form(&positive),
            form(&minimum),
            "positive magnitude {width}"
        );
        let excessive = max.parse::<u128>().unwrap() + 2;
        let invalid_magnitude = format!("fn f() -> {width} {{ -{excessive}{width} }}");
        let maximum = format!("fn f() -> {width} {{ {max}{width} }}");
        assert_ne!(form(&invalid_magnitude), form(&maximum));
        assert_ne!(failing(&invalid_magnitude), failing(&maximum));
        let invalid = format!("fn f() -> {width} {{ -(-{magnitude}{width}) }}");
        assert_ne!(form(&invalid), form(&minimum), "minimum negation {width}");
        let remainder = format!(
            "fn f() -> {width} {{ ((0{width} - {half}{width}) - {half}{width}) % -1{width} }}"
        );
        let zero = format!("fn f() -> {width} {{ 0{width} }}");
        assert_ne!(form(&remainder), form(&zero), "minimum remainder {width}");
        assert_ne!(
            failing(&remainder),
            failing(&zero),
            "failing minimum remainder {width}"
        );
    }
}

#[test]
fn constant_declaration_types_fold_unsuffixed_constants() {
    assert_eq!(
        form("fn f() -> u32 { let n: u32 = 2 + 3; n }"),
        form("fn f() -> u32 { let n: u32 = 5u32; n }")
    );
    assert_eq!(
        form("fn f() -> u32 { const N: u32 = 2 + 3; N }"),
        form("fn f() -> u32 { const N: u32 = 5u32; N }")
    );
    assert_eq!(form("const N: u32 = 2 + 3;"), form("const N: u32 = 5u32;"));
    assert_ne!(
        failing("fn f() -> u32 { let n: u32 = 2 + 3; n }"),
        failing("fn f() -> u32 { let n: u32 = 5u32; n }")
    );
    assert_ne!(
        failing("const N: u32 = 2 + 3;"),
        failing("const N: u32 = 5u32;")
    );
    let source: syn::File = syn::parse_str(
        "
        type Word = u32;
        fn original() -> u32 { let n: Word = 2 + 3; n }
        fn folded() -> u32 { let n: Word = 5u32; n }
        ",
    )
    .unwrap();
    assert_eq!(
        context_form(&source, "original"),
        context_form(&source, "folded")
    );
}

#[test]
fn constant_suffix_propagation_folds_proven_trees() {
    assert_eq!(
        form("fn f() -> u32 { 2u32 + 3 }"),
        form("fn f() -> u32 { 5u32 }")
    );
    assert_eq!(
        form("fn f() -> u32 { 3 + 2u32 }"),
        form("fn f() -> u32 { 5u32 }")
    );
    assert_ne!(
        failing("fn f() -> u32 { 2u32 + 3 }"),
        failing("fn f() -> u32 { 5u32 }")
    );
    assert_eq!(
        form("fn f() -> u32 { 1u32 << 4u64 }"),
        form("fn f() -> u32 { 16u32 }")
    );
    assert_ne!(
        failing("fn f() -> u32 { 1u32 << 4u64 }"),
        failing("fn f() -> u32 { 16u32 }")
    );
}

#[test]
fn constant_folds_compose_with_dependency_scheduling() {
    let original =
        "fn f(input: u32) -> u32 { let n = 2u32 + 3u32; let q = 10u32 ^ 6u32; input ^ (n | q) }";
    let scheduled = "fn f(input: u32) -> u32 { let q = 12u32; let n = 5u32; input ^ (n | q) }";
    assert_eq!(form(original), form(scheduled));
    assert_ne!(failing(original), failing(scheduled));
    let changed = "fn f(input: u32) -> u32 { let q = 12u32; let n = 6u32; input ^ (n | q) }";
    assert_ne!(form(scheduled), form(changed));
}

#[test]
fn constant_observer_freezes_only_disable_the_fold_under_them() {
    let plain = "fn f() -> u32 { let n = 2u32 + 3u32; n }";
    let literal = "fn f() -> u32 { let n = 5u32; n }";
    assert_eq!(form(plain), form(literal));
    let source: syn::File = syn::parse_str(
        "
        fn original() -> u32 { let n = 2u32 + 3u32; n }
        fn folded() -> u32 { let n = 5u32; n }
        ",
    )
    .unwrap();
    assert_eq!(
        context_form(&source, "original"),
        context_form(&source, "folded")
    );
}

fn grouped_form(source: &str) -> syn_canon::CanonicalForm {
    struct Groups;

    impl syn::visit_mut::VisitMut for Groups {
        fn visit_expr_mut(&mut self, expr: &mut syn::Expr) {
            syn::visit_mut::visit_expr_mut(self, expr);
            let inner =
                core::mem::replace(expr, syn::Expr::Verbatim(proc_macro2::TokenStream::new()));
            *expr = syn::Expr::Group(syn::ExprGroup {
                attrs: Vec::new(),
                group_token: syn::token::Group::default(),
                expr: Box::new(inner),
            });
        }
    }

    let mut file: syn::File = syn::parse_str(source).unwrap();
    syn::visit_mut::VisitMut::visit_file_mut(&mut Groups, &mut file);
    syn_canon::canonicalize(file)
}

#[test]
fn delimiterless_constant_groups_preserve_typed_results() {
    for width in WIDTHS {
        for (operator, left, right, result) in OPERATORS {
            let source = format!("fn f() -> {width} {{ {left}{width} {operator} {right}{width} }}");
            let expected = format!("fn f() -> {width} {{ {result}{width} }}");
            let neighbour = format!("fn f() -> {width} {{ {}{width} }}", plus_one(result));
            assert_eq!(grouped_form(&source), form(&expected));
            assert_ne!(grouped_form(&source), form(&neighbour));
        }
    }
    for (width, _, maximum) in SIGNED_MINIMA {
        let magnitude = maximum.parse::<u128>().unwrap() + 1;
        let minimum = format!("fn f() -> {width} {{ -{magnitude}{width} }}");
        assert_eq!(grouped_form(&minimum), form(&minimum));
    }
}

#[test]
fn compound_constant_negation_preserves_signed_results() {
    for width in ["i8", "i16", "i32", "i64", "i128"] {
        for (source, result) in [
            (format!("-(2{width} + 3{width})"), "-5"),
            (format!("-(2{width} - 8{width})"), "6"),
            (format!("-const {{ 2{width} + 3{width} }}"), "-5"),
        ] {
            let source = format!("fn f() -> {width} {{ {source} }}");
            let expected = format!("fn f() -> {width} {{ {result}{width} }}");
            assert_eq!(form(&source), form(&expected));
            assert_eq!(grouped_form(&source), form(&expected));
            assert_ne!(failing(&source), failing(&expected));
        }
    }
}

#[test]
fn comparison_operands_fold_without_boolean_evaluation() {
    for operator in ["==", "!=", "<", "<=", ">", ">="] {
        let source = format!("fn f() -> bool {{ (2u32 + 3u32) {operator} 5u32 }}");
        let expected = format!("fn f() -> bool {{ 5u32 {operator} 5u32 }}");
        assert_eq!(form(&source), form(&expected));
        assert_ne!(form(&source), form("fn f() -> bool { true }"));
        assert_ne!(form(&source), form("fn f() -> bool { false }"));
    }
}
