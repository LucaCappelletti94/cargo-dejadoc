//! Conservative primitive facts in original function contexts.

use core::fmt::Write;
use syn_canon::{CanonicalForm, SourceContext};

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

fn form(source: &str) -> CanonicalForm {
    syn_canon::canonicalize(syn::parse_str(source).unwrap())
}

fn alias_chain(count: usize) -> String {
    let mut out = String::new();
    for hop in 0..count {
        let name = format!("a{hop}");
        let target = if hop + 1 == count {
            "u32".to_owned()
        } else {
            format!("a{}", hop + 1)
        };
        write!(out, "type {name} = {target};").unwrap();
    }
    format!("{out} fn f(x: a0) -> a0 {{ x }}")
}

fn chained_alias_swap_source(count: usize, swapped: bool) -> String {
    let mut out = String::new();
    for hop in 0..count {
        let name = format!("a{hop}");
        let target = if hop + 1 == count {
            "u32".to_owned()
        } else {
            format!("a{}", hop + 1)
        };
        write!(out, "type {name} = {target};").unwrap();
    }
    let body = if swapped {
        "let high = x >> 4; let low = x & 15u32; (low, high)"
    } else {
        "let low = x & 15u32; let high = x >> 4; (low, high)"
    };
    format!("{out} fn f(x: a0) -> (a0, a0) {{ {body} }}")
}

#[test]
fn alias_chains_stop_at_the_sixteen_hop_bound() {
    let long: syn::File = syn::parse_str(&alias_chain(18)).unwrap();
    let bounded: syn::File = syn::parse_str(&alias_chain(16)).unwrap();
    let long_context = SourceContext::new(core::iter::once((&[][..], &long)));
    let bounded_context = SourceContext::new(core::iter::once((&[][..], &bounded)));
    let long_fn = function(&long, "f");
    let bounded_fn = function(&bounded, "f");
    assert_ne!(
        long_context
            .function(&long_fn.sig, &long_fn.block)
            .unwrap()
            .canonicalize(),
        bounded_context
            .function(&bounded_fn.sig, &bounded_fn.block)
            .unwrap()
            .canonicalize(),
    );
}

#[test]
fn seventeen_hop_alias_chains_keep_their_primitive() {
    let chained: syn::File = syn::parse_str(&alias_chain(17)).unwrap();
    let bounded: syn::File = syn::parse_str(&alias_chain(16)).unwrap();
    let chained_context = SourceContext::new(core::iter::once((&[][..], &chained)));
    let bounded_context = SourceContext::new(core::iter::once((&[][..], &bounded)));
    let chained_fn = function(&chained, "f");
    let bounded_fn = function(&bounded, "f");
    assert_eq!(
        chained_context
            .function(&chained_fn.sig, &chained_fn.block)
            .unwrap()
            .canonicalize(),
        bounded_context
            .function(&bounded_fn.sig, &bounded_fn.block)
            .unwrap()
            .canonicalize(),
    );
}

#[test]
fn two_hop_alias_chains_keep_their_primitive() {
    let chained: syn::File =
        syn::parse_str("type a1 = a2; type a2 = u32; fn f(x: a1) -> a1 { x }").unwrap();
    let direct: syn::File = syn::parse_str("type a1 = u32; fn f(x: a1) -> a1 { x }").unwrap();
    let chained_context = SourceContext::new(core::iter::once((&[][..], &chained)));
    let direct_context = SourceContext::new(core::iter::once((&[][..], &direct)));
    let chained_fn = function(&chained, "f");
    let direct_fn = function(&direct, "f");
    assert_eq!(
        chained_context
            .function(&chained_fn.sig, &chained_fn.block)
            .unwrap()
            .canonicalize(),
        direct_context
            .function(&direct_fn.sig, &direct_fn.block)
            .unwrap()
            .canonicalize(),
    );
}

#[test]
fn unrelated_types_do_not_block_alias_primitives() {
    let unrelated: syn::File =
        syn::parse_str("type a1 = u32; struct S; fn f(x: a1) -> a1 { x }").unwrap();
    let plain: syn::File = syn::parse_str("type a1 = u32; fn f(x: a1) -> a1 { x }").unwrap();
    let unrelated_context = SourceContext::new(core::iter::once((&[][..], &unrelated)));
    let plain_context = SourceContext::new(core::iter::once((&[][..], &plain)));
    let unrelated_fn = function(&unrelated, "f");
    let plain_fn = function(&plain, "f");
    assert_eq!(
        unrelated_context
            .function(&unrelated_fn.sig, &unrelated_fn.block)
            .unwrap()
            .canonicalize(),
        plain_context
            .function(&plain_fn.sig, &plain_fn.block)
            .unwrap()
            .canonicalize(),
    );
}

