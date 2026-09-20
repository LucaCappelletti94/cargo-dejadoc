//! Formatting drift rustfmt introduces, folded before hashing.

use alloc::boxed::Box;
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use proc_macro2::{Group, TokenStream, TokenTree};
use quote::ToTokens;
use syn::visit_mut::VisitMut;

/// Drop the trailing comma of every token group. A one-tuple keeps its
/// meaning because the fold of redundant parentheses already separates
/// it from the parenthesised expression of the same element.
pub(crate) fn strip_trailing_commas(stream: TokenStream) -> TokenStream {
    stream
        .into_iter()
        .map(|tree| match tree {
            TokenTree::Group(group) => {
                let inner = strip_last_comma(strip_trailing_commas(group.stream()));
                let mut rebuilt = Group::new(group.delimiter(), inner);
                rebuilt.set_span(group.span());
                TokenTree::Group(rebuilt)
            }
            other => other,
        })
        .collect()
}

/// `stream` without its last comma.
fn strip_last_comma(stream: TokenStream) -> TokenStream {
    let mut tokens: Vec<TokenTree> = stream.into_iter().collect();
    if tokens.last().is_some_and(is_comma) {
        tokens.pop();
    }
    tokens.into_iter().collect()
}

fn is_comma(tree: &TokenTree) -> bool {
    matches!(tree, TokenTree::Punct(p) if p.as_char() == ',')
}

/// Rewrite every literal token in `stream` to its canonical decimal spelling.
///
/// Skips the token tree of a macro call and of an attribute, both of
/// which a procedural macro receives verbatim, so the spelling of a
/// literal inside them can be observable.
pub(crate) fn canonical_literals(stream: TokenStream) -> TokenStream {
    let mut out = Vec::new();
    let mut opaque = false;
    for tree in stream {
        let tree = match tree {
            TokenTree::Literal(lit) => {
                let span = lit.span();
                match syn::parse_str::<syn::Lit>(&lit.to_string()) {
                    Ok(parsed) => canon_one_lit(parsed, span),
                    Err(_) => TokenTree::Literal(lit),
                }
            }
            TokenTree::Group(group) if !opaque => {
                let span = group.span();
                let mut rebuilt = Group::new(group.delimiter(), canonical_literals(group.stream()));
                rebuilt.set_span(span);
                TokenTree::Group(rebuilt)
            }
            other => other,
        };
        opaque = matches!(&tree, TokenTree::Punct(p) if matches!(p.as_char(), '!' | '#'));
        out.push(tree);
    }
    out.into_iter().collect()
}

fn canon_one_lit(lit: syn::Lit, span: proc_macro2::Span) -> TokenTree {
    let rebuilt = match lit {
        syn::Lit::Int(i) => {
            let s = format!("{}{}", i.base10_digits(), i.suffix());
            syn::Lit::Int(syn::LitInt::new(&s, span))
        }
        syn::Lit::Float(f) => {
            let digits = canon_float_str(f.base10_digits());
            syn::Lit::Float(syn::LitFloat::new(&format!("{digits}{}", f.suffix()), span))
        }
        syn::Lit::Str(s) => syn::Lit::Str(syn::LitStr::new(&s.value(), span)),
        syn::Lit::ByteStr(b) => syn::Lit::ByteStr(syn::LitByteStr::new(&b.value(), span)),
        syn::Lit::CStr(c) => syn::Lit::CStr(syn::LitCStr::new(c.value().as_c_str(), span)),
        syn::Lit::Char(c) => syn::Lit::Char(syn::LitChar::new(c.value(), span)),
        syn::Lit::Byte(b) => syn::Lit::Byte(syn::LitByte::new(b.value(), span)),
        other => other,
    };
    rebuilt
        .to_token_stream()
        .into_iter()
        .next()
        .expect("rebuilt literal produces at least one token")
}

