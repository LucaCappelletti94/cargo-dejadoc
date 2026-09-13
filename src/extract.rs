//! Doc comment walking and doctest block extraction.

use alloc::format;
use alloc::string::String;
use alloc::string::ToString;
use alloc::vec;
use alloc::vec::Vec;

use quote::ToTokens;
use syn::{
    AttrStyle, Expr, ExprLit, ExprMacro, ForeignItem, Ident, Item, Lit, LitStr, Macro, Meta, Token,
    TraitItem, Type, UseTree,
    parse::{Parse, ParseStream},
    parse2,
    punctuated::Punctuated,
};

use crate::DocTest;
use crate::fence;

/// A run of doc text with per-line source positions. One source spans the
/// item's whole doc stream: its lines may come from different files
/// (`#[doc = include_str!(…)]` splices other files in).
struct DocSource {
    text: String,
    /// 1-based source line of each text line; one entry per text line.
    lines: Vec<u32>,
    /// File of each text line; one entry per text line.
    files: Vec<String>,
}

/// Extract doctest blocks from one parsed source file, under the item path
/// `prefix` (the target name for the root file, with module segments for
/// submodules). Site paths drop the `root` prefix; `read_include` resolves an
/// `include_str!` doc splice to `(path, text)`.
#[must_use]
pub fn extract(
    prefix: &str,
    file: &str,
    parsed: &syn::File,
    root: &str,
    read_include: &dyn Fn(&str) -> Option<(String, String)>,
) -> Vec<DocTest> {
    let mut out = Vec::new();
    // File-level `//!` docs: inner doc attributes only.
    let inner: Vec<syn::Attribute> = parsed
        .attrs
        .iter()
        .filter(|a| matches!(a.style, AttrStyle::Inner { .. }))
        .cloned()
        .collect();
    for src in doc_sources(&inner, file, read_include) {
        out.extend(blocks(&src, prefix, root));
    }
    for item in &parsed.items {
        walk_item(item, prefix, file, root, read_include, &mut out);
    }
    out
}

fn item_attrs(item: &Item) -> &[syn::Attribute] {
    match item {
        Item::Const(c) => &c.attrs,
        Item::Enum(e) => &e.attrs,
        Item::ExternCrate(e) => &e.attrs,
        Item::Fn(f) => &f.attrs,
        Item::ForeignMod(f) => &f.attrs,
        Item::Impl(i) => &i.attrs,
        Item::Macro(m) => &m.attrs,
        Item::Mod(m) => &m.attrs,
        Item::Static(s) => &s.attrs,
        Item::Struct(s) => &s.attrs,
        Item::Trait(t) => &t.attrs,
        Item::TraitAlias(t) => &t.attrs,
        Item::Type(t) => &t.attrs,
        Item::Union(u) => &u.attrs,
        Item::Use(u) => &u.attrs,
        _ => &[],
    }
}

fn walk_item(
    item: &Item,
    prefix: &str,
    file: &str,
    root: &str,
    read_include: &dyn Fn(&str) -> Option<(String, String)>,
    out: &mut Vec<DocTest>,
) {
    match item {
        Item::Mod(m) => {
            let sub = format!("{prefix}::{}", m.ident);
            // `m.attrs` also holds the mod body's inner `//!` docs.
            push_doc(item_attrs(item), &sub, file, root, read_include, out);
            if let Some((_, items)) = &m.content {
                for child in items {
                    walk_item(child, &sub, file, root, read_include, out);
                }
            }
        }
        Item::Impl(i) => {
            let sub = format!("{prefix}::{}", type_name(&i.self_ty));
            push_doc(item_attrs(item), &sub, file, root, read_include, out);
            for assoc in &i.items {
                if let Some(name) = impl_assoc_name(assoc) {
                    push_doc(
                        assoc_attrs(assoc),
                        &format!("{sub}::{name}"),
                        file,
                        root,
                        read_include,
                        out,
                    );
                }
            }
        }
        Item::Trait(t) => {
            let sub = format!("{prefix}::{}", t.ident);
            push_doc(item_attrs(item), &sub, file, root, read_include, out);
            for assoc in &t.items {
                if let Some(name) = trait_assoc_name(assoc) {
                    push_doc(
                        trait_assoc_attrs(assoc),
                        &format!("{sub}::{name}"),
                        file,
                        root,
                        read_include,
                        out,
                    );
                }
            }
        }
        Item::ForeignMod(f) => {
            push_doc(item_attrs(item), prefix, file, root, read_include, out);
            for foreign in &f.items {
                if let Some(name) = foreign_name(foreign) {
                    push_doc(
                        foreign_attrs(foreign),
                        &format!("{prefix}::{name}"),
                        file,
                        root,
                        read_include,
                        out,
                    );
                }
            }
        }
        _ => {
            if let Some(name) = item_name(item) {
                push_doc(
                    item_attrs(item),
                    &format!("{prefix}::{name}"),
                    file,
                    root,
                    read_include,
                    out,
                );
            }
        }
    }
}