#[test]
fn local_type_shadows_block_alias_primitives() {
    let shadowed: syn::File =
        syn::parse_str("type a1 = u32; struct u32; fn f(x: a1) -> a1 { x }").unwrap();
    let plain: syn::File = syn::parse_str("type a1 = u32; fn f(x: a1) -> a1 { x }").unwrap();
    let shadowed_context = SourceContext::new(core::iter::once((&[][..], &shadowed)));
    let plain_context = SourceContext::new(core::iter::once((&[][..], &plain)));
    let shadowed_fn = function(&shadowed, "f");
    let plain_fn = function(&plain, "f");
    assert_ne!(
        shadowed_context
            .function(&shadowed_fn.sig, &shadowed_fn.block)
            .unwrap()
            .canonicalize(),
        plain_context
            .function(&plain_fn.sig, &plain_fn.block)
            .unwrap()
            .canonicalize(),
    );
}

#[test]
fn raw_alias_targets_keep_their_primitive() {
    let raw: syn::File = syn::parse_str("type a1 = r#u32; fn f(x: a1) -> a1 { x }").unwrap();
    let plain: syn::File = syn::parse_str("type a1 = u32; fn f(x: a1) -> a1 { x }").unwrap();
    let raw_context = SourceContext::new(core::iter::once((&[][..], &raw)));
    let plain_context = SourceContext::new(core::iter::once((&[][..], &plain)));
    let raw_fn = function(&raw, "f");
    let plain_fn = function(&plain, "f");
    assert_eq!(
        raw_context
            .function(&raw_fn.sig, &raw_fn.block)
            .unwrap()
            .canonicalize(),
        plain_context
            .function(&plain_fn.sig, &plain_fn.block)
            .unwrap()
            .canonicalize(),
    );
}

#[test]
fn cfg_attribution_blocks_alias_primitives() {
    let attributed: syn::File =
        syn::parse_str("#[cfg(unix)] type a1 = u32; fn f(x: a1) -> a1 { x }").unwrap();
    let plain: syn::File = syn::parse_str("type a1 = u32; fn f(x: a1) -> a1 { x }").unwrap();
    let attributed_context = SourceContext::new(core::iter::once((&[][..], &attributed)));
    let plain_context = SourceContext::new(core::iter::once((&[][..], &plain)));
    let attributed_fn = function(&attributed, "f");
    let plain_fn = function(&plain, "f");
    assert_ne!(
        attributed_context
            .function(&attributed_fn.sig, &attributed_fn.block)
            .unwrap()
            .canonicalize(),
        plain_context
            .function(&plain_fn.sig, &plain_fn.block)
            .unwrap()
            .canonicalize(),
    );
}

#[test]
fn forward_type_shadows_block_alias_primitives() {
    let plain: syn::File = syn::parse_str("type Word = u32; fn f(x: Word) -> Word { x }").unwrap();
    let plain_context = SourceContext::new(core::iter::once((&[][..], &plain)));
    let plain_fn = function(&plain, "f");
    let plain_form = plain_context
        .function(&plain_fn.sig, &plain_fn.block)
        .unwrap()
        .canonicalize();
    for shadow in ["struct u32;", "enum u32 { V }", "union u32 { v: usize }"] {
        let shadowed: syn::File = syn::parse_str(&format!(
            "{shadow} type Word = u32; fn f(x: Word) -> Word {{ x }}"
        ))
        .unwrap();
        let shadowed_context = SourceContext::new(core::iter::once((&[][..], &shadowed)));
        let shadowed_fn = function(&shadowed, "f");
        assert_ne!(
            shadowed_context
                .function(&shadowed_fn.sig, &shadowed_fn.block)
                .unwrap()
                .canonicalize(),
            plain_form,
        );
    }
}