/// Canonical decimal string for a float's digit part, text only so the
/// canonical form never depends on the feature set. `syn` has already
/// dropped the underscores, a `+` exponent sign and an upper case `E`.
fn canon_float_str(s: &str) -> String {
    let (mantissa, exponent) = match s.split_once('e') {
        Some((mantissa, exponent)) => (mantissa, Some(exponent)),
        None => (s, None),
    };
    let (whole, fraction) = mantissa.split_once('.').unwrap_or((mantissa, ""));
    let fraction = fraction.trim_end_matches('0');
    let mut out = String::with_capacity(s.len());
    out.push_str(whole);
    out.push('.');
    out.push_str(if fraction.is_empty() { "0" } else { fraction });
    if let Some(exponent) = exponent {
        out.push('e');
        let (sign, digits) = exponent
            .strip_prefix('-')
            .map_or(("", exponent), |digits| ("-", digits));
        out.push_str(sign);
        let digits = digits.trim_start_matches('0');
        out.push_str(if digits.is_empty() { "0" } else { digits });
    }
    out
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
        hoist_block_items(&mut block.stmts);
        semicolon_non_tail_macros(&mut block.stmts);
        syn::visit_mut::visit_block_mut(self, block);
        drop_empty_stmts(&mut block.stmts);
    }

    fn visit_item_fn_mut(&mut self, node: &mut syn::ItemFn) {
        syn::visit_mut::visit_item_fn_mut(self, node);
        fold_tail_return(&mut node.block.stmts);
    }

    fn visit_impl_item_fn_mut(&mut self, node: &mut syn::ImplItemFn) {
        syn::visit_mut::visit_impl_item_fn_mut(self, node);
        fold_tail_return(&mut node.block.stmts);
    }

    fn visit_trait_item_fn_mut(&mut self, node: &mut syn::TraitItemFn) {
        syn::visit_mut::visit_trait_item_fn_mut(self, node);
        if let Some(block) = &mut node.default {
            fold_tail_return(&mut block.stmts);
        }
    }

    fn visit_expr_async_mut(&mut self, node: &mut syn::ExprAsync) {
        syn::visit_mut::visit_expr_async_mut(self, node);
        fold_tail_return(&mut node.block.stmts);
    }

    fn visit_arm_mut(&mut self, arm: &mut syn::Arm) {
        strip_inert_attrs(&mut arm.attrs);
        unwrap_arm_block(arm);
        syn::visit_mut::visit_arm_mut(self, arm);
    }

    fn visit_expr_mut(&mut self, expr: &mut syn::Expr) {
        syn::visit_mut::visit_expr_mut(self, expr);
        fold_paren_expr(expr);
        if let syn::Expr::Closure(closure) = expr {
            if let syn::Expr::Block(block) = closure.body.as_mut() {
                fold_tail_return(&mut block.block.stmts);
            }
            unwrap_single_expr_block(&mut closure.body);
        }
    }

    fn visit_pat_mut(&mut self, pat: &mut syn::Pat) {
        strip_pat_inert_attrs(pat);
        syn::visit_mut::visit_pat_mut(self, pat);
        fold_paren_pat(pat);
    }

    fn visit_fn_arg_mut(&mut self, node: &mut syn::FnArg) {
        match node {
            syn::FnArg::Receiver(receiver) => strip_inert_attrs(&mut receiver.attrs),
            syn::FnArg::Typed(typed) => strip_inert_attrs(&mut typed.attrs),
        }
        syn::visit_mut::visit_fn_arg_mut(self, node);
    }

    fn visit_named_arg_mut(&mut self, node: &mut syn::NamedArg) {
        strip_inert_attrs(&mut node.attrs);
        syn::visit_mut::visit_named_arg_mut(self, node);
    }

    fn visit_type_mut(&mut self, ty: &mut syn::Type) {
        syn::visit_mut::visit_type_mut(self, ty);
        fold_paren_type(ty);
    }

    fn visit_signature_mut(&mut self, sig: &mut syn::Signature) {
        syn::visit_mut::visit_signature_mut(self, sig);
        fold_unit_return(&mut sig.output);
    }

    fn visit_stmt_mut(&mut self, stmt: &mut syn::Stmt) {
        match stmt {
            syn::Stmt::Macro(v) => strip_inert_attrs(&mut v.attrs),
            syn::Stmt::Local(v) => strip_inert_attrs(&mut v.attrs),
            _ => {}
        }
        syn::visit_mut::visit_stmt_mut(self, stmt);
    }

    fn visit_item_mut(&mut self, item: &mut syn::Item) {
        if let Some(attrs) = item_attrs(item) {
            strip_inert_attrs(attrs);
        }
        syn::visit_mut::visit_item_mut(self, item);
    }

    fn visit_impl_item_mut(&mut self, item: &mut syn::ImplItem) {
        match item {
            syn::ImplItem::Const(v) => strip_inert_attrs(&mut v.attrs),
            syn::ImplItem::Fn(v) => strip_inert_attrs(&mut v.attrs),
            syn::ImplItem::Type(v) => strip_inert_attrs(&mut v.attrs),
            syn::ImplItem::Macro(v) => strip_inert_attrs(&mut v.attrs),
            _ => {}
        }
        syn::visit_mut::visit_impl_item_mut(self, item);
    }

    fn visit_trait_item_mut(&mut self, item: &mut syn::TraitItem) {
        match item {
            syn::TraitItem::Const(v) => strip_inert_attrs(&mut v.attrs),
            syn::TraitItem::Fn(v) => strip_inert_attrs(&mut v.attrs),
            syn::TraitItem::Type(v) => strip_inert_attrs(&mut v.attrs),
            syn::TraitItem::Macro(v) => strip_inert_attrs(&mut v.attrs),
            _ => {}
        }
        syn::visit_mut::visit_trait_item_mut(self, item);
    }

    fn visit_field_mut(&mut self, field: &mut syn::Field) {
        strip_inert_attrs(&mut field.attrs);
        syn::visit_mut::visit_field_mut(self, field);
    }

    fn visit_variant_mut(&mut self, variant: &mut syn::Variant) {
        strip_inert_attrs(&mut variant.attrs);
        syn::visit_mut::visit_variant_mut(self, variant);
    }

    fn visit_expr_macro_mut(&mut self, node: &mut syn::ExprMacro) {
        normalize_macro_delim(&mut node.mac);
        syn::visit_mut::visit_expr_macro_mut(self, node);
    }

    fn visit_stmt_macro_mut(&mut self, node: &mut syn::StmtMacro) {
        normalize_macro_delim(&mut node.mac);
        syn::visit_mut::visit_stmt_macro_mut(self, node);
    }

    fn visit_type_macro_mut(&mut self, node: &mut syn::TypeMacro) {
        normalize_macro_delim(&mut node.mac);
        syn::visit_mut::visit_type_macro_mut(self, node);
    }

    fn visit_item_macro_mut(&mut self, node: &mut syn::ItemMacro) {
        if node.ident.is_none() {
            normalize_macro_delim(&mut node.mac);
            node.semi_token.get_or_insert_with(Default::default);
        }
        syn::visit_mut::visit_item_macro_mut(self, node);
    }

    fn visit_impl_item_macro_mut(&mut self, node: &mut syn::ImplItemMacro) {
        normalize_macro_delim(&mut node.mac);
        node.semi_token.get_or_insert_with(Default::default);
        syn::visit_mut::visit_impl_item_macro_mut(self, node);
    }

    fn visit_trait_item_macro_mut(&mut self, node: &mut syn::TraitItemMacro) {
        normalize_macro_delim(&mut node.mac);
        node.semi_token.get_or_insert_with(Default::default);
        syn::visit_mut::visit_trait_item_macro_mut(self, node);
    }
}

