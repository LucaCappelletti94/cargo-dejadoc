//! Opaque macro structure in canonical comparisons.

use proc_macro2::{Delimiter, Group};

fn observed(grouped: bool) -> syn::File {
    let mut file: syn::File = syn::parse_str("fn main() { inspect!(value); }").unwrap();
    let syn::Item::Fn(function) = &mut file.items[0] else {
        panic!("function fixture");
    };
    let syn::Stmt::Macro(statement) = &mut function.block.stmts[0] else {
        panic!("macro fixture");
    };
    if grouped {
        statement.mac.tokens = core::iter::once(proc_macro2::TokenTree::Group(Group::new(
            Delimiter::None,
            statement.mac.tokens.clone(),
        )))
        .collect();
    }
    file
}

#[test]
fn invisible_macro_groups_preserve_observable_structure() {
    assert_ne!(
        syn_canon::canonicalize(observed(false)).to_string(),
        syn_canon::canonicalize(observed(true)).to_string(),
    );
    assert_ne!(
        syn_canon::canonicalize_failing(observed(false)).to_string(),
        syn_canon::canonicalize_failing(observed(true)).to_string(),
    );
}

#[test]
fn self_type_macro_expansion_preserves_reference_provenance() {
    let original = "macro_rules! owner { ($ty:ty) => { $ty }; }
        struct Value;
        impl owner!(Value) { fn echo(value: Self) -> Self { value } }";
    let explicit = original.replace("Self", "owner!(Value)");
    for canonicalize in [syn_canon::canonicalize, syn_canon::canonicalize_failing] {
        assert_eq!(
            canonicalize(syn::parse_str(original).unwrap()),
            canonicalize(syn::parse_str(&explicit).unwrap()),
        );
    }
}

#[test]
fn debug_output_propagates_bounded_writer_errors() {
    use std::io::Write;

    let form = syn_canon::canonicalize(syn::parse_str("fn f() -> u32 { 1u32 }").unwrap());
    let mut output = std::io::Cursor::new([0_u8; 1]);
    let error = output.write_fmt(format_args!("{form:?}")).unwrap_err();
    assert_eq!(error.kind(), std::io::ErrorKind::WriteZero);
}

#[test]
fn hashed_collection_lookups_do_not_compare_every_form() {
    use std::collections::{HashSet, hash_map::DefaultHasher};
    use std::hash::{BuildHasherDefault, Hash, Hasher};
    use std::sync::atomic::{AtomicUsize, Ordering};

    static COMPARISONS: AtomicUsize = AtomicUsize::new(0);

    struct Measured<'a> {
        form: &'a syn_canon::CanonicalForm,
    }

    impl PartialEq for Measured<'_> {
        fn eq(&self, other: &Self) -> bool {
            COMPARISONS.fetch_add(1, Ordering::Relaxed);
            self.form == other.form
        }
    }

    impl Eq for Measured<'_> {}

    impl Hash for Measured<'_> {
        fn hash<H: Hasher>(&self, state: &mut H) {
            self.form.hash(state);
        }
    }

    let forms: Vec<_> = (0..64)
        .map(|value| {
            syn_canon::canonicalize(
                syn::parse_str(&format!("fn f() -> u32 {{ {value}u32 }}")).unwrap(),
            )
        })
        .collect();
    COMPARISONS.store(0, Ordering::Relaxed);
    let mut set = HashSet::with_capacity_and_hasher(
        forms.len(),
        BuildHasherDefault::<DefaultHasher>::default(),
    );
    for form in &forms {
        assert!(set.insert(Measured { form }));
    }
    for form in &forms {
        assert!(set.contains(&Measured { form }));
    }
    let comparisons = COMPARISONS.load(Ordering::Relaxed);
    assert!(comparisons < forms.len() * 4, "{comparisons}");
}

#[test]
fn opaque_compound_operators_count_as_body_units_without_swallowing_neighbors() {
    for (input, units) in [
        ("a <<= b", 5),
        ("a >>= b", 5),
        ("a ..= b", 5),
        ("a ... b", 5),
        ("a <<+ b", 6),
        ("a >>+ b", 6),
        ("a +=> b", 6),
        ("a &&& b", 6),
        ("a ->> b", 6),
        ("a ..+ b", 6),
        ("a =<< b", 6),
        ("a =.. b", 6),
    ] {
        let source = format!("fn f() {{ observe!({input}); }}");
        let form = syn_canon::canonicalize(syn::parse_str(&source).unwrap());
        assert_eq!(form.body_units(), units, "{input}");
        assert_eq!(form.leaf_tokens(), 9, "{input}");
    }
}

#[test]
fn macro_arguments_keep_unresolved_canonical_spellings_apart() {
    let owned: syn::File = syn::parse_str("fn a(x: i32) { emit!(x) }").unwrap();
    let renamed: syn::File = syn::parse_str("fn b(y: i32) { emit!(y) }").unwrap();
    let unowned: syn::File = syn::parse_str("fn c(x: i32) { emit!(_canon_1) }").unwrap();
    for canonicalize in [syn_canon::canonicalize, syn_canon::canonicalize_failing] {
        assert_eq!(canonicalize(owned.clone()), canonicalize(renamed.clone()));
        assert_ne!(canonicalize(owned.clone()), canonicalize(unowned.clone()));
    }
}