fn impl_assoc_name(assoc: &syn::ImplItem) -> Option<String> {
    match assoc {
        syn::ImplItem::Const(c) => Some(c.ident.to_string()),
        syn::ImplItem::Fn(f) => Some(f.sig.ident.to_string()),
        syn::ImplItem::Type(t) => Some(t.ident.to_string()),
        syn::ImplItem::Macro(m) => Some(macro_name(None, &m.mac)),
        _ => None,
    }
}

fn trait_assoc_name(assoc: &TraitItem) -> Option<String> {
    match assoc {
        TraitItem::Const(c) => Some(c.ident.to_string()),
        TraitItem::Fn(f) => Some(f.sig.ident.to_string()),
        TraitItem::Type(t) => Some(t.ident.to_string()),
        TraitItem::Macro(m) => Some(macro_name(None, &m.mac)),
        _ => None,
    }
}

fn foreign_name(foreign: &ForeignItem) -> Option<String> {
    match foreign {
        ForeignItem::Fn(f) => Some(f.sig.ident.to_string()),
        ForeignItem::Static(s) => Some(s.ident.to_string()),
        ForeignItem::Type(t) => Some(t.ident.to_string()),
        ForeignItem::Macro(m) => Some(macro_name(None, &m.mac)),
        _ => None,
    }
}

fn assoc_attrs(assoc: &syn::ImplItem) -> &[syn::Attribute] {
    match assoc {
        syn::ImplItem::Const(c) => &c.attrs,
        syn::ImplItem::Fn(f) => &f.attrs,
        syn::ImplItem::Type(t) => &t.attrs,
        syn::ImplItem::Macro(m) => &m.attrs,
        _ => &[],
    }
}

fn trait_assoc_attrs(assoc: &TraitItem) -> &[syn::Attribute] {
    match assoc {
        TraitItem::Const(c) => &c.attrs,
        TraitItem::Fn(f) => &f.attrs,
        TraitItem::Type(t) => &t.attrs,
        TraitItem::Macro(m) => &m.attrs,
        _ => &[],
    }
}

fn foreign_attrs(foreign: &ForeignItem) -> &[syn::Attribute] {
    match foreign {
        ForeignItem::Fn(f) => &f.attrs,
        ForeignItem::Static(s) => &s.attrs,
        ForeignItem::Type(t) => &t.attrs,
        ForeignItem::Macro(m) => &m.attrs,
        _ => &[],
    }
}

fn line_of(span: proc_macro2::Span) -> u32 {
    let line = span.start().line;
    // A line count beyond u32::MAX is physically unreachable; clamp it.
    u32::try_from(line).unwrap_or(u32::MAX)
}

fn push_doc(
    attrs: &[syn::Attribute],
    item: &str,
    file: &str,
    root: &str,
    read_include: &dyn Fn(&str) -> Option<(String, String)>,
    out: &mut Vec<DocTest>,
) {
    for src in doc_sources(attrs, file, read_include) {
        out.extend(blocks(&src, item, root));
    }
}

