//! Formatting drift rustfmt introduces, folded before hashing.

use alloc::boxed::Box;
use alloc::format;
use alloc::string::ToString;
use alloc::vec::Vec;

use proc_macro2::{Delimiter, Group, TokenStream, TokenTree};
use quote::ToTokens;
use syn::visit_mut::VisitMut;

/// Drop the trailing comma of every group, keeping the one that makes
/// a one-tuple.
pub(crate) fn strip_trailing_commas(stream: TokenStream) -> TokenStream {
    let mut out = Vec::new();
    let mut after_callee = false;
    for tree in stream {
        let tree = match tree {
            TokenTree::Group(group) => {
                let call = after_callee && group.delimiter() == Delimiter::Parenthesis;
                let inner = strip_last_comma(strip_trailing_commas(group.stream()), &group, call);
                let mut rebuilt = Group::new(group.delimiter(), inner);
                rebuilt.set_span(group.span());
                after_callee = true;
                TokenTree::Group(rebuilt)
            }
            TokenTree::Ident(ident) => {
                after_callee = !is_keyword(&ident.to_string());
                TokenTree::Ident(ident)
            }
            other => {
                after_callee = false;
                other
            }
        };
        out.push(tree);
    }
    out.into_iter().collect()
}

/// True for the Rust keywords, none of which is a callee.
fn is_keyword(ident: &str) -> bool {
    matches!(
        ident,
        "as" | "async"
            | "await"
            | "break"
            | "const"
            | "continue"
            | "crate"
            | "dyn"
            | "else"
            | "enum"
            | "extern"
            | "false"
            | "fn"
            | "for"
            | "if"
            | "impl"
            | "in"
            | "let"
            | "loop"
            | "match"
            | "mod"
            | "move"
            | "mut"
            | "pub"
            | "ref"
            | "return"
            | "self"
            | "Self"
            | "static"
            | "struct"
            | "super"
            | "trait"
            | "true"
            | "type"
            | "unsafe"
            | "use"
            | "where"
            | "while"
            | "yield"
    )
}

/// `stream` without its last comma, unless the parenthesised group is a
/// one-tuple rather than a single-argument call.
fn strip_last_comma(stream: TokenStream, group: &Group, call: bool) -> TokenStream {
    let mut tokens: Vec<TokenTree> = stream.into_iter().collect();
    let commas = tokens.iter().filter(|tree| is_comma(tree)).count();
    let one_tuple = group.delimiter() == Delimiter::Parenthesis && !call && commas == 1;
    if tokens.last().is_some_and(is_comma) && !one_tuple {
        tokens.pop();
    }
    tokens.into_iter().collect()
}

fn is_comma(tree: &TokenTree) -> bool {
    matches!(tree, TokenTree::Punct(p) if p.as_char() == ',')
}

/// Fold arm braces, doc attributes, and `use` shapes in place.
pub(crate) fn normalize_file(file: &mut syn::File) {
    Drift.visit_file_mut(file);
}

struct Drift;

impl VisitMut for Drift {
    #[expect(
        clippy::result_large_err,
        reason = "a non-use entry goes back to the list"
    )]
    fn visit_file_mut(&mut self, file: &mut syn::File) {
        hoist_uses(
            &mut file.items,
            |item| match item {
                syn::Item::Use(u) => Ok(u),
                other => Err(other),
            },
            syn::Item::Use,
        );
        syn::visit_mut::visit_file_mut(self, file);
    }

    #[expect(
        clippy::result_large_err,
        reason = "a non-use entry goes back to the list"
    )]
    fn visit_block_mut(&mut self, block: &mut syn::Block) {
        hoist_uses(
            &mut block.stmts,
            |stmt| match stmt {
                syn::Stmt::Item(syn::Item::Use(u)) => Ok(u),
                other => Err(other),
            },
            |u| syn::Stmt::Item(syn::Item::Use(u)),
        );
        syn::visit_mut::visit_block_mut(self, block);
    }

    fn visit_arm_mut(&mut self, arm: &mut syn::Arm) {
        unwrap_arm_block(arm);
        syn::visit_mut::visit_arm_mut(self, arm);
    }

    fn visit_stmt_mut(&mut self, stmt: &mut syn::Stmt) {
        match stmt {
            syn::Stmt::Macro(v) => strip_doc_attrs(&mut v.attrs),
            syn::Stmt::Local(v) => strip_doc_attrs(&mut v.attrs),
            _ => {}
        }
        syn::visit_mut::visit_stmt_mut(self, stmt);
    }

    fn visit_item_mut(&mut self, item: &mut syn::Item) {
        if let Some(attrs) = item_attrs(item) {
            strip_doc_attrs(attrs);
        }
        syn::visit_mut::visit_item_mut(self, item);
    }

    fn visit_impl_item_mut(&mut self, item: &mut syn::ImplItem) {
        match item {
            syn::ImplItem::Const(v) => strip_doc_attrs(&mut v.attrs),
            syn::ImplItem::Fn(v) => strip_doc_attrs(&mut v.attrs),
            syn::ImplItem::Type(v) => strip_doc_attrs(&mut v.attrs),
            syn::ImplItem::Macro(v) => strip_doc_attrs(&mut v.attrs),
            _ => {}
        }
        syn::visit_mut::visit_impl_item_mut(self, item);
    }

    fn visit_trait_item_mut(&mut self, item: &mut syn::TraitItem) {
        match item {
            syn::TraitItem::Const(v) => strip_doc_attrs(&mut v.attrs),
            syn::TraitItem::Fn(v) => strip_doc_attrs(&mut v.attrs),
            syn::TraitItem::Type(v) => strip_doc_attrs(&mut v.attrs),
            syn::TraitItem::Macro(v) => strip_doc_attrs(&mut v.attrs),
            _ => {}
        }
        syn::visit_mut::visit_trait_item_mut(self, item);
    }

    fn visit_field_mut(&mut self, field: &mut syn::Field) {
        strip_doc_attrs(&mut field.attrs);
        syn::visit_mut::visit_field_mut(self, field);
    }

    fn visit_variant_mut(&mut self, variant: &mut syn::Variant) {
        strip_doc_attrs(&mut variant.attrs);
        syn::visit_mut::visit_variant_mut(self, variant);
    }
}

