//! Function sites, compared within their module.

use alloc::boxed::Box;
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use syn::spanned::Spanned;

use crate::{DocTest, Remedy, SourceFile};

/// One function as found in a source file.
pub(crate) struct FnSite {
    /// The reported site.
    pub(crate) site: DocTest,
    /// Crate root file and module path, since two crates may share a target name.
    pub(crate) scope: String,
    /// The function as compared, without its visibility and inert attributes.
    pub(crate) func: syn::ItemFn,
    /// What decides whether a copy can be deleted.
    pub(crate) context: Context,
}

/// Where a function is defined, whether it exports a symbol, and under which `cfg`, all of
/// which the comparison ignores.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Context {
    owner: Owner,
    exported: bool,
    /// The printed `cfg` attributes, sorted, since their order carries no meaning.
    cfg: Vec<String>,
}

/// What holds a function.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Owner {
    /// A module.
    Free,
    /// An inherent impl.
    Inherent,
    /// A trait impl or a trait's default methods.
    Trait,
}

/// The remedy of a group's copies, from the first matching row of the table in the spec.
pub(crate) fn remedy(sites: &[(DocTest, Context)]) -> Remedy {
    let Some(((first, first_context), rest)) = sites.split_first() else {
        return Remedy::Delete;
    };
    if rest
        .iter()
        .any(|(site, _)| site.self_type != first.self_type)
    {
        Remedy::GenericOrMacro
    } else if sites
        .iter()
        .any(|(_, context)| context.owner == Owner::Trait || context.exported)
    {
        Remedy::HelperOrMacro
    } else if rest
        .iter()
        .any(|(_, context)| context.cfg != first_context.cfg)
    {
        Remedy::MergeCfg
    } else {
        Remedy::Delete
    }
}

/// The free functions, impl methods and trait default methods of `file`, nested functions excluded.
pub(crate) fn functions(
    prefix: &str,
    file: &SourceFile,
    root: &str,
    crate_root: &str,
    library: bool,
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
        library,
        lines: &lines,
        cfg: Vec::new(),
        out: Vec::new(),
    };
    walk.items(&file.parsed.items, prefix);
    walk.out
}

/// The walk over one source file.
struct Walk<'a> {
    path: &'a str,
    crate_root: &'a str,
    /// Whether the file belongs to a library, whose `pub` functions are public API.
    library: bool,
    lines: &'a [&'a str],
    /// The `cfg` attributes of the inline modules, impls and traits the walk is inside.
    cfg: Vec<String>,
    out: Vec<FnSite>,
}

