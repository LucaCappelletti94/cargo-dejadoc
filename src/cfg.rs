//! `cfg` evaluation as rustdoc collects doctests on a 64-bit Linux host
//! with every feature on and `test` off, so a scan gives the same answer
//! on every machine.

use alloc::string::{String, ToString};

use alloc::vec::Vec;
use syn::{Expr, ExprLit, Lit, LitStr, Meta, Token, punctuated::Punctuated};

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

/// The printed `cfg` attributes among `attrs`.
pub(crate) fn cfgs(attrs: &[syn::Attribute]) -> impl Iterator<Item = String> + '_ {
    attrs
        .iter()
        .filter(|a| a.path().is_ident("cfg"))
        .map(|a| quote::ToTokens::to_token_stream(a).to_string())
}

/// The metas a `#[cfg_attr(pred, …)]` applies, nested ones expanded in
/// place, `None` unless it is one and its predicate holds.
pub(crate) fn cfg_attr_metas(attr: &syn::Attribute) -> Option<Vec<Meta>> {
    let mut metas = Vec::new();
    expand(&attr.meta, &mut metas).then_some(metas)
}

/// Push what `meta` applies onto `out` when it is a `cfg_attr` whose
/// predicate holds, and tell whether it was.
fn expand(meta: &Meta, out: &mut Vec<Meta>) -> bool {
    let Meta::List(list) = meta else {
        return false;
    };
    if !list.path.is_ident("cfg_attr") {
        return false;
    }
    let Ok(metas) = list.parse_args_with(Punctuated::<Meta, Token![,]>::parse_terminated) else {
        return false;
    };
    let mut metas = metas.into_iter();
    if !metas.next().is_some_and(|pred| holds(&pred)) {
        return false;
    }
    for meta in metas {
        if meta.path().is_ident("cfg_attr") {
            expand(&meta, out);
        } else {
            out.push(meta);
        }
    }
    true
}

/// The string literal `expr` is, if any.
pub(crate) fn lit_str(expr: &Expr) -> Option<&LitStr> {
    match expr {
        Expr::Lit(ExprLit {
            lit: Lit::Str(s), ..
        }) => Some(s),
        _ => None,
    }
}