#[test]
fn unrelated_imports_do_not_block_alias_primitives() {
    let imported: syn::File = syn::parse_str(
        "mod m { pub struct other; } use m::other; type Word = u32; fn f(x: Word) -> Word { x }",
    )
    .unwrap();
    let plain: syn::File = syn::parse_str("type Word = u32; fn f(x: Word) -> Word { x }").unwrap();
    let imported_context = SourceContext::new(core::iter::once((&[][..], &imported)));
    let plain_context = SourceContext::new(core::iter::once((&[][..], &plain)));
    let imported_fn = function(&imported, "f");
    let plain_fn = function(&plain, "f");
    assert_eq!(
        imported_context
            .function(&imported_fn.sig, &imported_fn.block)
            .unwrap()
            .canonicalize(),
        plain_context
            .function(&plain_fn.sig, &plain_fn.block)
            .unwrap()
            .canonicalize(),
    );
}

#[test]
fn imports_claiming_the_target_block_alias_primitives() {
    let plain: syn::File = syn::parse_str("type Word = u32; fn f(x: Word) -> Word { x }").unwrap();
    let plain_context = SourceContext::new(core::iter::once((&[][..], &plain)));
    let plain_fn = function(&plain, "f");
    let plain_form = plain_context
        .function(&plain_fn.sig, &plain_fn.block)
        .unwrap()
        .canonicalize();
    for import in [
        "mod m { pub struct u32; } use m::u32;",
        "mod m { pub struct u32; } use m::{u32};",
        "mod m { pub struct other; } use m::other as u32;",
    ] {
        let claimed: syn::File = syn::parse_str(&format!(
            "{import} type Word = u32; fn f(x: Word) -> Word {{ x }}"
        ))
        .unwrap();
        let claimed_context = SourceContext::new(core::iter::once((&[][..], &claimed)));
        let claimed_fn = function(&claimed, "f");
        assert_ne!(
            claimed_context
                .function(&claimed_fn.sig, &claimed_fn.block)
                .unwrap()
                .canonicalize(),
            plain_form,
        );
    }
}

#[test]
fn self_imports_claim_their_parent_name() {
    let claimed: syn::File =
        syn::parse_str("use std::{self}; use std::primitive::u32; fn f(x: u32) -> u32 { x }")
            .unwrap();
    let plain: syn::File =
        syn::parse_str("use std::primitive::u32; fn f(x: u32) -> u32 { x }").unwrap();
    let claimed_context = SourceContext::new(core::iter::once((&[][..], &claimed)));
    let plain_context = SourceContext::new(core::iter::once((&[][..], &plain)));
    let claimed_fn = function(&claimed, "f");
    let plain_fn = function(&plain, "f");
    assert_ne!(
        claimed_context
            .function(&claimed_fn.sig, &claimed_fn.block)
            .unwrap()
            .canonicalize(),
        plain_context
            .function(&plain_fn.sig, &plain_fn.block)
            .unwrap()
            .canonicalize(),
    );
}

#[test]
fn cfg_attributed_modules_block_primitive_facts() {
    let body = "fn f(input: u32) -> (u32, u32) { let low = input & 15u32; let high = input >> 4; (low, high) }";
    let attributed: syn::File = syn::parse_str(&format!("#[cfg(unix)] mod m {{}} {body}")).unwrap();
    let plain: syn::File = syn::parse_str(body).unwrap();
    let attributed_context = SourceContext::new(core::iter::once((&[][..], &attributed)));
    let plain_context = SourceContext::new(core::iter::once((&[][..], &plain)));
    let attributed_fn = function(&attributed, "f");
    let plain_fn = function(&plain, "f");
    assert_ne!(
        attributed_context
            .function(&attributed_fn.sig, &attributed_fn.block)
            .unwrap()
            .canonicalize(),
        plain_context
            .function(&plain_fn.sig, &plain_fn.block)
            .unwrap()
            .canonicalize(),
    );
}

#[test]
fn extern_crates_block_primitive_facts() {
    let body = "fn f(input: u32) -> (u32, u32) { let low = input & 15u32; let high = input >> 4; (low, high) }";
    let declared: syn::File = syn::parse_str(&format!("extern crate alloc; {body}")).unwrap();
    let plain: syn::File = syn::parse_str(body).unwrap();
    let declared_context = SourceContext::new(core::iter::once((&[][..], &declared)));
    let plain_context = SourceContext::new(core::iter::once((&[][..], &plain)));
    let declared_fn = function(&declared, "f");
    let plain_fn = function(&plain, "f");
    assert_ne!(
        declared_context
            .function(&declared_fn.sig, &declared_fn.block)
            .unwrap()
            .canonicalize(),
        plain_context
            .function(&plain_fn.sig, &plain_fn.block)
            .unwrap()
            .canonicalize(),
    );
}

