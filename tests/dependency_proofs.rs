//! Conservative primitive facts in original function contexts.

use syn_canon::SourceContext;

fn function<'a>(file: &'a syn::File, name: &str) -> &'a syn::ItemFn {
    file.items
        .iter()
        .find_map(|item| find_fn(item, name))
        .unwrap()
}

#[test]
fn identical_alias_spelling_retains_resolved_operation_width() {
    let body = "fn f(input: Word) -> (Word, Word) {
        let low = input & 15; let high = input >> 4; (low, high)
    }";
    let narrow: syn::File = syn::parse_str(&format!("type Word = u32; {body}")).unwrap();
    let wide: syn::File = syn::parse_str(&format!("type Word = u64; {body}")).unwrap();
    let narrow_context = SourceContext::new(core::iter::once((&[][..], &narrow)));
    let wide_context = SourceContext::new(core::iter::once((&[][..], &wide)));
    let narrow = function(&narrow, "f");
    let wide = function(&wide, "f");
    assert_ne!(
        narrow_context
            .function(&narrow.sig, &narrow.block)
            .unwrap()
            .canonicalize(),
        wide_context
            .function(&wide.sig, &wide.block)
            .unwrap()
            .canonicalize(),
    );
}

#[test]
fn identical_alias_spelling_retains_resolved_tail_type() {
    let narrow: syn::File =
        syn::parse_str("type Word = u32; fn f(input: Word) -> Word { input }").unwrap();
    let wide: syn::File =
        syn::parse_str("type Word = u64; fn f(input: Word) -> Word { input }").unwrap();
    let narrow_context = SourceContext::new(core::iter::once((&[][..], &narrow)));
    let wide_context = SourceContext::new(core::iter::once((&[][..], &wide)));
    let narrow = function(&narrow, "f");
    let wide = function(&wide, "f");
    assert_ne!(
        narrow_context
            .function(&narrow.sig, &narrow.block)
            .unwrap()
            .canonicalize(),
        wide_context
            .function(&wide.sig, &wide.block)
            .unwrap()
            .canonicalize(),
    );
}

#[test]
fn imported_alias_sharing_a_value_name_retains_its_operation_width() {
    let source = |width| {
        syn::parse_str::<syn::File>(&format!(
            "mod definitions {{ pub type Word = u{width}; pub fn Word() -> bool {{ true }} }}
            use definitions::Word;
            fn f(input: Word) -> (Word, Word) {{
                let low = input & 15; let high = input >> 4; (low, high)
            }}"
        ))
        .unwrap()
    };
    let narrow = source(32);
    let wide = source(64);
    let narrow_context = SourceContext::new([(&[][..], &narrow)]);
    let wide_context = SourceContext::new([(&[][..], &wide)]);
    let narrow = function(&narrow, "f");
    let wide = function(&wide, "f");
    assert_ne!(
        narrow_context
            .function(&narrow.sig, &narrow.block)
            .unwrap()
            .canonicalize(),
        wide_context
            .function(&wide.sig, &wide.block)
            .unwrap()
            .canonicalize(),
    );
}

fn alias_width_pair_inequal(widths: [u32; 2], body: &str) {
    let source =
        |width| syn::parse_str::<syn::File>(&format!("type Word = u{width}; {body}")).unwrap();
    let narrow = source(widths[0]);
    let wide = source(widths[1]);
    let narrow_context = SourceContext::new([(&[][..], &narrow)]);
    let wide_context = SourceContext::new([(&[][..], &wide)]);
    let narrow = function(&narrow, "f");
    let wide = function(&wide, "f");
    assert_ne!(
        narrow_context
            .function(&narrow.sig, &narrow.block)
            .unwrap()
            .canonicalize(),
        wide_context
            .function(&wide.sig, &wide.block)
            .unwrap()
            .canonicalize(),
    );
}

fn renamed_pair_equal(a: &str, b: &str) {
    let a: syn::File = syn::parse_str(a).unwrap();
    let b: syn::File = syn::parse_str(b).unwrap();
    let a_context = SourceContext::new(core::iter::once((&[][..], &a)));
    let b_context = SourceContext::new(core::iter::once((&[][..], &b)));
    let fa = function(&a, "f");
    let fb = function(&b, "f");
    assert_eq!(
        a_context
            .function(&fa.sig, &fa.block)
            .unwrap()
            .canonicalize(),
        b_context
            .function(&fb.sig, &fb.block)
            .unwrap()
            .canonicalize(),
    );
}

fn find_fn<'a>(item: &'a syn::Item, name: &str) -> Option<&'a syn::ItemFn> {
    match item {
        syn::Item::Fn(function) if function.sig.ident == name => Some(function),
        syn::Item::Mod(mod_) => {
            let content = mod_.content.as_ref()?;
            content.1.iter().find_map(|inner| find_fn(inner, name))
        }
        _ => None,
    }
}

#[test]
fn identical_alias_spelling_retains_xor_and_shift_width() {
    alias_width_pair_inequal(
        [32, 64],
        "fn f(input: Word) -> Word { let low = input ^ 15; low << 4 }",
    );
}

#[test]
fn identical_alias_spelling_retains_shift_overflow_distinction() {
    alias_width_pair_inequal(
        [8, 16],
        "#[expect(arithmetic_overflow, reason = \"The narrow alias overflows\")] fn f(input: Word) -> Word { let low = input << 8; low }",
    );
}