/// An arm body `{ expr }` becomes `expr`, the form rustfmt writes.
fn unwrap_arm_block(arm: &mut syn::Arm) {
    unwrap_single_expr_block(&mut arm.body);
}

/// A closure or arm body `{ expr }` becomes `expr` when the block holds
/// exactly one tail expression and carries no label or attributes.
fn unwrap_single_expr_block(body: &mut Box<syn::Expr>) {
    let syn::Expr::Block(block) = body.as_mut() else {
        return;
    };
    if block.label.is_some()
        || !block.attrs.is_empty()
        || !matches!(block.block.stmts.as_slice(), [syn::Stmt::Expr(_, None)])
    {
        return;
    }
    if let Some(syn::Stmt::Expr(expr, None)) = block.block.stmts.pop() {
        **body = expr;
    }
}

/// Remove every bare empty statement from `stmts`.
fn drop_empty_stmts(stmts: &mut Vec<syn::Stmt>) {
    stmts.retain(|stmt| {
        !matches!(
            stmt,
            syn::Stmt::Expr(syn::Expr::Verbatim(v), Some(_)) if v.is_empty()
        )
    });
}

/// Replace a tail `return expr;` with `expr`, then propagate through every
/// tail position forwarding its value, an `if` only with an `else` clause
/// because rustc rejects a valued return in a discarded then position
/// (`E0317`).
fn fold_tail_return(stmts: &mut Vec<syn::Stmt>) {
    let n = stmts.len();
    if n != 0 {
        let is_valued_tail_return = matches!(&stmts[n - 1], syn::Stmt::Expr(syn::Expr::Return(r), Some(_)) if r.expr.is_some());
        if is_valued_tail_return {
            let last = stmts.remove(n - 1);
            if let syn::Stmt::Expr(syn::Expr::Return(mut ret), Some(_)) = last
                && let Some(mut inner) = ret.expr.take()
            {
                fold_tail_expr(&mut inner);
                stmts.push(syn::Stmt::Expr(*inner, None));
            }
        } else if let Some(syn::Stmt::Expr(expr, None)) = stmts.last_mut() {
            fold_tail_expr(expr);
        }
    }
}