#[test]
fn grouped_glob_imports_block_primitive_facts() {
    let body = "fn f(input: u32) -> (u32, u32) { let low = input & 15u32; let high = input >> 4; (low, high) }";
    let grouped: syn::File = syn::parse_str(&format!(
        "mod m {{ pub struct a; }} use m::{{a, *}}; {body}"
    ))
    .unwrap();
    let plain: syn::File = syn::parse_str(body).unwrap();
    let grouped_context = SourceContext::new(core::iter::once((&[][..], &grouped)));
    let plain_context = SourceContext::new(core::iter::once((&[][..], &plain)));
    let grouped_fn = function(&grouped, "f");
    let plain_fn = function(&plain, "f");
    assert_ne!(
        grouped_context
            .function(&grouped_fn.sig, &grouped_fn.block)
            .unwrap()
            .canonicalize(),
        plain_context
            .function(&plain_fn.sig, &plain_fn.block)
            .unwrap()
            .canonicalize(),
    );
}

#[test]
fn local_shadows_block_primitive_crate_imports() {
    let shadowed: syn::File =
        syn::parse_str("mod std { pub mod primitive { pub struct u32; } } use std::primitive::u32; fn f(x: u32) -> u32 { x }")
            .unwrap();
    let plain: syn::File =
        syn::parse_str("use std::primitive::u32; fn f(x: u32) -> u32 { x }").unwrap();
    let shadowed_context = SourceContext::new(core::iter::once((&[][..], &shadowed)));
    let plain_context = SourceContext::new(core::iter::once((&[][..], &plain)));
    let shadowed_fn = function(&shadowed, "f");
    let plain_fn = function(&plain, "f");
    assert_ne!(
        shadowed_context
            .function(&shadowed_fn.sig, &shadowed_fn.block)
            .unwrap()
            .canonicalize(),
        plain_context
            .function(&plain_fn.sig, &plain_fn.block)
            .unwrap()
            .canonicalize(),
    );
}

#[test]
fn raw_shadows_block_primitive_crate_imports() {
    let shadowed: syn::File =
        syn::parse_str("mod r#core { pub mod primitive { pub struct u8; } } use core::primitive::u8; fn f(x: u8) -> u8 { x }")
            .unwrap();
    let plain: syn::File =
        syn::parse_str("use core::primitive::u8; fn f(x: u8) -> u8 { x }").unwrap();
    let shadowed_context = SourceContext::new(core::iter::once((&[][..], &shadowed)));
    let plain_context = SourceContext::new(core::iter::once((&[][..], &plain)));
    let shadowed_fn = function(&shadowed, "f");
    let plain_fn = function(&plain, "f");
    assert_ne!(
        shadowed_context
            .function(&shadowed_fn.sig, &shadowed_fn.block)
            .unwrap()
            .canonicalize(),
        plain_context
            .function(&plain_fn.sig, &plain_fn.block)
            .unwrap()
            .canonicalize(),
    );
}

#[test]
fn qself_paths_keep_their_non_primitive_identity() {
    let associated: syn::File = syn::parse_str(
        "trait U { type u32; } struct Wrap; impl U for Wrap { type u32 = core::primitive::u32; } fn f(input: <Wrap as U>::u32) -> (u32, u32) { let low = input & 15u32; let high = input >> 4; (low, high) }",
    )
    .unwrap();
    let plain: syn::File = syn::parse_str(
        "fn f(input: u32) -> (u32, u32) { let low = input & 15u32; let high = input >> 4; (low, high) }",
    )
    .unwrap();
    let associated_context = SourceContext::new(core::iter::once((&[][..], &associated)));
    let plain_context = SourceContext::new(core::iter::once((&[][..], &plain)));
    let associated_fn = function(&associated, "f");
    let plain_fn = function(&plain, "f");
    assert_ne!(
        associated_context
            .function(&associated_fn.sig, &associated_fn.block)
            .unwrap()
            .canonicalize(),
        plain_context
            .function(&plain_fn.sig, &plain_fn.block)
            .unwrap()
            .canonicalize(),
    );
}