/// Doc text from an item's attributes: `#[doc = "…"]` literals,
/// `include_str!` files, and `concat!` mixes, assembled into one doc
/// stream as rustdoc does, so fences may straddle include boundaries.
fn doc_sources(
    attrs: &[syn::Attribute],
    file: &str,
    read_include: &dyn Fn(&str) -> Option<(String, String)>,
) -> Vec<DocSource> {
    let mut parts: Vec<(String, u32, String)> = Vec::new();
    for attr in attrs.iter().filter(|a| a.path().is_ident("doc")) {
        let line = line_of(attr.pound_token.span);
        let Meta::NameValue(nv) = &attr.meta else {
            continue;
        };
        match &nv.value {
            Expr::Lit(ExprLit {
                lit: Lit::Str(s), ..
            }) => parts.push((strip_doc_spaces(&s.value()), line, file.to_string())),
            Expr::Macro(ExprMacro { mac, .. }) if mac.path.is_ident("include_str") => {
                if let Ok(s) = parse2::<LitStr>(mac.tokens.clone()) {
                    push_include(&mut parts, read_include, &s.value());
                }
            }
            Expr::Macro(ExprMacro { mac, .. }) if mac.path.is_ident("concat") => {
                if let Ok(concat) = parse2::<ConcatParts>(mac.tokens.clone()) {
                    for part in concat.0 {
                        match part {
                            ConcatPart::Lit(s) => {
                                parts.push((strip_doc_spaces(&s.value()), line, file.to_string()));
                            }
                            ConcatPart::Include(s) => {
                                push_include(&mut parts, read_include, &s.value());
                            }
                        }
                    }
                }
            }
            _ => {}
        }
    }
    merge_parts(parts)
}

fn push_include(
    parts: &mut Vec<(String, u32, String)>,
    read: &dyn Fn(&str) -> Option<(String, String)>,
    p: &str,
) {
    if let Some((path, text)) = read(p) {
        parts.push((text, 1, path));
    }
}

/// Merge all parts of one item's doc stream into a single source: rustdoc
/// assembles the stream regardless of where each line comes from.
fn merge_parts(parts: Vec<(String, u32, String)>) -> Vec<DocSource> {
    let mut out: Vec<DocSource> = Vec::new();
    let mut prev: Option<(u32, String)> = None;
    for (text, line, file) in parts {
        let lines = line_range(&text, line);
        let files = vec![file.clone(); lines.len()];
        if let Some(last) = out.last_mut() {
            // The separator '\n' after a part that ended a line (or is
            // empty) adds one blank text line; attribute it to the
            // previous part's file and line.
            if last.text.is_empty() || last.text.ends_with('\n') {
                let (pbase, pfile) = prev.unwrap();
                let line = if last.lines.is_empty() {
                    pbase
                } else {
                    *last.lines.last().unwrap() + 1
                };
                last.lines.push(line);
                last.files.push(pfile);
            }
            last.text.push('\n');
            last.text.push_str(&text);
            last.lines.extend(lines);
            last.files.extend(files);
        } else {
            out.push(DocSource { text, lines, files });
        }
        prev = Some((line, file));
    }
    out
}

/// Strip one leading space per line: `///` comments keep it in the `#[doc]`
/// value, but rustdoc drops it.
fn strip_doc_spaces(text: &str) -> String {
    text.split('\n')
        .map(|l| l.strip_prefix(' ').unwrap_or(l))
        .collect::<Vec<_>>()
        .join("\n")
}
/// 1-based source lines occupied by `text` starting at `line`.
fn line_range(text: &str, line: u32) -> Vec<u32> {
    let n = u32::try_from(text.lines().count()).unwrap_or(u32::MAX);
    (line..line.saturating_add(n)).collect()
}

struct ConcatParts(Vec<ConcatPart>);

enum ConcatPart {
    Lit(LitStr),
    Include(LitStr),
}

enum ConcatArg {
    Lit(LitStr),
    Include(LitStr),
}

impl Parse for ConcatArg {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        if input.peek(Lit) {
            return Ok(ConcatArg::Lit(input.parse()?));
        }
        let mac: Macro = input.parse()?;
        if !mac.path.is_ident("include_str") {
            return Err(syn::Error::new(
                mac.bang_token.span,
                "unsupported macro in doc concat",
            ));
        }
        Ok(ConcatArg::Include(parse2(mac.tokens)?))
    }
}

impl Parse for ConcatParts {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let args = Punctuated::<ConcatArg, Token![,]>::parse_terminated(input)?;
        let parts = args
            .into_iter()
            .map(|a| match a {
                ConcatArg::Lit(l) => ConcatPart::Lit(l),
                ConcatArg::Include(l) => ConcatPart::Include(l),
            })
            .collect();
        Ok(ConcatParts(parts))
    }
}