impl Walk<'_> {
    fn items(&mut self, items: &[syn::Item], module: &str) {
        for item in items {
            match item {
                syn::Item::Fn(f) => {
                    self.push(
                        module,
                        None,
                        f,
                        Parts {
                            attrs: &f.attrs,
                            vis: &f.vis,
                            sig: &f.sig,
                            block: &f.block,
                        },
                    );
                }
                syn::Item::Mod(m) => {
                    if let Some((_, inner)) = &m.content {
                        let depth = self.enter(&m.attrs);
                        self.items(inner, &format!("{module}::{}", m.ident));
                        self.cfg.truncate(depth);
                    }
                }
                syn::Item::Impl(i) => {
                    let self_type = type_text(&i.self_ty);
                    let owner = if i.trait_.is_some() {
                        Owner::Trait
                    } else {
                        Owner::Inherent
                    };
                    let depth = self.enter(&i.attrs);
                    for member in &i.items {
                        if let syn::ImplItem::Fn(f) = member {
                            self.push(
                                module,
                                Some((&self_type, owner)),
                                f,
                                Parts {
                                    attrs: &f.attrs,
                                    vis: &f.vis,
                                    sig: &f.sig,
                                    block: &f.block,
                                },
                            );
                        }
                    }
                    self.cfg.truncate(depth);
                }
                syn::Item::Trait(t) => {
                    let self_type = t.ident.to_string();
                    let inherited = syn::Visibility::Inherited;
                    let depth = self.enter(&t.attrs);
                    for member in &t.items {
                        if let syn::TraitItem::Fn(f) = member
                            && let Some(block) = &f.default
                        {
                            self.push(
                                module,
                                Some((&self_type, Owner::Trait)),
                                f,
                                Parts {
                                    attrs: &f.attrs,
                                    vis: &inherited,
                                    sig: &f.sig,
                                    block,
                                },
                            );
                        }
                    }
                    self.cfg.truncate(depth);
                }
                _ => {}
            }
        }
    }

    /// Push the `cfg` attributes of an item the walk enters, returning the depth to restore.
    fn enter(&mut self, attrs: &[syn::Attribute]) -> usize {
        let depth = self.cfg.len();
        self.cfg.extend(cfgs(attrs));
        depth
    }

    /// Record the function `whole`, attributes and doc comments included, `method` holding
    /// the self type and owner of a method.
    fn push(
        &mut self,
        module: &str,
        method: Option<(&str, Owner)>,
        whole: &impl Spanned,
        parts: Parts<'_>,
    ) {
        let Parts {
            attrs,
            vis,
            sig,
            block,
        } = parts;
        let (self_type, owner) = match method {
            Some((self_type, owner)) => (Some(self_type), owner),
            None => (None, Owner::Free),
        };
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
        let mut cfg = self.cfg.clone();
        cfg.extend(cfgs(attrs));
        cfg.sort_unstable();
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
                public: self.library && matches!(vis, syn::Visibility::Public(_)),
            },
            scope: format!("{}\n{module}", self.crate_root),
            func: syn::ItemFn {
                attrs: attrs.iter().filter(|a| live(&a.meta)).cloned().collect(),
                vis: syn::Visibility::Inherited,
                modifiers: syn::FnModifiers::default(),
                sig: sig.clone(),
                block: Box::new(block.clone()),
            },
            context: Context {
                owner,
                exported: attrs.iter().any(|a| exports(&a.meta)),
                cfg,
            },
        });
    }
}

/// The pieces of a function the walk records.
#[derive(Clone, Copy)]
struct Parts<'a> {
    attrs: &'a [syn::Attribute],
    vis: &'a syn::Visibility,
    sig: &'a syn::Signature,
    block: &'a syn::Block,
}

/// The printed `cfg` attributes among `attrs`.
fn cfgs(attrs: &[syn::Attribute]) -> impl Iterator<Item = String> + '_ {
    attrs
        .iter()
        .filter(|a| a.path().is_ident("cfg"))
        .map(|a| quote::ToTokens::to_token_stream(a).to_string())
}

/// Whether `meta` takes part in the comparison. `cfg`, the hints that leave the body's
/// meaning alone, the attributes that export a symbol, and a `cfg_attr` wrapping only those
/// do not.
fn live(meta: &syn::Meta) -> bool {
    let path = meta.path();
    if path.is_ident("cfg_attr") {
        return wrapped(meta, 1).is_none_or(|metas| metas.iter().any(live));
    }
    if path.is_ident("unsafe") {
        return wrapped(meta, 0).is_none_or(|metas| metas.iter().any(live));
    }
    let inert = [
        "cfg",
        "inline",
        "cold",
        "must_use",
        "track_caller",
        "doc",
        "allow",
        "warn",
        "deny",
        "forbid",
        "expect",
        "no_mangle",
        "export_name",
    ];
    !inert.iter().any(|name| path.is_ident(name))
        && path.segments.first().is_none_or(|s| s.ident != "rustfmt")
}

/// Whether `meta` exports a symbol, which the symbol's users then need.
fn exports(meta: &syn::Meta) -> bool {
    let path = meta.path();
    if path.is_ident("unsafe") {
        return wrapped(meta, 0).is_some_and(|metas| metas.iter().any(exports));
    }
    path.is_ident("no_mangle") || path.is_ident("export_name")
}

/// The metas `meta` wraps after its first `skip` arguments, `None` when they don't parse.
fn wrapped(meta: &syn::Meta, skip: usize) -> Option<Vec<syn::Meta>> {
    let syn::Meta::List(list) = meta else {
        return None;
    };
    let parser = syn::punctuated::Punctuated::<syn::Meta, syn::Token![,]>::parse_terminated;
    list.parse_args_with(parser)
        .ok()
        .map(|metas| metas.into_iter().skip(skip).collect())
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