#[test]
fn grouped_block_imports_preserve_renamed_bindings_and_targets() {
    let source = "fn f(input: u32) -> u32 { use std::{hint::black_box as observe, primitive::u32 as Word}; observe(input as Word) }";
    let renamed = source
        .replace("input", "argument")
        .replace("observe", "consume")
        .replace("Word", "Scalar");
    let other = source.replace("hint::black_box", "convert::identity");
    assert_eq!(form(source), form(&renamed));
    assert_ne!(form(source), form(&other));
}

#[test]
fn alias_parameters_schedule_like_their_primitive() {
    let original: syn::File = syn::parse_str(
        "type Word = u32; struct S; fn f(input: Word) -> (Word, Word) { let low = input & 15u32; let high = input >> 4; (low, high) }",
    )
    .unwrap();
    let swapped: syn::File = syn::parse_str(
        "type Word = u32; struct S; fn f(input: Word) -> (Word, Word) { let high = input >> 4; let low = input & 15u32; (low, high) }",
    )
    .unwrap();
    let original_context = SourceContext::new(core::iter::once((&[][..], &original)));
    let swapped_context = SourceContext::new(core::iter::once((&[][..], &swapped)));
    let original_fn = function(&original, "f");
    let swapped_fn = function(&swapped, "f");
    assert_eq!(
        original_context
            .function(&original_fn.sig, &original_fn.block)
            .unwrap()
            .canonicalize(),
        swapped_context
            .function(&swapped_fn.sig, &swapped_fn.block)
            .unwrap()
            .canonicalize(),
    );
}

#[test]
fn cast_targets_keep_their_resolved_identity() {
    let original: syn::File = syn::parse_str(
        "type Wide = u64; type Signed = i64; fn f(input: u32) -> (Wide, Signed) { let low = input & 15u32; let high = input >> 4; (low as Wide, high as Signed) }",
    )
    .unwrap();
    let swapped: syn::File = syn::parse_str(
        "type Wide = u64; type Signed = i64; fn f(input: u32) -> (Wide, Signed) { let high = input >> 4; let low = input & 15u32; (low as Wide, high as Signed) }",
    )
    .unwrap();
    let other: syn::File = syn::parse_str(
        "type Wide = u64; type Signed = i32; fn f(input: u32) -> (Wide, Signed) { let low = input & 15u32; let high = input >> 4; (low as Wide, high as Signed) }",
    )
    .unwrap();
    let original_context = SourceContext::new(core::iter::once((&[][..], &original)));
    let swapped_context = SourceContext::new(core::iter::once((&[][..], &swapped)));
    let other_context = SourceContext::new(core::iter::once((&[][..], &other)));
    let original_fn = function(&original, "f");
    let swapped_fn = function(&swapped, "f");
    let other_fn = function(&other, "f");
    let original_form = original_context
        .function(&original_fn.sig, &original_fn.block)
        .unwrap()
        .canonicalize();
    assert_eq!(
        original_form,
        swapped_context
            .function(&swapped_fn.sig, &swapped_fn.block)
            .unwrap()
            .canonicalize(),
    );
    assert_ne!(
        original_form,
        other_context
            .function(&other_fn.sig, &other_fn.block)
            .unwrap()
            .canonicalize(),
    );
}

#[test]
fn nested_blocks_keep_their_captured_schedule() {
    let original: syn::File = syn::parse_str(
        "fn f(input: u32) -> (u32, u32) { let low = input & 15u32; let high = input >> 4; { (low, high) } }",
    )
    .unwrap();
    let swapped: syn::File = syn::parse_str(
        "fn f(input: u32) -> (u32, u32) { let high = input >> 4; let low = input & 15u32; { (low, high) } }",
    )
    .unwrap();
    let other: syn::File = syn::parse_str(
        "fn f(input: u32) -> (u32, u32) { let low = input & 15u32; let high = input >> 5; { (low, high) } }",
    )
    .unwrap();
    let original_context = SourceContext::new(core::iter::once((&[][..], &original)));
    let swapped_context = SourceContext::new(core::iter::once((&[][..], &swapped)));
    let other_context = SourceContext::new(core::iter::once((&[][..], &other)));
    let original_fn = function(&original, "f");
    let swapped_fn = function(&swapped, "f");
    let other_fn = function(&other, "f");
    let original_form = original_context
        .function(&original_fn.sig, &original_fn.block)
        .unwrap()
        .canonicalize();
    assert_eq!(
        original_form,
        swapped_context
            .function(&swapped_fn.sig, &swapped_fn.block)
            .unwrap()
            .canonicalize(),
    );
    assert_ne!(
        original_form,
        other_context
            .function(&other_fn.sig, &other_fn.block)
            .unwrap()
            .canonicalize(),
    );
}