/// An arm body `{ expr }` becomes `expr`, the form rustfmt writes.
fn unwrap_arm_block(arm: &mut syn::Arm) {
    let syn::Expr::Block(block) = arm.body.as_mut() else {
        return;
    };
    if block.label.is_some()
        || !block.attrs.is_empty()
        || !matches!(block.block.stmts.as_slice(), [syn::Stmt::Expr(_, None)])
    {
        return;
    }
    if let Some(syn::Stmt::Expr(expr, None)) = block.block.stmts.pop() {
        *arm.body = expr;
    }
}

fn strip_doc_attrs(attrs: &mut Vec<syn::Attribute>) {
    attrs.retain(|attr| !attr.path().is_ident("doc"));
}

fn item_attrs(item: &mut syn::Item) -> Option<&mut Vec<syn::Attribute>> {
    Some(match item {
        syn::Item::Const(v) => &mut v.attrs,
        syn::Item::Enum(v) => &mut v.attrs,
        syn::Item::ExternCrate(v) => &mut v.attrs,
        syn::Item::Fn(v) => &mut v.attrs,
        syn::Item::ForeignMod(v) => &mut v.attrs,
        syn::Item::Impl(v) => &mut v.attrs,
        syn::Item::Macro(v) => &mut v.attrs,
        syn::Item::Mod(v) => &mut v.attrs,
        syn::Item::Static(v) => &mut v.attrs,
        syn::Item::Struct(v) => &mut v.attrs,
        syn::Item::Trait(v) => &mut v.attrs,
        syn::Item::TraitAlias(v) => &mut v.attrs,
        syn::Item::Type(v) => &mut v.attrs,
        syn::Item::Union(v) => &mut v.attrs,
        syn::Item::Use(v) => &mut v.attrs,
        _ => return None,
    })
}

/// Replace the `use` entries of `list` by one sorted item per leaf path,
/// placed at the front.
fn hoist_uses<T>(
    list: &mut Vec<T>,
    into_use: impl Fn(T) -> Result<syn::ItemUse, T>,
    wrap: impl Fn(syn::ItemUse) -> T,
) {
    let mut leaves = Vec::new();
    let rest: Vec<T> = core::mem::take(list)
        .into_iter()
        .filter_map(|entry| {
            let item = match into_use(entry) {
                Ok(item) => item,
                Err(entry) => return Some(entry),
            };
            let mut trees = Vec::new();
            flatten_use_tree(&[], item.tree, &mut trees);
            leaves.extend(trees.into_iter().map(|tree| syn::ItemUse {
                attrs: item.attrs.clone(),
                vis: item.vis.clone(),
                use_token: item.use_token,
                leading_colon: item.leading_colon,
                tree,
                semi_token: item.semi_token,
            }));
            None
        })
        .collect();
    leaves.sort_by_cached_key(|leaf| {
        format!(
            "{} {}",
            leaf.vis.to_token_stream(),
            leaf.tree.to_token_stream()
        )
    });
    list.extend(leaves.into_iter().map(wrap));
    list.extend(rest);
}

/// Split `tree` into its leaf paths, `a::{self}` becoming `a`.
fn flatten_use_tree(prefix: &[syn::Ident], tree: syn::UseTree, out: &mut Vec<syn::UseTree>) {
    match tree {
        syn::UseTree::Path(path) => {
            let mut prefix = prefix.to_vec();
            prefix.push(path.ident);
            flatten_use_tree(&prefix, *path.tree, out);
        }
        syn::UseTree::Group(group) => {
            for item in group.items {
                flatten_use_tree(prefix, item, out);
            }
        }
        syn::UseTree::Name(name) if name.ident == "self" => {
            if let [head @ .., last] = prefix {
                let leaf = syn::UseTree::Name(syn::UseName {
                    ident: last.clone(),
                });
                out.push(path_tree(head, leaf));
            }
        }
        leaf => out.push(path_tree(prefix, leaf)),
    }
}

fn path_tree(prefix: &[syn::Ident], leaf: syn::UseTree) -> syn::UseTree {
    match prefix {
        [] => leaf,
        [head, rest @ ..] => syn::UseTree::Path(syn::UsePath {
            ident: head.clone(),
            colon2_token: syn::token::PathSep::default(),
            tree: Box::new(path_tree(rest, leaf)),
        }),
    }
}