/// Fence blocks of one doc source, classified and positioned.
fn blocks(src: &DocSource, item: &str, root: &str) -> Vec<DocTest> {
    let mut out = Vec::new();
    for f in fence::scan(&src.text) {
        let (counted, info, allow) = classify(&f.info);
        if !counted {
            continue;
        }
        let file = src.files[f.line]
            .strip_prefix(root)
            .and_then(|f| f.strip_prefix('/'))
            .unwrap_or(&src.files[f.line]);
        out.push(DocTest {
            file: file.to_string(),
            line: src.lines[f.line],
            item: item.to_string(),
            info,
            code: f.code,
            allow,
        });
    }
    out
}

/// Counted / allowed / skipped for an info string, plus the recorded
/// doctest attributes.
fn classify(info: &str) -> (bool, Vec<String>, bool) {
    let tokens: Vec<&str> = info
        .split(',')
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .collect();
    let first = tokens.first().copied();
    let is_rust = matches!(
        first,
        None | Some(
            "rust"
                | "no_run"
                | "should_panic"
                | "compile_fail"
                | "edition2015"
                | "edition2018"
                | "edition2021"
                | "edition2024",
        )
    );
    if !is_rust {
        return (false, Vec::new(), false);
    }
    let mut allow = false;
    let mut out = Vec::new();
    for t in tokens {
        match t {
            "rust" | "dejadoc" => allow |= t == "dejadoc",
            "ignore" => return (false, Vec::new(), false),
            _ => out.extend(t.split_whitespace().map(String::from)),
        }
    }
    (true, out, allow)
}

fn item_name(item: &Item) -> Option<String> {
    Some(match item {
        Item::Fn(f) => f.sig.ident.to_string(),
        Item::Const(c) => c.ident.to_string(),
        Item::Static(s) => s.ident.to_string(),
        Item::Enum(e) => e.ident.to_string(),
        Item::Struct(s) => s.ident.to_string(),
        Item::Union(u) => u.ident.to_string(),
        Item::Type(t) => t.ident.to_string(),
        Item::TraitAlias(t) => t.ident.to_string(),
        Item::Use(u) => use_name(&u.tree),
        Item::ExternCrate(e) => e
            .rename
            .as_ref()
            .map_or_else(|| e.ident.to_string(), |(_, i)| i.to_string()),
        Item::Macro(m) => macro_name(m.ident.as_ref(), &m.mac),
        _ => return None,
    })
}

fn macro_name(ident: Option<&Ident>, mac: &Macro) -> String {
    ident.map_or_else(
        || {
            mac.path
                .segments
                .iter()
                .map(|s| s.ident.to_string())
                .collect::<Vec<_>>()
                .join("::")
        },
        ToString::to_string,
    )
}

/// Last path segment of a type, for `Type::name` item paths.
fn type_name(ty: &Type) -> String {
    match ty {
        Type::Path(p) => p
            .path
            .segments
            .last()
            .map(|s| s.ident.to_string())
            .unwrap_or_default(),
        Type::Group(g) => type_name(&g.elem),
        Type::Paren(p) => type_name(&p.elem),
        other => other.to_token_stream().to_string(),
    }
}