#[test]
fn renamed_dead_storage_observer_keeps_identical_form() {
    renamed_pair_equal(
        "type Word = u32;
        fn f(input: Word) -> Word {
            let low = input & 15;
            return low;
            low.to_be_bytes();
        }",
        "type Word = u32;
        fn f(input: Word) -> Word {
            let high = input & 15;
            return high;
            high.to_be_bytes();
        }",
    );
}

#[test]
fn renamed_unreachable_tail_keeps_identical_form() {
    renamed_pair_equal(
        "type Word = u32;
        fn f(input: Word) -> Word {
            let low = input & 15;
            return low;
            low
        }",
        "type Word = u32;
        fn f(input: Word) -> Word {
            let high = input & 15;
            return high;
            high
        }",
    );
}

#[test]
fn renamed_total_tail_keeps_identical_form() {
    renamed_pair_equal(
        "type Word = u32;
        fn f(input: Word) -> Word {
            let low = input & 15;
            low ^ 7
        }",
        "type Word = u32;
        fn f(input: Word) -> Word {
            let high = input & 15;
            high ^ 7
        }",
    );
}

#[test]
fn renamed_let_else_keeps_identical_form() {
    renamed_pair_equal(
        "type Word = u32;
        fn f(input: Word) -> Word {
            let v = input & 15 else { return 0; };
            v
        }",
        "type Word = u32;
        fn f(input: Word) -> Word {
            let w = input & 15 else { return 0; };
            w
        }",
    );
}

#[test]
fn renamed_pattern_macro_observer_keeps_identical_form() {
    renamed_pair_equal(
        "type Word = u32;
        fn f(input: Word) -> Word {
            let _ = matches!(input, _);
            input
        }",
        "type Word = u32;
        fn f(value: Word) -> Word {
            let _ = matches!(value, _);
            value
        }",
    );
}

#[test]
fn renamed_dbg_observer_keeps_identical_form() {
    renamed_pair_equal(
        "type Word = u32;
        fn f(input: Word) -> Word {
            dbg!(input);
            input
        }",
        "type Word = u32;
        fn f(value: Word) -> Word {
            dbg!(value);
            value
        }",
    );
}

#[test]
fn renamed_long_macro_path_keeps_identical_form() {
    let a: syn::File = syn::parse_str(
        "mod a {
            pub mod b {
                pub mod c {
                    macro_rules! probe { ($value:expr) => { let _ = $value; } }
                    pub(crate) use probe;
                    fn f(input: u32) -> u32 {
                        crate::a::b::c::probe!(input);
                        input
                    }
                }
            }
        }",
    )
    .unwrap();
    let b: syn::File = syn::parse_str(
        "mod a {
            pub mod b {
                pub mod c {
                    macro_rules! probe { ($value:expr) => { let _ = $value; } }
                    pub(crate) use probe;
                    fn f(value: u32) -> u32 {
                        crate::a::b::c::probe!(value);
                        value
                    }
                }
            }
        }",
    )
    .unwrap();
    let a_context = SourceContext::new(core::iter::once((&[][..], &a)));
    let b_context = SourceContext::new(core::iter::once((&[][..], &b)));
    let fa = function(&a, "f");
    let fb = function(&b, "f");
    assert_eq!(
        a_context
            .function(&fa.sig, &fa.block)
            .unwrap()
            .canonicalize(),
        b_context
            .function(&fb.sig, &fb.block)
            .unwrap()
            .canonicalize(),
    );
}

#[test]
fn local_shift_traits_preserve_binding_identity() {
    renamed_pair_equal(
        "struct Amount;
         impl core::ops::Shl<Amount> for bool {
             type Output = bool;
             fn shl(self, _: Amount) -> bool { !self }
         }
         fn f(input: bool, amount: Amount) -> bool {
             let shifted = input << amount;
             shifted
         }",
        "struct Amount;
         impl core::ops::Shl<Amount> for bool {
             type Output = bool;
             fn shl(self, _: Amount) -> bool { !self }
         }
         fn f(value: bool, count: Amount) -> bool {
             let result = value << count;
             result
         }",
    );
}

#[test]
fn nested_mutable_shadows_preserve_binding_identity() {
    renamed_pair_equal(
        "fn f(input: u32) -> u32 {
             let value = input & 15;
             if input != 0 {
                 let mut value = input ^ 3;
                 value ^= 1;
                 value
             } else { value }
         }",
        "fn f(source: u32) -> u32 {
             let outer = source & 15;
             if source != 0 {
                 let mut inner = source ^ 3;
                 inner ^= 1;
                 inner
             } else { outer }
         }",
    );
}

#[test]
fn nested_let_else_preserves_binding_identity() {
    renamed_pair_equal(
        "fn f(input: u32) -> u32 {
             if input != 0 {
                 let value = input else { return 0; };
                 value
             } else { 1 }
         }",
        "fn f(source: u32) -> u32 {
             if source != 0 {
                 let result = source else { return 0; };
                 result
             } else { 1 }
         }",
    );
}

#[test]
fn captured_macro_patterns_preserve_outer_binding_identity() {
    renamed_pair_equal(
        "fn f(input: u32) -> bool { matches!(input, captured @ 0..=7) }",
        "fn f(value: u32) -> bool { matches!(value, captured @ 0..=7) }",
    );
}