#[test]
fn macro_tokens_keep_unresolved_canonical_spellings_apart() {
    let owned: syn::File = syn::parse_str("fn a(x: i32) { emit!(x @) }").unwrap();
    let renamed: syn::File = syn::parse_str("fn b(y: i32) { emit!(y @) }").unwrap();
    let unowned: syn::File = syn::parse_str("fn c(x: i32) { emit!(_canon_1 @) }").unwrap();
    for canonicalize in [syn_canon::canonicalize, syn_canon::canonicalize_failing] {
        assert_eq!(canonicalize(owned.clone()), canonicalize(renamed.clone()));
        assert_ne!(canonicalize(owned.clone()), canonicalize(unowned.clone()));
    }
}

#[test]
fn format_placeholders_keep_unresolved_canonical_spellings_apart() {
    let owned: syn::File = syn::parse_str("fn a(x: i32) { format!(\"{x}\"); }").unwrap();
    let renamed: syn::File = syn::parse_str("fn b(y: i32) { format!(\"{y}\"); }").unwrap();
    let unowned: syn::File = syn::parse_str("fn c(x: i32) { format!(\"{_canon_1}\"); }").unwrap();
    for canonicalize in [syn_canon::canonicalize, syn_canon::canonicalize_failing] {
        assert_eq!(canonicalize(owned.clone()), canonicalize(renamed.clone()));
        assert_ne!(canonicalize(owned.clone()), canonicalize(unowned.clone()));
    }
}

#[test]
fn const_generic_parameters_renamed_like_other_binders() {
    let first: syn::File =
        syn::parse_str("fn first<const N: usize>(x: [u8; N]) -> [u8; N] { x }").unwrap();
    let renamed: syn::File =
        syn::parse_str("fn renamed<const M: usize>(y: [u8; M]) -> [u8; M] { y }").unwrap();
    let bare: syn::File = syn::parse_str("fn bare(x: [u8; 1]) -> [u8; 1] { x }").unwrap();
    for canonicalize in [syn_canon::canonicalize, syn_canon::canonicalize_failing] {
        assert_eq!(canonicalize(first.clone()), canonicalize(renamed.clone()));
        assert_ne!(canonicalize(first.clone()), canonicalize(bare.clone()));
    }
}

#[test]
fn impl_self_type_renamed_like_its_definition() {
    let first: syn::File =
        syn::parse_str("struct Value; impl Value { fn describe(self) -> Self { self } }").unwrap();
    let renamed: syn::File =
        syn::parse_str("struct Other; impl Other { fn describe(self) -> Self { self } }").unwrap();
    let explicit: syn::File =
        syn::parse_str("struct Value; impl Value { fn describe(self) -> Value { self } }").unwrap();
    for canonicalize in [syn_canon::canonicalize, syn_canon::canonicalize_failing] {
        assert_eq!(canonicalize(first.clone()), canonicalize(renamed.clone()));
        assert_eq!(canonicalize(first.clone()), canonicalize(explicit.clone()));
    }
}

#[test]
fn qualified_module_steps_ignore_same_spelling_functions() {
    let first: syn::File = syn::parse_str(
        "
        mod first {
            pub mod inner { pub fn leaf() -> u32 { 7 } }
            pub fn inner() -> u32 { 1 }
        }
        fn caller() -> u32 { first::inner::leaf() }
        ",
    )
    .unwrap();
    let renamed: syn::File = syn::parse_str(
        "
        mod second {
            pub mod middle { pub fn result() -> u32 { 7 } }
            pub fn middle() -> u32 { 1 }
        }
        fn renamed() -> u32 { second::middle::result() }
        ",
    )
    .unwrap();
    for canonicalize in [syn_canon::canonicalize, syn_canon::canonicalize_failing] {
        assert_eq!(canonicalize(first.clone()), canonicalize(renamed.clone()));
    }
}

#[test]
fn local_aliases_preserve_binding_identity_across_resolution_paths() {
    for source in [
        "fn f(input: u16) -> u16 { type Word = Storage; type Storage = u16; let output: Word = input ^ 1; output }",
        "fn f(input: u16) -> u16 { use core::primitive::u16 as Number; type Word = Number; let output: Word = input ^ 1; output }",
        "fn f(input: u16) -> u16 { use core::primitive::*; type Word = u16; let output: Word = input ^ 1; output }",
        "fn f(input: u16) -> u16 { #[cfg_attr(all(), allow(dead_code))] type Word = u16; let output: Word = input ^ 1; output }",
    ] {
        let renamed = source
            .replace("f(", "g(")
            .replace("input", "source")
            .replace("output", "result")
            .replace("Word", "Scalar")
            .replace("Storage", "Base")
            .replace("Number", "Imported");
        let different = source.replace("output }", "input }");
        let form = syn_canon::canonicalize(syn::parse_str(source).unwrap());
        assert_eq!(
            form,
            syn_canon::canonicalize(syn::parse_str(&renamed).unwrap()),
            "{source}",
        );
        assert_ne!(
            form,
            syn_canon::canonicalize(syn::parse_str(&different).unwrap()),
            "{source}",
        );
    }
}