#[test]
fn closure_captures_keep_their_lexical_order() {
    let original: syn::File = syn::parse_str(
        "fn f(input: u32) -> (u32, u32) { let low = input & 15u32; let high = input >> 4; let capture = || (low, high); std::hint::black_box(capture); (low, high) }",
    )
    .unwrap();
    let swapped: syn::File = syn::parse_str(
        "fn f(input: u32) -> (u32, u32) { let high = input >> 4; let low = input & 15u32; let capture = || (low, high); std::hint::black_box(capture); (low, high) }",
    )
    .unwrap();
    let original_context = SourceContext::new(core::iter::once((&[][..], &original)));
    let swapped_context = SourceContext::new(core::iter::once((&[][..], &swapped)));
    let original_fn = function(&original, "f");
    let swapped_fn = function(&swapped, "f");
    assert_ne!(
        original_context
            .function(&original_fn.sig, &original_fn.block)
            .unwrap()
            .canonicalize(),
        swapped_context
            .function(&swapped_fn.sig, &swapped_fn.block)
            .unwrap()
            .canonicalize(),
    );
}

#[test]
fn shallow_sibling_blocks_keep_their_schedule_near_the_depth_bound() {
    let sibling = "let p = input & 15u32; let q = input >> 4; p ^ q";
    let swapped = "let q = input >> 4; let p = input & 15u32; p ^ q";
    let source = |body| {
        let mut out = String::from("fn f(input: u32) -> u32 {");
        for _ in 0..60 {
            out.push_str(" {");
        }
        for _ in 0..39 {
            write!(out, " {{{body}}};").unwrap();
        }
        write!(out, " {{{body}}}").unwrap();
        for _ in 0..61 {
            out.push('}');
        }
        out
    };
    let original: syn::File = syn::parse_str(&source(sibling)).unwrap();
    let swapped: syn::File = syn::parse_str(&source(swapped)).unwrap();
    let original_context = SourceContext::new(core::iter::once((&[][..], &original)));
    let swapped_context = SourceContext::new(core::iter::once((&[][..], &swapped)));
    let original_fn = function(&original, "f");
    let swapped_fn = function(&swapped, "f");
    assert_eq!(
        original_context
            .function(&original_fn.sig, &original_fn.block)
            .unwrap()
            .canonicalize(),
        swapped_context
            .function(&swapped_fn.sig, &swapped_fn.block)
            .unwrap()
            .canonicalize(),
    );
}

#[test]
fn nested_modules_inherit_their_observer_ancestry() {
    let body = "let low = input & 15u32; let high = input >> 4; (low, high)";
    let swapped = "let high = input >> 4; let low = input & 15u32; (low, high)";
    let template = |attribute, body| {
        format!(
            "{attribute} mod outer {{ mod inner {{ fn f(input: u32) -> (u32, u32) {{ {body} }} }} }}"
        )
    };
    let observed = template("#[observer::inspect]", body);
    let observed_swapped = template("#[observer::inspect]", swapped);
    let plain = template("", body);
    let plain_swapped = template("", swapped);
    assert_ne!(form(&observed), form(&observed_swapped));
    assert_eq!(form(&plain), form(&plain_swapped));
}

#[test]
fn observed_impl_methods_freeze_their_bodies() {
    let body_a = "fn a(input: u32) -> (u32, u32) { let low = input & 15u32; let high = input >> 4; (low, high) }";
    let body_b = "fn b(input: u32) -> (u32, u32) { let high = input >> 4; let low = input & 15u32; (low, high) }";
    let source = |attribute| format!("struct S; {attribute} impl S {{ {body_a} {body_b} }}");
    let observed_file: syn::File = syn::parse_str(&source("#[observer::inspect]")).unwrap();
    let plain_file: syn::File = syn::parse_str(&source("")).unwrap();
    let forms = |file: &syn::File| -> Vec<CanonicalForm> {
        let context = SourceContext::new(core::iter::once((&[][..], file)));
        file.items
            .iter()
            .filter_map(|item| match item {
                syn::Item::Impl(impl_item) => Some(&impl_item.items),
                _ => None,
            })
            .flatten()
            .filter_map(|item| match item {
                syn::ImplItem::Fn(method) => Some(
                    context
                        .function(&method.sig, &method.block)
                        .unwrap()
                        .canonicalize(),
                ),
                _ => None,
            })
            .collect()
    };
    let observed = forms(&observed_file);
    let plain = forms(&plain_file);
    assert_ne!(observed[0], observed[1]);
    assert_eq!(plain[0], plain[1]);
}

