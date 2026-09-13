//! Doc comment walking and doctest block extraction.

use std::path::{Path, PathBuf};

use quote::ToTokens;
use syn::{
    AttrStyle, Expr, ExprLit, ExprMacro, ForeignItem, Ident, Item, Lit, LitStr, Macro, Meta, Token,
    TraitItem, Type, UseTree,
    parse::{Parse, ParseStream},
    parse2,
    punctuated::Punctuated,
};

use crate::DocTest;
use crate::discover::Target;
use crate::fence;

/// A run of doc text with per-line source positions.
struct DocSource {
    text: String,
    /// 1-based source line of each text line; one entry per text line.
    lines: Vec<u32>,
    /// Absolute path of the file the text lives in.
    file: PathBuf,
}

/// Extract doctest blocks from one parsed source file.
pub fn extract(target: &Target, file: &Path, parsed: &syn::File, root: &Path) -> Vec<DocTest> {
    let mut out = Vec::new();
    // File-level `//!` docs: inner doc attributes only.
    let inner: Vec<syn::Attribute> = parsed
        .attrs
        .iter()
        .filter(|a| matches!(a.style, AttrStyle::Inner { .. }))
        .cloned()
        .collect();
    for src in doc_sources(&inner, file) {
        out.extend(blocks(&src, &target.name, root));
    }
    for item in &parsed.items {
        walk_item(item, &target.name, file, root, &mut out);
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

fn walk_item(item: &Item, prefix: &str, file: &Path, root: &Path, out: &mut Vec<DocTest>) {
    match item {
        Item::Mod(m) => {
            let sub = format!("{prefix}::{}", m.ident);
            // `m.attrs` also holds the mod body's inner `//!` docs.
            push_doc(item_attrs(item), &sub, file, root, out);
            if let Some((_, items)) = &m.content {
                for child in items {
                    walk_item(child, &sub, file, root, out);
                }
            }
        }
        Item::Impl(i) => {
            let sub = format!("{prefix}::{}", type_name(&i.self_ty));
            push_doc(item_attrs(item), &sub, file, root, out);
            for assoc in &i.items {
                if let Some(name) = impl_assoc_name(assoc) {
                    push_doc(
                        assoc_attrs(assoc),
                        &format!("{sub}::{name}"),
                        file,
                        root,
                        out,
                    );
                }
            }
        }
        Item::Trait(t) => {
            let sub = format!("{prefix}::{}", t.ident);
            push_doc(item_attrs(item), &sub, file, root, out);
            for assoc in &t.items {
                if let Some(name) = trait_assoc_name(assoc) {
                    push_doc(
                        trait_assoc_attrs(assoc),
                        &format!("{sub}::{name}"),
                        file,
                        root,
                        out,
                    );
                }
            }
        }
        Item::ForeignMod(f) => {
            push_doc(item_attrs(item), prefix, file, root, out);
            for foreign in &f.items {
                if let Some(name) = foreign_name(foreign) {
                    push_doc(
                        foreign_attrs(foreign),
                        &format!("{prefix}::{name}"),
                        file,
                        root,
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
    // Spans count file lines; multi-gigabyte files do not occur.
    debug_assert!(line <= u32::MAX as usize, "span line overflows u32");
    line as u32
}

fn push_doc(
    attrs: &[syn::Attribute],
    item: &str,
    file: &Path,
    root: &Path,
    out: &mut Vec<DocTest>,
) {
    for src in doc_sources(attrs, file) {
        out.extend(blocks(&src, item, root));
    }
}

/// Doc text sources from an item's attributes: `#[doc = "…"]` literals,
/// `include_str!` files, and `concat!` mixes of both. Consecutive text from
/// the same file merges into one source, as rustdoc does.
fn doc_sources(attrs: &[syn::Attribute], file: &Path) -> Vec<DocSource> {
    let mut parts: Vec<(String, u32, PathBuf)> = Vec::new();
    for attr in attrs.iter().filter(|a| a.path().is_ident("doc")) {
        let line = line_of(attr.pound_token.span);
        let Meta::NameValue(nv) = &attr.meta else {
            continue;
        };
        match &nv.value {
            Expr::Lit(ExprLit {
                lit: Lit::Str(s), ..
            }) => parts.push((strip_doc_spaces(&s.value()), line, file.to_path_buf())),
            Expr::Macro(ExprMacro { mac, .. }) if mac.path.is_ident("include_str") => {
                if let Ok(s) = parse2::<LitStr>(mac.tokens.clone()) {
                    push_include(&mut parts, file, &s.value());
                }
            }
            Expr::Macro(ExprMacro { mac, .. }) if mac.path.is_ident("concat") => {
                if let Ok(concat) = parse2::<ConcatParts>(mac.tokens.clone()) {
                    for part in concat.0 {
                        match part {
                            ConcatPart::Lit(s) => {
                                parts.push((strip_doc_spaces(&s.value()), line, file.to_path_buf()))
                            }
                            ConcatPart::Include(s) => push_include(&mut parts, file, &s.value()),
                        }
                    }
                }
            }
            _ => {}
        }
    }
    merge_parts(parts)
}

fn push_include(parts: &mut Vec<(String, u32, PathBuf)>, file: &Path, p: &str) {
    let path = match file.parent() {
        Some(parent) => parent.join(p),
        None => PathBuf::from(p),
    };
    match std::fs::read_to_string(&path) {
        Ok(text) => parts.push((text, 1, path)),
        Err(err) => eprintln!(
            "dejadoc: warning: cannot read doc include {:?}: {err}",
            path
        ),
    }
}

/// Merge consecutive parts sharing a file into single sources.
fn merge_parts(parts: Vec<(String, u32, PathBuf)>) -> Vec<DocSource> {
    let mut out: Vec<DocSource> = Vec::new();
    for (text, line, file) in parts {
        let lines = line_range(&text, line);
        if out.last().is_some_and(|src| src.file == file) {
            let last = out.last_mut().unwrap();
            last.text.push('\n');
            last.text.push_str(&text);
            last.lines.extend(lines);
        } else {
            out.push(DocSource { text, lines, file });
        }
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
    // `str::count` is gone on this toolchain; `matches` counts the same way.
    let n = text.matches('\n').count() + 1;
    debug_assert!(n <= u32::MAX as usize, "doc text exceeds u32 lines");
    (line..line.saturating_add(n as u32)).collect()
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
fn blocks(src: &DocSource, item: &str, root: &Path) -> Vec<DocTest> {
    let mut out = Vec::new();
    for f in fence::scan(&src.text) {
        let (counted, info, allow) = classify(&f.info);
        if !counted {
            continue;
        }
        let file = src
            .file
            .strip_prefix(root)
            .unwrap_or(&src.file)
            .to_path_buf();
        out.push(DocTest {
            file,
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
            .map(|(_, i)| i.to_string())
            .unwrap_or_else(|| e.ident.to_string()),
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
        |i| i.to_string(),
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
    use std::path::PathBuf;

    fn run(src: &str) -> Vec<DocTest> {
        let parsed = syn::parse_file(src).unwrap();
        let target = Target {
            name: "mycrate".into(),
            kind: crate::discover::TargetKind::Lib,
            src: PathBuf::from("/root/src/lib.rs"),
        };
        extract(
            &target,
            Path::new("/root/src/lib.rs"),
            &parsed,
            Path::new("/root"),
        )
    }

    fn dt(file: &str, line: u32, item: &str, info: &[&str], code: &str, allow: bool) -> DocTest {
        DocTest {
            file: PathBuf::from(file),
            line,
            item: item.into(),
            info: info.iter().map(|s| s.to_string()).collect(),
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
        let target = Target {
            name: "mycrate".into(),
            kind: crate::discover::TargetKind::Lib,
            src: src.join("lib.rs"),
        };
        let dts = extract(&target, &src.join("lib.rs"), &parsed, dir.path());
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
        let target = Target {
            name: "mycrate".into(),
            kind: crate::discover::TargetKind::Lib,
            src: src.join("lib.rs"),
        };
        let dts = extract(&target, &src.join("lib.rs"), &parsed, dir.path());
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
}
