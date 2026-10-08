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