/// Continue the fold through an expression whose tail value is the enclosing
/// tail's value.
fn fold_tail_expr(expr: &mut syn::Expr) {
    match expr {
        syn::Expr::Block(block) => fold_tail_return(&mut block.block.stmts),
        syn::Expr::Unsafe(block) => fold_tail_return(&mut block.block.stmts),
        syn::Expr::If(expr_if) if expr_if.else_branch.is_some() => {
            fold_tail_return(&mut expr_if.then_branch.stmts);
            if let Some((_, otherwise)) = &mut expr_if.else_branch {
                fold_tail_expr(otherwise);
            }
        }
        syn::Expr::Match(expr_match) => {
            for arm in &mut expr_match.arms {
                fold_tail_expr(&mut arm.body);
                unwrap_single_expr_block(&mut arm.body);
            }
        }
        _ => {}
    }
}

/// Give every statement macro but the last one a semicolon, which the
/// brace form omits. A tail macro's value is the block's value.
fn semicolon_non_tail_macros(stmts: &mut [syn::Stmt]) {
    let Some((_, rest)) = stmts.split_last_mut() else {
        return;
    };
    for stmt in rest {
        if let syn::Stmt::Macro(mac) = stmt {
            mac.semi_token.get_or_insert_with(Default::default);
        }
    }
}

/// Drop an explicit `-> ()`, which a signature means by omitting it.
fn fold_unit_return(output: &mut syn::ReturnType) {
    if let syn::ReturnType::Type(_, ty) = output
        && matches!(ty.as_ref(), syn::Type::Tuple(t) if t.elems.is_empty())
    {
        *output = syn::ReturnType::Default;
    }
}

/// Remove a no-attribute `Expr::Paren` wrapper, leaving the inner expression.
fn fold_paren_expr(expr: &mut syn::Expr) {
    if !matches!(expr, syn::Expr::Paren(p) if p.attrs.is_empty()) {
        return;
    }
    let dummy = syn::Expr::Verbatim(proc_macro2::TokenStream::new());
    if let syn::Expr::Paren(paren) = core::mem::replace(expr, dummy) {
        *expr = *paren.expr;
    }
}

/// Remove a no-attribute `Pat::Paren` wrapper, leaving the inner pattern.
fn fold_paren_pat(pat: &mut syn::Pat) {
    if !matches!(pat, syn::Pat::Paren(p) if p.attrs.is_empty()) {
        return;
    }
    let dummy = syn::Pat::Verbatim(proc_macro2::TokenStream::new());
    if let syn::Pat::Paren(paren) = core::mem::replace(pat, dummy) {
        *pat = *paren.pat;
    }
}

/// Remove a `Type::Paren` wrapper, leaving the inner type.
fn fold_paren_type(ty: &mut syn::Type) {
    if !matches!(ty, syn::Type::Paren(_)) {
        return;
    }
    let dummy = syn::Type::Verbatim(proc_macro2::TokenStream::new());
    if let syn::Type::Paren(paren) = core::mem::replace(ty, dummy) {
        *ty = *paren.elem;
    }
}

/// True for attributes that carry no program meaning for doctests: doc
/// comments and lint-level directives.
fn is_inert_attr(attr: &syn::Attribute) -> bool {
    let p = attr.path();
    p.is_ident("doc") || p.is_ident("allow") || p.is_ident("expect") || p.is_ident("warn")
}

/// Drop inert attributes from the list.
fn strip_inert_attrs(attrs: &mut Vec<syn::Attribute>) {
    attrs.retain(|attr| !is_inert_attr(attr));
}