#[test]
fn observed_trait_methods_freeze_their_bodies() {
    let body_a = "fn a(input: u32) -> (u32, u32) { let low = input & 15u32; let high = input >> 4; (low, high) }";
    let body_b = "fn b(input: u32) -> (u32, u32) { let high = input >> 4; let low = input & 15u32; (low, high) }";
    let source = |attribute| format!("{attribute} trait Tt {{ {body_a} {body_b} }}");
    let observed_file: syn::File = syn::parse_str(&source("#[observer::inspect]")).unwrap();
    let plain_file: syn::File = syn::parse_str(&source("")).unwrap();
    let forms = |file: &syn::File| -> Vec<CanonicalForm> {
        let context = SourceContext::new(core::iter::once((&[][..], file)));
        file.items
            .iter()
            .filter_map(|item| match item {
                syn::Item::Trait(trait_item) => Some(&trait_item.items),
                _ => None,
            })
            .flatten()
            .filter_map(|item| match item {
                syn::TraitItem::Fn(method) if method.default.is_some() => {
                    let block = method.default.as_ref().unwrap();
                    Some(context.function(&method.sig, block).unwrap().canonicalize())
                }
                _ => None,
            })
            .collect()
    };
    let observed = forms(&observed_file);
    let plain = forms(&plain_file);
    assert_ne!(observed[0], observed[1]);
    assert_eq!(plain[0], plain[1]);
}

#[test]
fn group_wrapped_parameters_keep_their_primitive() {
    let sources = [
        "fn f(input: u32) -> (u32, u32) { let low = input & 15u32; let high = input >> 4; (low, high) }",
        "fn f(input: u32) -> (u32, u32) { let high = input >> 4; let low = input & 15u32; (low, high) }",
    ];
    let forms = sources.map(|source| {
        let mut grouped: syn::File = syn::parse_str(source).unwrap();
        let syn::Item::Fn(function) = &mut grouped.items[0] else {
            unreachable!()
        };
        let syn::FnArg::Typed(pat_type) = function.sig.inputs.iter_mut().next().unwrap() else {
            unreachable!()
        };
        let inner =
            core::mem::replace(&mut *pat_type.ty, syn::parse_str::<syn::Type>("_").unwrap());
        *pat_type.ty = syn::Type::Group(syn::TypeGroup {
            attrs: Vec::new(),
            group_token: syn::token::Group::default(),
            elem: Box::new(inner),
        });
        syn_canon::canonicalize(grouped)
    });
    assert_eq!(forms[0], forms[1]);
}

#[test]
fn chained_alias_parameters_schedule_past_decoy_siblings() {
    let source = |body| {
        format!("type C = S; struct S; type A = B; type B = u32; fn f(x: A) -> (A, A) {{ {body} }}")
    };
    assert_eq!(
        form(&source(
            "let low = x & 15u32; let high = x >> 4; (low, high)"
        )),
        form(&source(
            "let high = x >> 4; let low = x & 15u32; (low, high)"
        )),
    );
}

#[test]
fn unrelated_imports_do_not_block_chained_alias_schedules() {
    let source = |body| {
        format!(
            "mod m {{ pub struct other; }} use m::other; type A = B; type B = u32; fn f(x: A) -> (A, A) {{ {body} }}"
        )
    };
    assert_eq!(
        form(&source(
            "let low = x & 15u32; let high = x >> 4; (low, high)"
        )),
        form(&source(
            "let high = x >> 4; let low = x & 15u32; (low, high)"
        )),
    );
}

#[test]
fn sibling_structs_do_not_block_chained_alias_schedules() {
    let source =
        |body| format!("struct S; type A = B; type B = u32; fn f(x: A) -> (A, A) {{ {body} }}");
    assert_eq!(
        form(&source(
            "let low = x & 15u32; let high = x >> 4; (low, high)"
        )),
        form(&source(
            "let high = x >> 4; let low = x & 15u32; (low, high)"
        )),
    );
}