#[test]
fn invisible_parameter_type_groups_preserve_binding_identity() {
    let form = |source: &str| {
        let mut file: syn::File = syn::parse_str(source).unwrap();
        let syn::Item::Fn(function) = &mut file.items[0] else {
            panic!("function fixture");
        };
        let syn::FnArg::Typed(mut input) = function.sig.inputs.pop().unwrap() else {
            panic!("parameter fixture");
        };
        input.ty = Box::new(syn::Type::Group(syn::TypeGroup {
            attrs: Vec::new(),
            group_token: syn::token::Group::default(),
            elem: input.ty,
        }));
        function.sig.inputs.push(syn::FnArg::Typed(input));
        syn_canon::canonicalize(file)
    };
    assert_eq!(
        form("fn f(input: u16) -> u16 { let result = input ^ 1; result << 1 }"),
        form("fn g(source: u16) -> u16 { let value = source ^ 1; value << 1 }"),
    );
    assert_ne!(
        form("fn f(input: u16) -> u16 { let result = input ^ 1; result << 1 }"),
        form("fn f(input: u16) -> u16 { let result = input ^ 1; input << 1 }"),
    );
}

#[test]
fn computed_documentation_preserves_grouped_import_binding_identity() {
    let source = "#[doc = concat!(\"local\", \" imports\")]
        fn f(input: u16) -> u16 {
            use {core::primitive::u16 as Imported, core::mem::size_of as measure};
            type Word = u16;
            let output: Word = input ^ 1;
            let _: Imported = output;
            let _ = measure::<Word>();
            output
        }";
    let renamed = source
        .replace("f(", "g(")
        .replace("input", "source")
        .replace("output", "result")
        .replace("Imported", "Scalar")
        .replace("measure", "size")
        .replace("Word", "Storage");
    let different = source.replace("            output", "            input");
    let form = syn_canon::canonicalize(syn::parse_str(source).unwrap());
    assert_eq!(
        form,
        syn_canon::canonicalize(syn::parse_str(&renamed).unwrap()),
    );
    assert_ne!(
        form,
        syn_canon::canonicalize(syn::parse_str(&different).unwrap()),
    );
}

#[test]
fn long_local_alias_chains_preserve_binding_identity() {
    use std::fmt::Write;

    let mut source = String::from("fn f(input: u16) -> u16 {");
    for index in 0..32 {
        write!(source, "type Alias{index} = Alias{};", index + 1).unwrap();
    }
    source.push_str("type Alias32 = u16; let output: Alias0 = input ^ 1; output }");
    let renamed = source
        .replace("f(", "g(")
        .replace("Alias", "Word")
        .replace("input", "source")
        .replace("output", "result");
    let different = source.replace("input ^ 1", "input ^ 2");
    let form = syn_canon::canonicalize(syn::parse_str(&source).unwrap());
    assert_eq!(
        form,
        syn_canon::canonicalize(syn::parse_str(&renamed).unwrap()),
    );
    assert_ne!(
        form,
        syn_canon::canonicalize(syn::parse_str(&different).unwrap()),
    );
}

#[test]
fn derived_array_types_preserve_nested_import_binding_identity() {
    let source = "#[derive(Clone)]
        struct Owner([u8; {
            fn f(input: u16) -> u16 {
                use {core::primitive::u16 as Word, core::mem::size_of as measure};
                let value: Word = input ^ 1;
                let _ = measure::<Word>();
                value
            }
            1
        }]);";
    let renamed = source
        .replace("f(", "g(")
        .replace("input", "source")
        .replace("Word", "Scalar")
        .replace("measure", "size")
        .replace("value", "result");
    let different = source.replace("input ^ 1", "input ^ 2");
    let form = syn_canon::canonicalize(syn::parse_str(source).unwrap());
    assert_eq!(
        form,
        syn_canon::canonicalize(syn::parse_str(&renamed).unwrap()),
    );
    assert_ne!(
        form,
        syn_canon::canonicalize(syn::parse_str(&different).unwrap()),
    );
}

#[test]
fn module_aliases_preserve_binding_identity_under_derive_uncertainty() {
    let source = "mod types {
            #[derive(Clone)] struct Marker(u8);
            type Word = u16;
            fn f(input: Word) -> Word { let value = input ^ 1; value }
        }";
    let renamed = source
        .replace("f(", "g(")
        .replace("input", "source")
        .replace("Word", "Storage")
        .replace("value", "result");
    let different = source.replace("input ^ 1", "input ^ 2");
    let form = syn_canon::canonicalize(syn::parse_str(source).unwrap());
    assert_eq!(
        form,
        syn_canon::canonicalize(syn::parse_str(&renamed).unwrap()),
    );
    assert_ne!(
        form,
        syn_canon::canonicalize(syn::parse_str(&different).unwrap()),
    );
}