/// Local name a `use` tree introduces; groups of names keep their tree form.
fn use_name(tree: &UseTree) -> String {
    match tree {
        UseTree::Path(p) => use_name(&p.tree),
        UseTree::Name(n) => n.ident.to_string(),
        UseTree::Rename(r) => r.rename.to_string(),
        UseTree::Glob(_) => "*".to_string(),
        UseTree::Group(_) => tree.to_token_stream().to_string(),
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    fn run(src: &str) -> Vec<DocTest> {
        let parsed = syn::parse_file(src).unwrap();
        extract(
            "mycrate",
            "/root/src/lib.rs",
            &parsed,
            "/root",
            &|_p: &str| None,
        )
    }

    fn dt(file: &str, line: u32, item: &str, info: &[&str], code: &str, allow: bool) -> DocTest {
        DocTest {
            file: file.to_string(),
            line,
            item: item.into(),
            info: info.iter().map(ToString::to_string).collect(),
            code: code.into(),
            allow,
        }
    }

    #[test]
    fn fn_doctest() {
        let src = "/// ```rust\n/// fn main() { }\n/// ```\npub fn f() {}\n";
        assert_eq!(
            run(src),
            vec![dt(
                "src/lib.rs",
                1,
                "mycrate::f",
                &[],
                "fn main() { }",
                false
            )]
        );
    }

    #[test]
    fn attributes_recorded() {
        let src = "/// ```rust,should_panic\n/// fn main() { panic!(); }\n/// ```\npub fn f() {}\n";
        assert_eq!(
            run(src),
            vec![dt(
                "src/lib.rs",
                1,
                "mycrate::f",
                &["should_panic"],
                "fn main() { panic!(); }",
                false
            )]
        );
    }

    #[test]
    fn nested_module_path() {
        let src = "pub mod a {\n    pub mod b {\n        /// ```\n        /// fn main() {}\n        /// ```\n        pub fn c() {}\n    }\n}\n";
        assert_eq!(
            run(src),
            vec![dt(
                "src/lib.rs",
                3,
                "mycrate::a::b::c",
                &[],
                "fn main() {}",
                false
            )]
        );
    }

    #[test]
    fn file_level_docs() {
        let src = "//! ```\n//! fn main() {}\n//! ```\npub fn f() {}\n";
        assert_eq!(
            run(src),
            vec![dt("src/lib.rs", 1, "mycrate", &[], "fn main() {}", false)]
        );
    }

    #[test]
    fn impl_method_path() {
        let src = "pub struct S;\nimpl S {\n    /// ```rust\n    /// fn main() {}\n    /// ```\n    pub fn m(&self) {}\n}\n";
        assert_eq!(
            run(src),
            vec![dt(
                "src/lib.rs",
                3,
                "mycrate::S::m",
                &[],
                "fn main() {}",
                false
            )]
        );
    }

    #[test]
    fn trait_method_path() {
        let src =
            "pub trait T {\n    /// ```\n    /// fn main() {}\n    /// ```\n    fn m(&self);\n}\n";
        assert_eq!(
            run(src),
            vec![dt(
                "src/lib.rs",
                2,
                "mycrate::T::m",
                &[],
                "fn main() {}",
                false
            )]
        );
    }

    #[test]
    fn extern_block_item() {
        let src = "unsafe extern \"C\" {\n    /// ```\n    /// fn main() {}\n    /// ```\n    fn f();\n}\n";
        assert_eq!(
            run(src),
            vec![dt(
                "src/lib.rs",
                2,
                "mycrate::f",
                &[],
                "fn main() {}",
                false
            )]
        );
    }

    #[test]
    fn macro_rules_site() {
        let src = "/// ```\n/// fn main() {}\n/// ```\nmacro_rules! m { () => {} }\n";
        assert_eq!(
            run(src),
            vec![dt(
                "src/lib.rs",
                1,
                "mycrate::m",
                &[],
                "fn main() {}",
                false
            )]
        );
    }

    #[test]
    fn doc_on_use() {
        let src = "/// ```\n/// fn main() {}\n/// ```\npub use std::vec;\n";
        assert_eq!(
            run(src),
            vec![dt(
                "src/lib.rs",
                1,
                "mycrate::vec",
                &[],
                "fn main() {}",
                false
            )]
        );
    }

    #[test]
    fn doc_on_extern_crate() {
        let src = "/// ```\n/// fn main() {}\n/// ```\nextern crate std;\n";
        assert_eq!(
            run(src),
            vec![dt(
                "src/lib.rs",
                1,
                "mycrate::std",
                &[],
                "fn main() {}",
                false
            )]
        );
    }

    #[test]
    fn ignore_is_skipped() {
        let src = "/// ```ignore\n/// fn main() {}\n/// ```\npub fn f() {}\n";
        assert_eq!(run(src), vec![]);
    }

    #[test]
    fn non_rust_is_skipped() {
        let src = "/// ```text\n/// fn main() {}\n/// ```\npub fn f() {}\n";
        assert_eq!(run(src), vec![]);
    }

    #[test]
    fn dejadoc_token_allows() {
        let src = "/// ```rust,dejadoc\n/// fn main() {}\n/// ```\npub fn f() {}\n";
        assert_eq!(
            run(src),
            vec![dt("src/lib.rs", 1, "mycrate::f", &[], "fn main() {}", true)]
        );
    }

    #[test]
    fn dejadoc_alone_is_not_rust() {
        let src = "/// ```dejadoc\n/// fn main() {}\n/// ```\npub fn f() {}\n";
        assert_eq!(run(src), vec![]);
    }

    #[test]
    fn min_rustc_recorded() {
        let src = "/// ```rust,min rustc 1.45\n/// fn main() {}\n/// ```\npub fn f() {}\n";
        assert_eq!(
            run(src),
            vec![dt(
                "src/lib.rs",
                1,
                "mycrate::f",
                &["min", "rustc", "1.45"],
                "fn main() {}",
                false
            )]
        );
    }

    #[test]
    fn multiple_fences_one_doc() {
        let src = "/// text\n/// ```rust\n/// fn main() {}\n/// ```\n/// more\n/// ```\n/// 1 + 1;\n/// ```\npub fn f() {}\n";
        assert_eq!(
            run(src),
            vec![
                dt("src/lib.rs", 2, "mycrate::f", &[], "fn main() {}", false),
                dt("src/lib.rs", 6, "mycrate::f", &[], "1 + 1;", false),
            ]
        );
    }

    fn fs_read(src: &std::path::Path) -> impl Fn(&str) -> Option<(String, String)> {
        move |p: &str| {
            let path = src.join(p);
            match std::fs::read_to_string(&path) {
                Ok(text) => Some((path.to_string_lossy().into_owned(), text)),
                Err(err) => {
                    std::eprintln!(
                        "dejadoc: warning: cannot read doc include {}: {err}",
                        path.display()
                    );
                    None
                }
            }
        }
    }

    #[test]
    fn include_str_doc() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("src");
        std::fs::create_dir_all(&src).unwrap();
        std::fs::write(
            src.join("doc.md"),
            "See below.\n\n```rust\nlet from = include;\n```\n",
        )
        .unwrap();
        std::fs::write(
            src.join("lib.rs"),
            "#[doc = include_str!(\"doc.md\")]\npub fn f() {}\n",
        )
        .unwrap();
        let code = std::fs::read_to_string(src.join("lib.rs")).unwrap();
        let parsed = syn::parse_file(&code).unwrap();
        let dts = extract(
            "mycrate",
            &src.join("lib.rs").to_string_lossy(),
            &parsed,
            &dir.path().to_string_lossy(),
            &fs_read(&src),
        );
        assert_eq!(
            dts,
            vec![dt(
                "src/doc.md",
                3,
                "mycrate::f",
                &[],
                "let from = include;",
                false
            )]
        );
    }

    #[test]
    fn concat_include_str_doc() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("src");
        std::fs::create_dir_all(&src).unwrap();
        std::fs::write(src.join("doc.md"), "```\nfn main() {}\n```\n").unwrap();
        std::fs::write(
            src.join("lib.rs"),
            "#[doc = concat!(\"prefix \", include_str!(\"doc.md\"))]\npub fn f() {}\n",
        )
        .unwrap();
        let code = std::fs::read_to_string(src.join("lib.rs")).unwrap();
        let parsed = syn::parse_file(&code).unwrap();
        let dts = extract(
            "mycrate",
            &src.join("lib.rs").to_string_lossy(),
            &parsed,
            &dir.path().to_string_lossy(),
            &fs_read(&src),
        );
        assert_eq!(
            dts,
            vec![dt(
                "src/doc.md",
                1,
                "mycrate::f",
                &[],
                "fn main() {}",
                false
            )]
        );
    }

    #[test]
    fn doc_attribute_literal_form() {
        // rustdoc numbers a `#[doc = "…"]` literal's lines from the
        // attribute's line, so a fence at doc line 2 reports line +1.
        let src = r#"#[doc = "intro\n```rust\nfn main() { }\n```\ntail"]
pub fn escaped() {}
"#;
        assert_eq!(
            run(src),
            vec![dt(
                "src/lib.rs",
                2,
                "mycrate::escaped",
                &[],
                "fn main() { }",
                false
            )]
        );
    }

    #[test]
    fn doc_attribute_raw_string() {
        let src = r##"#[doc = r#"raw
intro
```rust
fn main() { }
```"#]
pub fn raw() {}
"##;
        assert_eq!(
            run(src),
            vec![dt(
                "src/lib.rs",
                3,
                "mycrate::raw",
                &[],
                "fn main() { }",
                false
            )]
        );
    }

    #[test]
    fn data_item_sites() {
        let src = "\
/// ```\n/// fn a() {}\n/// ```\npub const C: i32 = 1;\n\
/// ```\n/// fn b() {}\n/// ```\npub static S: i32 = 1;\n\
/// ```\n/// fn c() {}\n/// ```\npub enum E { X }\n\
/// ```\n/// fn d() {}\n/// ```\npub struct St;\n\
/// ```\n/// fn e() {}\n/// ```\npub union U { f: i32 }\n\
/// ```\n/// fn f() {}\n/// ```\npub type Ty = i32;\n\
/// ```\n/// fn g() {}\n/// ```\npub trait Tr = i32;\n";
        assert_eq!(
            run(src),
            vec![
                dt("src/lib.rs", 1, "mycrate::C", &[], "fn a() {}", false),
                dt("src/lib.rs", 5, "mycrate::S", &[], "fn b() {}", false),
                dt("src/lib.rs", 9, "mycrate::E", &[], "fn c() {}", false),
                dt("src/lib.rs", 13, "mycrate::St", &[], "fn d() {}", false),
                dt("src/lib.rs", 17, "mycrate::U", &[], "fn e() {}", false),
                dt("src/lib.rs", 21, "mycrate::Ty", &[], "fn f() {}", false),
                dt("src/lib.rs", 25, "mycrate::Tr", &[], "fn g() {}", false),
            ]
        );
    }

    #[test]
    fn fence_spanning_include() {
        // A fence straddling a `#[doc = include_str!(…)]` part belongs to
        // the item's one doc stream, as rustdoc assembles it.
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("src");
        std::fs::create_dir_all(&src).unwrap();
        std::fs::write(src.join("doc.md"), "fn main() {}").unwrap();
        std::fs::write(
            src.join("lib.rs"),
            "/// ```\n#[doc = include_str!(\"doc.md\")]\n/// ```\npub fn f() {}\n",
        )
        .unwrap();
        let code = std::fs::read_to_string(src.join("lib.rs")).unwrap();
        let parsed = syn::parse_file(&code).unwrap();
        let dts = extract(
            "mycrate",
            &src.join("lib.rs").to_string_lossy(),
            &parsed,
            &dir.path().to_string_lossy(),
            &fs_read(&src),
        );
        assert_eq!(
            dts,
            vec![dt(
                "src/lib.rs",
                1,
                "mycrate::f",
                &[],
                "fn main() {}",
                false
            )]
        );
    }

    #[test]
    fn newline_terminated_doc_part_keeps_line_mapping() {
        // A doc part ending in a newline plus the merge separator yields
        // one extra text line; the line mapping must cover it, since
        // fences are indexed by text line.
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("src");
        std::fs::create_dir_all(&src).unwrap();
        std::fs::write(
            src.join("lib.rs"),
            "#[doc = \"x\\n\"]\n#[doc = \"a\\n```rust\"]\npub fn f() {}\n",
        )
        .unwrap();
        let code = std::fs::read_to_string(src.join("lib.rs")).unwrap();
        let parsed = syn::parse_file(&code).unwrap();
        let dts = extract(
            "mycrate",
            &src.join("lib.rs").to_string_lossy(),
            &parsed,
            &dir.path().to_string_lossy(),
            &|_p: &str| None,
        );
        assert_eq!(dts, vec![dt("src/lib.rs", 3, "mycrate::f", &[], "", false)]);
    }

    #[test]
    fn empty_first_doc_part_keeps_line_mapping() {
        // A bare `///` line is a zero-line doc part; the merge separator
        // after it still adds one text line, which must stay mapped.
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("src");
        std::fs::create_dir_all(&src).unwrap();
        std::fs::write(
            src.join("lib.rs"),
            "///\n/// text\n/// ```rust\npub fn f() {}\n",
        )
        .unwrap();
        let code = std::fs::read_to_string(src.join("lib.rs")).unwrap();
        let parsed = syn::parse_file(&code).unwrap();
        let dts = extract(
            "mycrate",
            &src.join("lib.rs").to_string_lossy(),
            &parsed,
            &dir.path().to_string_lossy(),
            &|_p: &str| None,
        );
        assert_eq!(dts, vec![dt("src/lib.rs", 3, "mycrate::f", &[], "", false)]);
    }

    #[test]
    fn impl_associated_items_carry_doctests() {
        let src = "struct S;\nimpl S {\n    /// ```\n    /// fn main() {}\n    /// ```\n    const C: i32 = 1;\n    /// ```\n    /// fn main() { let a = 1; }\n    /// ```\n    type T = i32;\n}\n";
        assert_eq!(
            run(src),
            vec![
                dt("src/lib.rs", 3, "mycrate::S::C", &[], "fn main() {}", false),
                dt(
                    "src/lib.rs",
                    7,
                    "mycrate::S::T",
                    &[],
                    "fn main() { let a = 1; }",
                    false
                )
            ]
        );
    }

    #[test]
    fn trait_associated_items_carry_doctests() {
        let src = "trait T {\n    /// ```\n    /// fn main() {}\n    /// ```\n    const C: i32;\n    /// ```\n    /// fn main() { let a = 1; }\n    /// ```\n    type Out;\n}\n";
        assert_eq!(
            run(src),
            vec![
                dt("src/lib.rs", 2, "mycrate::T::C", &[], "fn main() {}", false),
                dt(
                    "src/lib.rs",
                    6,
                    "mycrate::T::Out",
                    &[],
                    "fn main() { let a = 1; }",
                    false
                )
            ]
        );
    }

    #[test]
    fn extern_associated_items_carry_doctests() {
        let src = "extern \"C\" {\n    /// ```\n    /// fn main() {}\n    /// ```\n    static S: i32;\n    /// ```\n    /// fn main() { let a = 1; }\n    /// ```\n    type T;\n}\n";
        assert_eq!(
            run(src),
            vec![
                dt("src/lib.rs", 2, "mycrate::S", &[], "fn main() {}", false),
                dt(
                    "src/lib.rs",
                    6,
                    "mycrate::T",
                    &[],
                    "fn main() { let a = 1; }",
                    false
                )
            ]
        );
    }

    #[test]
    fn mod_declaration_carries_its_docs() {
        let src = "/// ```\n/// fn main() {}\n/// ```\npub mod m {\n    pub fn f() {}\n}\n";
        assert_eq!(
            run(src),
            vec![dt(
                "src/lib.rs",
                1,
                "mycrate::m",
                &[],
                "fn main() {}",
                false
            )]
        );
    }

    #[test]
    fn impl_declaration_carries_its_docs() {
        let src = "struct S;\n/// ```\n/// fn main() {}\n/// ```\nimpl S {\n    pub fn f() {}\n}\n";
        assert_eq!(
            run(src),
            vec![dt(
                "src/lib.rs",
                2,
                "mycrate::S",
                &[],
                "fn main() {}",
                false
            )]
        );
    }

    #[test]
    fn trait_declaration_carries_its_docs() {
        let src = "/// ```\n/// fn main() {}\n/// ```\ntrait T {\n    fn m(&self);\n}\n";
        assert_eq!(
            run(src),
            vec![dt(
                "src/lib.rs",
                1,
                "mycrate::T",
                &[],
                "fn main() {}",
                false
            )]
        );
    }

    #[test]
    fn extern_block_carries_its_docs() {
        let src = "/// ```\n/// fn main() {}\n/// ```\nextern \"C\" {\n    fn f();\n}\n";
        assert_eq!(
            run(src),
            vec![dt("src/lib.rs", 1, "mycrate", &[], "fn main() {}", false)]
        );
    }

    #[test]
    fn rust_ignore_fence_is_not_scanned() {
        let src = "/// ```rust,ignore\n/// fn main() {}\n/// ```\npub fn f() {}\n";
        assert_eq!(run(src), Vec::new());
    }
}