#[test]
fn direct_alias_parameters_schedule_their_bodies() {
    let source = |body| format!("type A = u32; fn f(x: A) -> (A, A) {{ {body} }}");
    assert_eq!(
        form(&source(
            "let low = x & 15u32; let high = x >> 4; (low, high)"
        )),
        form(&source(
            "let high = x >> 4; let low = x & 15u32; (low, high)"
        )),
    );
}

#[test]
fn seventeen_hop_alias_chains_schedule_their_parameters() {
    assert_eq!(
        form(&chained_alias_swap_source(17, false)),
        form(&chained_alias_swap_source(17, true)),
    );
}

#[test]
fn eighteen_hop_alias_chains_lose_their_parameter_schedule() {
    assert_ne!(
        form(&chained_alias_swap_source(18, false)),
        form(&chained_alias_swap_source(18, true)),
    );
}

#[test]
fn cyclic_alias_chains_lose_their_primitives() {
    let failing = |source| syn_canon::canonicalize_failing(syn::parse_str(source).unwrap());
    assert_eq!(
        failing("type A = B; type B = A; fn f(x: A) -> A { x }"),
        failing("type C = D; type D = C; fn f(y: C) -> C { y }"),
    );
}

#[test]
fn single_segment_qself_parameters_lack_primitive_evidence() {
    let source = |body| {
        format!(
            "trait U {{ type u32: Copy + core::ops::BitAnd<u32, Output = u32> + core::ops::Shr<u32, Output = u32>; }}
             fn f<T: U>(input: <T>::u32) -> (u32, u32) {{ {body} }}"
        )
    };
    let qself = form(&source(
        "let low = input & 15u32; let high = input >> 4; (low, high)",
    ));
    let swapped = form(&source(
        "let high = input >> 4; let low = input & 15u32; (low, high)",
    ));
    assert_ne!(qself, swapped);
    let plain = |body: &str| form(&format!("fn f(input: u32) -> (u32, u32) {{ {body} }}"));
    assert_eq!(
        plain("let low = input & 15u32; let high = input >> 4; (low, high)"),
        plain("let high = input >> 4; let low = input & 15u32; (low, high)"),
    );
}

#[test]
fn observed_owners_drop_the_bool_parameter_proof() {
    let source = |attribute: &str| {
        format!("struct S; {attribute} impl S {{ fn m(x: bool) {{ let y = x; let _ = y; }} }}")
    };
    let method_form = |source: &str| {
        let file: syn::File = syn::parse_str(source).unwrap();
        let context = SourceContext::new(core::iter::once((&[][..], &file)));
        let syn::Item::Impl(impl_item) = &file.items[1] else {
            unreachable!()
        };
        let syn::ImplItem::Fn(method) = &impl_item.items[0] else {
            unreachable!()
        };
        context
            .function(&method.sig, &method.block)
            .unwrap()
            .canonicalize()
    };
    assert_ne!(
        method_form(&source("#[observer::inspect]")),
        method_form(&source("")),
    );
}

#[test]
fn disabled_block_aliases_keep_closure_operations_unproven() {
    for attribute in ["#[cfg(any())]", "#[cfg_attr(all(), cfg(any()))]"] {
        let source = |body: &str| {
            format!(
                "struct Scalar(u32);
                 impl Copy for Scalar {{}}
                 impl Clone for Scalar {{
                     fn clone(&self) -> Self {{ *self }}
                 }}
                 impl core::ops::BitAnd<u32> for Scalar {{
                     type Output = u32;
                     fn bitand(self, rhs: u32) -> u32 {{ self.0 & rhs }}
                 }}
                 impl core::ops::Shr<u32> for Scalar {{
                     type Output = u32;
                     fn shr(self, rhs: u32) -> u32 {{ self.0 >> rhs }}
                 }}
                 type Word = Scalar;
                 fn value(input: Word) -> (u32, u32) {{
                     {attribute} type Word = u32;
                     let calculate = |argument: Word| {{ {body} }};
                     calculate(input)
                 }}"
            )
        };
        let original = form(&source(
            "let low = argument & 15u32; let high = argument >> 4; (low, high)",
        ));
        assert_ne!(
            original,
            form(&source(
                "let high = argument >> 4; let low = argument & 15u32; (low, high)",
            ))
        );
        assert_eq!(
            original,
            form(&source(
                "let first = argument & 15u32; let second = argument >> 4; (first, second)",
            ))
        );
    }
}