/// An attribute on an untyped closure parameter sits on the pattern itself,
/// arm and named-parameter attributes sit on the arm or `FnArg` instead.
/// `Path`, `Or`, `Range`, `Guard`, `Rest` and `Const` never keep an attribute
/// in a parsed tree, a single-segment name parses as `Ident`, an or-pattern
/// splits the parameter list and an attributed range reprints as a `Paren`.
fn strip_pat_inert_attrs(pat: &mut syn::Pat) {
    let attrs = match pat {
        syn::Pat::Ident(p) => &mut p.attrs,
        syn::Pat::Lit(p) => &mut p.attrs,
        syn::Pat::Macro(p) => &mut p.attrs,
        syn::Pat::Paren(p) => &mut p.attrs,
        syn::Pat::Reference(p) => &mut p.attrs,
        syn::Pat::Slice(p) => &mut p.attrs,
        syn::Pat::Struct(p) => &mut p.attrs,
        syn::Pat::Tuple(p) => &mut p.attrs,
        syn::Pat::TupleStruct(p) => &mut p.attrs,
        syn::Pat::Wild(p) => &mut p.attrs,
        _ => return,
    };
    strip_inert_attrs(attrs);
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

/// True when every attribute on `item` would be removed by
/// `strip_inert_attrs`, meaning hoisting it cannot move an attribute
/// that references a local binding. `use` items never reach here,
/// `hoist_uses` has already taken them.
fn item_has_only_inert_attrs(item: &syn::Item) -> bool {
    let attrs: &[syn::Attribute] = match item {
        syn::Item::Const(v) => &v.attrs,
        syn::Item::Enum(v) => &v.attrs,
        syn::Item::ExternCrate(v) => &v.attrs,
        syn::Item::Fn(v) => &v.attrs,
        syn::Item::ForeignMod(v) => &v.attrs,
        syn::Item::Impl(v) => &v.attrs,
        syn::Item::Mod(v) => &v.attrs,
        syn::Item::Static(v) => &v.attrs,
        syn::Item::Struct(v) => &v.attrs,
        syn::Item::Trait(v) => &v.attrs,
        syn::Item::TraitAlias(v) => &v.attrs,
        syn::Item::Type(v) => &v.attrs,
        syn::Item::Union(v) => &v.attrs,
        _ => return true,
    };
    attrs.iter().all(is_inert_attr)
}

/// Hoist non-`use`, non-`macro_rules!` item statements that appear before
/// the first `macro_rules!` definition in a block to the front, right
/// after the sorted `use` items placed there by `hoist_uses`.
///
/// Only items whose attributes are all inert are hoisted. An item with a
/// non-inert attribute may have an argument that references a local
/// binding; hoisting it before that binding changes the visit order and
/// breaks alpha renaming.
///
/// Items in that region move to the front in their original relative order
/// among items. Non-item statements follow them in their original relative
/// order. Everything from the first `Stmt::Item(Item::Macro)` onward is
/// untouched, because `macro_rules!` visibility is sequential.
fn hoist_block_items(stmts: &mut Vec<syn::Stmt>) {
    let use_count = stmts.partition_point(|s| matches!(s, syn::Stmt::Item(syn::Item::Use(_))));

    let rel_macro = stmts[use_count..]
        .iter()
        .position(|s| matches!(s, syn::Stmt::Item(syn::Item::Macro(_))));
    let hoist_end = use_count + rel_macro.unwrap_or(stmts.len() - use_count);

    if hoist_end == use_count {
        return;
    }

    let tail = stmts.split_off(hoist_end);
    let region = stmts.split_off(use_count);

    let mut items = Vec::new();
    let mut rest = Vec::new();
    for stmt in region {
        let hoistable = match &stmt {
            syn::Stmt::Item(item) => item_has_only_inert_attrs(item),
            _ => false,
        };
        if hoistable {
            items.push(stmt);
        } else {
            rest.push(stmt);
        }
    }

    stmts.extend(items);
    stmts.extend(rest);
    stmts.extend(tail);
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

/// Rewrite the outer delimiter of a macro call to parentheses.
fn normalize_macro_delim(mac: &mut syn::Macro) {
    if matches!(mac.delimiter, syn::MacroDelimiter::Paren(_)) {
        return;
    }
    let span = *mac.delimiter.span();
    mac.delimiter = syn::MacroDelimiter::Paren(syn::token::Paren { span });
}
