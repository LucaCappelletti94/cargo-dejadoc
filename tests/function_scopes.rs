//! A function view keeps the syntax an enclosing attribute macro reads, the derive input of a
//! std derive name its module may bind to another macro, and the format string of a call its
//! scopes do not prove to be the std formatting macro.

use syn_canon::{CanonicalForm, SourceContext};

/// The view of the function `f`, free or in an impl, canonicalized in its source context.
fn view(source: &str) -> CanonicalForm {
    let file: syn::File = syn::parse_str(source).unwrap_or_else(|e| panic!("{source}: {e}"));
    let context = SourceContext::new(core::iter::once((&[][..], &file)));
    let (sig, block) = file
        .items
        .iter()
        .find_map(|item| match item {
            syn::Item::Fn(f) if f.sig.ident == "f" => Some((&f.sig, &*f.block)),
            syn::Item::Impl(i) => i.items.iter().find_map(|member| match member {
                syn::ImplItem::Fn(f) if f.sig.ident == "f" => Some((&f.sig, &f.block)),
                _ => None,
            }),
            _ => None,
        })
        .expect("a function f");
    context
        .function(sig, block)
        .expect("an indexed f")
        .canonicalize()
}

#[test]
fn an_enclosing_attribute_macro_reads_the_method_as_written() {
    let source = |attr: &str, body: &str| {
        format!("struct S;\n{attr}\nimpl S {{\n    fn f() -> u8 {{ {body} }}\n}}")
    };
    assert_ne!(
        view(&source("#[observe]", "return 1;")),
        view(&source("#[observe]", "1"))
    );
    assert_eq!(view(&source("", "return 1;")), view(&source("", "1")));
}

#[test]
fn a_module_binding_of_a_std_derive_name_keeps_the_derive_input() {
    let source = |scope: &str, derives: &str| {
        format!(
            "{scope}\nfn f() -> u8 {{\n    #[derive({derives})]\n    struct P {{ x: u8 }}\n    1\n}}"
        )
    };
    for scope in [
        "use count::Debug;",
        "use count::*;",
        "#[macro_use]\nextern crate count;",
    ] {
        assert_ne!(
            view(&source(scope, "Debug, Clone")),
            view(&source(scope, "Clone, Debug")),
            "{scope}"
        );
    }
    for scope in ["", "use std::fmt::Debug;", "use count::Other;"] {
        assert_eq!(
            view(&source(scope, "Debug, Clone")),
            view(&source(scope, "Clone, Debug")),
            "{scope}"
        );
    }
}

/// The view of the function `f` in module `m`, declared in its own file under a crate root
/// holding `root`.
fn module_view(root: &str, module: &str) -> CanonicalForm {
    let parse = |source: &str| -> syn::File {
        syn::parse_str(source).unwrap_or_else(|e| panic!("{source}: {e}"))
    };
    let (root, module) = (parse(&format!("mod m;\n{root}")), parse(module));
    let path = [String::from("m")];
    let context = SourceContext::new([(&[][..], &root), (&path[..], &module)]);
    let f = module
        .items
        .iter()
        .find_map(|item| match item {
            syn::Item::Fn(f) if f.sig.ident == "f" => Some(f),
            _ => None,
        })
        .expect("a function f");
    context
        .function(&f.sig, &f.block)
        .expect("an indexed f")
        .canonicalize()
}

#[test]
fn format_captures_rename_when_the_module_proves_the_std_macro() {
    let capture = |module: &str, name: &str| {
        module_view(
            "",
            &format!("{module}\nfn f({name}: u8) -> String {{ format!(\"{{{name}}}\") }}"),
        )
    };
    for module in [
        "#[derive(Debug)]\nstruct S;\n#[cfg(test)]\nmod tests {}",
        "use std::fmt::*;",
    ] {
        assert_eq!(capture(module, "x"), capture(module, "y"), "{module}");
    }
    for module in ["use count::format;", "use count::*;"] {
        assert_ne!(capture(module, "x"), capture(module, "y"), "{module}");
    }
}
