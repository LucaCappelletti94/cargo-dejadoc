//! Function sites, compared within their module.

use alloc::boxed::Box;
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use syn::spanned::Spanned;

use crate::{DocTest, SourceFile};

/// One function as found in a source file.
pub(crate) struct FnSite {
    /// The reported site.
    pub(crate) site: DocTest,
    /// Crate root file and module path, since two crates may share a target name.
    pub(crate) scope: String,
    /// The function as compared, without its visibility and inert attributes.
    pub(crate) func: syn::ItemFn,
}

/// The free functions, impl methods and trait default methods of `file`, nested functions excluded.
pub(crate) fn functions(
    prefix: &str,
    file: &SourceFile,
    root: &str,
    crate_root: &str,
) -> Vec<FnSite> {
    let path = file
        .path
        .strip_prefix(root)
        .and_then(|p| p.strip_prefix('/'))
        .unwrap_or(&file.path);
    let lines: Vec<&str> = file.text.lines().collect();
    let mut walk = Walk {
        path,
        crate_root,
        lines: &lines,
        out: Vec::new(),
    };
    walk.items(&file.parsed.items, prefix);
    walk.out
}

/// The walk over one source file.
struct Walk<'a> {
    path: &'a str,
    crate_root: &'a str,
    lines: &'a [&'a str],
    out: Vec<FnSite>,
}

impl Walk<'_> {
    fn items(&mut self, items: &[syn::Item], module: &str) {
        for item in items {
            match item {
                syn::Item::Fn(f) => self.push(module, None, f, &f.attrs, &f.sig, &f.block),
                syn::Item::Mod(m) => {
                    if let Some((_, inner)) = &m.content {
                        self.items(inner, &format!("{module}::{}", m.ident));
                    }
                }
                syn::Item::Impl(i) => {
                    let self_type = type_text(&i.self_ty);
                    for member in &i.items {
                        if let syn::ImplItem::Fn(f) = member {
                            self.push(module, Some(&self_type), f, &f.attrs, &f.sig, &f.block);
                        }
                    }
                }
                syn::Item::Trait(t) => {
                    let self_type = t.ident.to_string();
                    for member in &t.items {
                        if let syn::TraitItem::Fn(f) = member
                            && let Some(block) = &f.default
                        {
                            self.push(module, Some(&self_type), f, &f.attrs, &f.sig, block);
                        }
                    }
                }
                _ => {}
            }
        }
    }

    /// Record the function `whole`, attributes and doc comments included.
    fn push(
        &mut self,
        module: &str,
        self_type: Option<&str>,
        whole: &impl Spanned,
        attrs: &[syn::Attribute],
        sig: &syn::Signature,
        block: &syn::Block,
    ) {
        let span = whole.span();
        let (line, end) = (span.start().line, span.end().line);
        let allow = line
            .checked_sub(2)
            .and_then(|above| self.lines.get(above))
            .is_some_and(|above| above.trim() == "// dejadoc: allow");
        let item = match self_type {
            Some(ty) => format!("{module}::{ty}::{}", sig.ident),
            None => format!("{module}::{}", sig.ident),
        };
        let code = self
            .lines
            .get(line.saturating_sub(1)..end.min(self.lines.len()))
            .unwrap_or_default()
            .join("\n");
        self.out.push(FnSite {
            site: DocTest {
                file: self.path.to_string(),
                line: u32::try_from(line).unwrap_or(u32::MAX),
                end: u32::try_from(end).ok(),
                item,
                info: Vec::new(),
                code,
                allow,
                self_type: self_type.map(ToString::to_string),
            },
            scope: format!("{}\n{module}", self.crate_root),
            func: syn::ItemFn {
                attrs: attrs.iter().filter(|a| compared(a)).cloned().collect(),
                vis: syn::Visibility::Inherited,
                modifiers: syn::FnModifiers::default(),
                sig: sig.clone(),
                block: Box::new(block.clone()),
            },
        });
    }
}

/// Whether an attribute takes part in the comparison, `syn-canon` dropping `doc`, lint levels and `rustfmt::` itself.
fn compared(attr: &syn::Attribute) -> bool {
    ![
        "cfg",
        "cfg_attr",
        "inline",
        "cold",
        "must_use",
        "track_caller",
    ]
    .iter()
    .any(|name| attr.path().is_ident(name))
}

/// The whole type as written, `Foo<u8>` and `a::Foo` telling apart types one last segment would merge.
fn type_text(ty: &syn::Type) -> String {
    let printed = quote::ToTokens::to_token_stream(ty).to_string();
    let word = |c: Option<char>| c.is_some_and(|c| c.is_alphanumeric() || c == '_');
    let mut out = String::with_capacity(printed.len());
    let mut chars = printed.chars().peekable();
    while let Some(c) = chars.next() {
        if c != ' ' || (word(out.chars().last()) && word(chars.peek().copied())) {
            out.push(c);
        }
    }
    out
}