/// Whether a cfg predicate holds.
pub(crate) fn holds(pred: &Meta) -> bool {
    let list = match pred {
        Meta::Path(path) => return path.get_ident().is_some_and(|i| flag(&i.to_string())),
        Meta::NameValue(nv) => {
            return lit_str(&nv.value).is_some_and(|value| {
                nv.path
                    .get_ident()
                    .is_some_and(|i| pair(&i.to_string(), &value.value()))
            });
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
        "target_abi" => value.is_empty(),
        "target_has_atomic" => matches!(value, "8" | "16" | "32" | "64" | "ptr"),
        "target_feature" => matches!(value, "fxsr" | "sse" | "sse2"),
        "panic" => value == "unwind",
        _ => false,
    }
}

/// Source-ordered module path providers with expanded `cfg_attr` conditions, the first
/// applying provider winning as in rustc.
pub(crate) struct PathClaims {
    /// The top-level `#[path]` value, if any.
    pub(crate) direct: Option<String>,
    /// The condition under which the top-level path is linked.
    pub(crate) direct_pred: Vec<String>,
    /// The `cfg_attr` providers in source order, each with its linking condition.
    pub(crate) branches: Vec<(Vec<String>, String)>,
    /// The condition under which the implicit files are linked, every provider failing.
    pub(crate) fallback: Vec<String>,
}

/// The path providers of a module declaration's `attrs`, `atom` printing one `cfg_attr`
/// predicate into a condition.
pub(crate) fn path_claims(attrs: &[syn::Attribute], atom: &dyn Fn(&Meta) -> String) -> PathClaims {
    let mut providers: Vec<(Vec<String>, String)> = Vec::new();
    let mut direct: Option<String> = None;
    for attr in attrs {
        if let Some(path) = path_value(&attr.meta) {
            if direct.is_none() {
                direct = Some(path.clone());
            }
            providers.push((Vec::new(), path));
            continue;
        }
        let Meta::List(list) = &attr.meta else {
            continue;
        };
        if !list.path.is_ident("cfg_attr") {
            continue;
        }
        let Ok(metas) = list.parse_args_with(Punctuated::<Meta, Token![,]>::parse_terminated)
        else {
            continue;
        };
        let Some(pred) = metas.first() else {
            continue;
        };
        cfg_attr_paths(metas.iter().skip(1), &[atom(pred)], atom, &mut providers);
    }
    let mut direct_pred: Vec<String> = Vec::new();
    let mut branches: Vec<(Vec<String>, String)> = Vec::new();
    for (index, (own, path)) in providers.iter().enumerate() {
        let linkable: Vec<String> = own
            .iter()
            .cloned()
            .chain(
                providers[..index]
                    .iter()
                    .map(|(earlier, _)| negate(earlier)),
            )
            .collect();
        if own.is_empty() {
            direct_pred = linkable;
        } else {
            branches.push((linkable, path.clone()));
        }
    }
    let fallback: Vec<String> = providers.iter().map(|(own, _)| negate(own)).collect();
    PathClaims {
        direct,
        direct_pred,
        branches,
        fallback,
    }
}

/// The `path = "..."` value `meta` is, if any.
fn path_value(meta: &Meta) -> Option<String> {
    match meta {
        Meta::NameValue(nv) if nv.path.is_ident("path") => lit_str(&nv.value).map(LitStr::value),
        _ => None,
    }
}

/// The `path` values of the `cfg_attr` metas, nested levels expanded into the condition chain
/// in the order declared.
fn cfg_attr_paths<'a>(
    metas: impl Iterator<Item = &'a Meta>,
    chain: &[String],
    atom: &dyn Fn(&Meta) -> String,
    out: &mut Vec<(Vec<String>, String)>,
) {
    for meta in metas {
        if let Some(path) = path_value(meta) {
            out.push((chain.to_vec(), path));
            continue;
        }
        let Meta::List(list) = meta else {
            continue;
        };
        if !list.path.is_ident("cfg_attr") {
            continue;
        }
        let Ok(inner) = list.parse_args_with(Punctuated::<Meta, Token![,]>::parse_terminated)
        else {
            continue;
        };
        let Some(pred) = inner.first() else {
            continue;
        };
        let mut deep = chain.to_vec();
        deep.push(atom(pred));
        cfg_attr_paths(inner.iter().skip(1), &deep, atom, out);
    }
}

/// The negation of a provider's own condition, `false` for an unconditional one.
pub(crate) fn negate(conditions: &[String]) -> String {
    if conditions.is_empty() {
        "false".to_string()
    } else {
        alloc::format!("not({})", render_conjunction(conditions))
    }
}

/// The text of a conjunction of conditions, empty for none.
pub(crate) fn render_conjunction(conditions: &[String]) -> String {
    match conditions.len() {
        0 => String::new(),
        1 => conditions[0].clone(),
        _ => alloc::format!("all({})", conditions.join(", ")),
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
            "target_abi = \"\"",
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
            "target_abi = \"eabi\"",
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

    #[test]
    fn nested_cfg_attr_expands_in_place_when_its_predicate_holds() {
        let names = |src: &str| -> Vec<alloc::string::String> {
            attrs(src)
                .iter()
                .filter_map(cfg_attr_metas)
                .flatten()
                .map(|meta| meta.path().get_ident().unwrap().to_string())
                .collect()
        };
        assert_eq!(
            names(
                "#[cfg_attr(unix, must_use, cfg_attr(feature = \"a\", doc = \"x\", cfg_attr(windows, cold)), inline)]"
            ),
            ["must_use", "doc", "inline"]
        );
        assert_eq!(
            names("#[cfg_attr(windows, cfg_attr(unix, doc = \"x\"))]"),
            Vec::<alloc::string::String>::new()
        );
        // A malformed `cfg_attr`, outer or nested, applies nothing.
        assert_eq!(
            names("#[cfg_attr]\n#[cfg_attr(unix, = x)]\n#[cfg_attr(unix, cfg_attr, inline)]"),
            ["inline"]
        );
        assert!(cfg_attr_metas(&attrs("#[cfg_attr]")[0]).is_none());
    }
}
