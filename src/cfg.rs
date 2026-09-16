//! `cfg` evaluation as rustdoc collects doctests on a 64-bit Linux host
//! with every feature on and `test` off, so a scan gives the same answer
//! on every machine.

use alloc::string::ToString;

use syn::{Expr, ExprLit, Lit, Meta, Token, punctuated::Punctuated};

/// Whether every `#[cfg(…)]` in `attrs` holds.
pub(crate) fn allows(attrs: &[syn::Attribute]) -> bool {
    attrs.iter().all(|attr| {
        !attr.path().is_ident("cfg")
            || match &attr.meta {
                Meta::List(list) => list.parse_args::<Meta>().is_ok_and(|pred| holds(&pred)),
                _ => true,
            }
    })
}

/// Whether a cfg predicate holds.
pub(crate) fn holds(pred: &Meta) -> bool {
    let list = match pred {
        Meta::Path(path) => return path.get_ident().is_some_and(|i| flag(&i.to_string())),
        Meta::NameValue(nv) => {
            let Expr::Lit(ExprLit {
                lit: Lit::Str(value),
                ..
            }) = &nv.value
            else {
                return false;
            };
            return nv
                .path
                .get_ident()
                .is_some_and(|i| pair(&i.to_string(), &value.value()));
        }
        Meta::List(list) => list,
    };
    if list.path.is_ident("not") {
        return !list.parse_args::<Meta>().is_ok_and(|inner| holds(&inner));
    }
    let inner = list.parse_args_with(Punctuated::<Meta, Token![,]>::parse_terminated);
    if list.path.is_ident("all") {
        inner.is_ok_and(|inner| inner.iter().all(holds))
    } else if list.path.is_ident("any") {
        inner.is_ok_and(|inner| inner.iter().any(holds))
    } else {
        false
    }
}

/// A bare cfg name that is set.
fn flag(name: &str) -> bool {
    matches!(name, "doc" | "doctest" | "unix" | "debug_assertions")
}

/// A `name = "value"` cfg that is set.
fn pair(name: &str, value: &str) -> bool {
    match name {
        "feature" => true,
        "target_os" => value == "linux",
        "target_family" => value == "unix",
        "target_arch" => value == "x86_64",
        "target_pointer_width" => value == "64",
        "target_endian" => value == "little",
        "target_env" => value == "gnu",
        "target_vendor" => value == "unknown",
        "target_has_atomic" => matches!(value, "8" | "16" | "32" | "64" | "ptr"),
        "target_feature" => matches!(value, "fxsr" | "sse" | "sse2"),
        "panic" => value == "unwind",
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec::Vec;

    fn attrs(src: &str) -> Vec<syn::Attribute> {
        syn::parse_str::<syn::ItemFn>(&alloc::format!("{src}\nfn f() {{}}"))
            .unwrap()
            .attrs
    }

    #[test]
    fn rustdoc_flags_hold_and_test_does_not() {
        assert!(!allows(&attrs("#[cfg(test)]")));
        assert!(allows(&attrs("#[cfg(not(test))]")));
        assert!(allows(&attrs("#[cfg(doc)]")));
        assert!(allows(&attrs("#[cfg(doctest)]")));
        assert!(!allows(&attrs("#[cfg(not(doc))]")));
        assert!(allows(&attrs("#[cfg(debug_assertions)]")));
    }

    #[test]
    fn every_feature_is_on() {
        assert!(allows(&attrs("#[cfg(feature = \"x\")]")));
        assert!(!allows(&attrs("#[cfg(not(feature = \"x\"))]")));
        assert!(allows(&attrs("#[cfg(any(test, feature = \"x\"))]")));
        assert!(!allows(&attrs(
            "#[cfg(all(feature = \"x\", not(doctest)))]"
        )));
    }

    #[test]
    fn the_host_is_linux_x86_64() {
        let on = [
            "unix",
            "target_os = \"linux\"",
            "target_family = \"unix\"",
            "target_arch = \"x86_64\"",
            "target_pointer_width = \"64\"",
            "target_endian = \"little\"",
            "target_env = \"gnu\"",
            "target_vendor = \"unknown\"",
            "target_has_atomic = \"ptr\"",
            "target_feature = \"sse2\"",
            "panic = \"unwind\"",
        ];
        let off = [
            "windows",
            "target_os = \"macos\"",
            "target_family = \"wasm\"",
            "target_arch = \"wasm32\"",
            "target_pointer_width = \"32\"",
            "target_endian = \"big\"",
            "target_env = \"msvc\"",
            "target_vendor = \"apple\"",
            "target_has_atomic = \"128\"",
            "target_feature = \"avx2\"",
            "panic = \"abort\"",
        ];
        for pred in on {
            assert!(allows(&attrs(&alloc::format!("#[cfg({pred})]"))), "{pred}");
        }
        for pred in off {
            assert!(!allows(&attrs(&alloc::format!("#[cfg({pred})]"))), "{pred}");
        }
    }

    #[test]
    fn custom_cfgs_are_off() {
        assert!(!allows(&attrs("#[cfg(crossbeam_loom)]")));
        assert!(allows(&attrs("#[cfg(not(loom))]")));
        assert!(!allows(&attrs("#[cfg(docsrs)]")));
        assert!(!allows(&attrs("#[cfg(py = \"3.14\")]")));
        assert!(!allows(&attrs("#[cfg(version(\"1.0\"))]")));
        assert!(!allows(&attrs("#[cfg(feature = 1)]")));
    }

    #[test]
    fn empty_combinators_and_every_attribute() {
        assert!(!allows(&attrs("#[cfg(any())]")));
        assert!(allows(&attrs("#[cfg(all())]")));
        assert!(!allows(&attrs("#[cfg(doc)]\n#[cfg(test)]")));
        assert!(allows(&attrs("#[inline]\n#[cfg(doc)]")));
        assert!(allows(&attrs("#[cfg]")));
    }
}
