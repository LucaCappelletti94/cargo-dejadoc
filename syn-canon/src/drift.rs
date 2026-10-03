//! Formatting drift rustfmt introduces, folded before hashing.

use alloc::boxed::Box;
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use proc_macro2::{Group, TokenStream, TokenTree};
use quote::ToTokens;
use syn::visit_mut::VisitMut;

/// `group` with its stream mapped through `f`, delimiter and span kept.
pub(crate) fn map_group(group: &Group, f: impl FnOnce(TokenStream) -> TokenStream) -> TokenTree {
    let mut rebuilt = Group::new(group.delimiter(), f(group.stream()));
    rebuilt.set_span(group.span());
    TokenTree::Group(rebuilt)
}

/// `lit` in its canonical decimal spelling.
pub(crate) fn canonical_literal(lit: proc_macro2::Literal) -> TokenTree {
    let span = lit.span();
    let rebuilt = match syn::Lit::new(lit) {
        syn::Lit::Int(i) => {
            let decimal = format!("{}{}", i.base10_digits(), i.suffix());
            match syn::parse_str::<syn::LitInt>(&decimal) {
                Ok(mut read)
                    if read.base10_digits() == i.base10_digits() && read.suffix() == i.suffix() =>
                {
                    read.set_span(span);
                    syn::Lit::Int(read)
                }
                // The suffix reads as a radix prefix or an exponent after the digits, `0b0buu`.
                _ => syn::Lit::Int(i),
            }
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
    Drift::default().visit_file_mut(file);
}

#[derive(Default)]
struct Drift {
    /// Inside an impl header's trait path or self type, where a path may not
    /// elide its lifetime (E0726).
    in_impl_header: bool,
}

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
        // The printer writes the comma a non-block arm needs, a written one is drift.
        arm.comma = None;
        syn::visit_mut::visit_arm_mut(self, arm);
    }

    fn visit_expr_mut(&mut self, expr: &mut syn::Expr) {
        syn::visit_mut::visit_expr_mut(self, expr);
        fold_paren_expr(expr);
        if let syn::Expr::If(expr_if) = expr {
            collapse_else_if(expr_if);
        }
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

    fn visit_generics_mut(&mut self, generics: &mut syn::Generics) {
        syn::visit_mut::visit_generics_mut(self, generics);
        bounds_to_where(generics);
    }

    fn visit_type_reference_mut(&mut self, node: &mut syn::TypeReference) {
        syn::visit_mut::visit_type_reference_mut(self, node);
        if node.lifetime.as_ref().is_some_and(|l| l.ident == "_") {
            node.lifetime = None;
        }
    }

    fn visit_path_arguments_mut(&mut self, node: &mut syn::PathArguments) {
        syn::visit_mut::visit_path_arguments_mut(self, node);
        if !self.in_impl_header {
            drop_anonymous_lifetimes(node);
        }
    }

    fn visit_item_impl_mut(&mut self, node: &mut syn::ItemImpl) {
        for attr in &mut node.attrs {
            self.visit_attribute_mut(attr);
        }
        self.visit_generics_mut(&mut node.generics);
        let outer = core::mem::replace(&mut self.in_impl_header, true);
        if let Some((path, _)) = &mut node.trait_ {
            self.visit_path_mut(path);
        }
        self.visit_type_mut(&mut node.self_ty);
        self.in_impl_header = outer;
        for item in &mut node.items {
            self.visit_impl_item_mut(item);
        }
    }

    fn visit_item_const_mut(&mut self, node: &mut syn::ItemConst) {
        syn::visit_mut::visit_item_const_mut(self, node);
        StaticRefs.visit_type_mut(&mut node.ty);
    }

    fn visit_item_static_mut(&mut self, node: &mut syn::ItemStatic) {
        syn::visit_mut::visit_item_static_mut(self, node);
        StaticRefs.visit_type_mut(&mut node.ty);
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
        if let syn::Item::Struct(syn::ItemStruct { attrs, .. })
        | syn::Item::Enum(syn::ItemEnum { attrs, .. })
        | syn::Item::Union(syn::ItemUnion { attrs, .. }) = item
        {
            fold_std_derives(attrs);
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
        inline_format_args(&mut node.mac);
        syn::visit_mut::visit_expr_macro_mut(self, node);
    }

    fn visit_stmt_macro_mut(&mut self, node: &mut syn::StmtMacro) {
        normalize_macro_delim(&mut node.mac);
        inline_format_args(&mut node.mac);
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

/// An `else { if … }` whose block holds only that `if`, with no attribute
/// on it, becomes `else if …`. An else block itself never carries a label
/// or attributes.
fn collapse_else_if(expr_if: &mut syn::ExprIf) {
    let Some((_, otherwise)) = &mut expr_if.else_branch else {
        return;
    };
    let syn::Expr::Block(block) = otherwise.as_mut() else {
        return;
    };
    if !matches!(block.block.stmts.as_slice(), [syn::Stmt::Expr(syn::Expr::If(inner), None)] if inner.attrs.is_empty())
    {
        return;
    }
    if let Some(syn::Stmt::Expr(inner, None)) = block.block.stmts.pop() {
        **otherwise = inner;
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

/// Replace a tail `return expr`, with or without the semicolon, with
/// `expr`, then propagate through every tail position forwarding its value,
/// an `if` only with an `else` clause because rustc rejects a valued return
/// in a discarded then position (`E0317`). A tail unit `return` adds
/// nothing and goes. A return keeping a live attribute, `#[cfg]` for
/// instance, must not fold, the fold would drop the condition.
fn fold_tail_return(stmts: &mut Vec<syn::Stmt>) {
    let Some(last) = stmts.last_mut() else {
        return;
    };
    match last {
        syn::Stmt::Expr(syn::Expr::Return(ret), _) => {
            strip_inert_attrs(&mut ret.attrs);
            if !ret.attrs.is_empty() {
                return;
            }
            if let Some(mut inner) = ret.expr.take() {
                fold_tail_expr(&mut inner);
                *last = syn::Stmt::Expr(*inner, None);
            } else {
                stmts.pop();
                fold_tail_return(stmts);
            }
        }
        syn::Stmt::Expr(expr, None) => fold_tail_expr(expr),
        _ => {}
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
        // An arm body `return value`, written bare or as a block the arm fold unwrapped.
        syn::Expr::Return(ret) => {
            strip_inert_attrs(&mut ret.attrs);
            if ret.attrs.is_empty()
                && let Some(inner) = ret.expr.take()
            {
                *expr = *inner;
                fold_tail_expr(expr);
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

/// Move the bounds written on type and lifetime parameters into the `where`
/// clause, ahead of its own predicates and in parameter order, the place
/// they would take written there.
fn bounds_to_where(generics: &mut syn::Generics) {
    let mut moved = Vec::new();
    for param in &mut generics.params {
        match param {
            syn::GenericParam::Type(ty) if !ty.bounds.is_empty() => {
                ty.colon_token = None;
                moved.push(syn::WherePredicate::Type(syn::PredicateType {
                    attrs: Vec::new(),
                    lifetimes: None,
                    bounded_ty: syn::Type::Path(syn::TypePath {
                        attrs: Vec::new(),
                        qself: None,
                        path: ty.ident.clone().into(),
                    }),
                    colon_token: syn::token::Colon::default(),
                    bounds: core::mem::take(&mut ty.bounds),
                }));
            }
            syn::GenericParam::Lifetime(lifetime) if !lifetime.bounds.is_empty() => {
                lifetime.colon_token = None;
                moved.push(syn::WherePredicate::Lifetime(syn::PredicateLifetime {
                    attrs: Vec::new(),
                    lifetime: lifetime.lifetime.clone(),
                    colon_token: syn::token::Colon::default(),
                    bounds: core::mem::take(&mut lifetime.bounds),
                }));
            }
            _ => {}
        }
    }
    if moved.is_empty() {
        return;
    }
    let clause = generics.make_where_clause();
    let written = core::mem::take(&mut clause.predicates);
    clause.predicates.extend(moved);
    clause.predicates.extend(written);
}

/// Drop every `'_` argument from angle-bracketed path arguments, and the
/// brackets once nothing is left, `Foo<'_>` spelling `Foo`.
fn drop_anonymous_lifetimes(arguments: &mut syn::PathArguments) {
    let syn::PathArguments::AngleBracketed(angle) = arguments else {
        return;
    };
    let anonymous = |arg: &syn::GenericArgument| matches!(arg, syn::GenericArgument::Lifetime(l) if l.ident == "_");
    if !angle.args.iter().any(anonymous) {
        return;
    }
    angle.args = core::mem::take(&mut angle.args)
        .into_iter()
        .filter(|arg| !anonymous(arg))
        .collect();
    if angle.args.is_empty() {
        *arguments = syn::PathArguments::None;
    }
}

/// The derives whose expansions are independent of each other, so their
/// order and grouping carry no meaning.
const STD_DERIVES: [&str; 9] = [
    "Clone",
    "Copy",
    "Debug",
    "Default",
    "Eq",
    "Hash",
    "Ord",
    "PartialEq",
    "PartialOrd",
];

/// Merge an item's `derive` attributes into one sorted list at the place of
/// the first, when every derive among them is a std one. A proc macro derive
/// can depend on what ran before it, so any other name keeps them as written.
fn fold_std_derives(attrs: &mut Vec<syn::Attribute>) {
    use syn::parse::Parser;

    let mut names: Vec<syn::Ident> = Vec::new();
    let mut first = None;
    for (index, attr) in attrs.iter().enumerate() {
        if !attr.path().is_ident("derive") {
            continue;
        }
        let syn::Meta::List(list) = &attr.meta else {
            return;
        };
        let parser = syn::punctuated::Punctuated::<syn::Path, syn::Token![,]>::parse_terminated;
        let Ok(paths) = parser.parse2(list.tokens.clone()) else {
            return;
        };
        for path in paths {
            match path.get_ident() {
                Some(name) if STD_DERIVES.contains(&name.to_string().as_str()) => {
                    names.push(name.clone());
                }
                _ => return,
            }
        }
        first.get_or_insert(index);
    }
    let Some(first) = first else {
        return;
    };
    names.sort_by_cached_key(ToString::to_string);
    attrs.retain(|attr| !attr.path().is_ident("derive"));
    attrs.insert(first, syn::parse_quote!(#[derive(#(#names),*)]));
}

/// Elides the `'static` of references in a `const` or `static` type, where an
/// elided lifetime is `'static`. Inside a fn pointer or `Fn` sugar elision
/// means a fresh higher-ranked lifetime, so neither is entered.
struct StaticRefs;

impl VisitMut for StaticRefs {
    fn visit_type_reference_mut(&mut self, node: &mut syn::TypeReference) {
        if node.lifetime.as_ref().is_some_and(|l| l.ident == "static") {
            node.lifetime = None;
        }
        syn::visit_mut::visit_type_reference_mut(self, node);
    }

    fn visit_type_fn_ptr_mut(&mut self, _: &mut syn::TypeFnPtr) {}

    fn visit_parenthesized_generic_arguments_mut(
        &mut self,
        _: &mut syn::ParenthesizedGenericArguments,
    ) {
    }
}

/// True for attributes that carry no program meaning for doctests: doc
/// comments, lint-level directives, and the `rustfmt::` tool attributes.
fn is_inert_attr(attr: &syn::Attribute) -> bool {
    let p = attr.path();
    p.is_ident("doc")
        || p.is_ident("allow")
        || p.is_ident("expect")
        || p.is_ident("warn")
        || p.segments.first().is_some_and(|s| s.ident == "rustfmt")
}

/// Drop inert attributes from the list.
fn strip_inert_attrs(attrs: &mut Vec<syn::Attribute>) {
    attrs.retain(|attr| !is_inert_attr(attr));
}

/// An attribute on an untyped closure parameter sits on the pattern itself,
/// arm and named-parameter attributes sit on the arm or `FnArg` instead.
/// Every pattern kind can carry one, a range for example parses with its
/// attribute and reprints it as `#[a] (1..=5)`.
fn strip_pat_inert_attrs(pat: &mut syn::Pat) {
    let attrs = match pat {
        syn::Pat::Const(p) => &mut p.attrs,
        syn::Pat::Guard(p) => &mut p.attrs,
        syn::Pat::Ident(p) => &mut p.attrs,
        syn::Pat::Lit(p) => &mut p.attrs,
        syn::Pat::Macro(p) => &mut p.attrs,
        syn::Pat::Or(p) => &mut p.attrs,
        syn::Pat::Paren(p) => &mut p.attrs,
        syn::Pat::Path(p) => &mut p.attrs,
        syn::Pat::Range(p) => &mut p.attrs,
        syn::Pat::Reference(p) => &mut p.attrs,
        syn::Pat::Rest(p) => &mut p.attrs,
        syn::Pat::Slice(p) => &mut p.attrs,
        syn::Pat::Struct(p) => &mut p.attrs,
        syn::Pat::Tuple(p) => &mut p.attrs,
        syn::Pat::TupleStruct(p) => &mut p.attrs,
        syn::Pat::Type(p) => &mut p.attrs,
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

/// Hoist non-`use`, non-`macro_rules!` item statements that appear before
/// the first `macro_rules!` definition in a block to the front, right
/// after the sorted `use` items placed there by `hoist_uses`.
///
/// Only items whose attributes `strip_inert_attrs` would all remove are
/// hoisted. A live attribute may reference a local binding, and hoisting
/// it before that binding changes the visit order and breaks alpha
/// renaming.
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
    for mut stmt in region {
        let hoistable = match &mut stmt {
            syn::Stmt::Item(item) => {
                item_attrs(item).is_none_or(|attrs| attrs.iter().all(is_inert_attr))
            }
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

/// The format string's index among a std formatting macro's arguments, and
/// whether the macro is a panic, whose lone literal before edition 2021 is
/// no format string.
pub(crate) fn format_operand(path: &syn::Path) -> Option<(usize, bool)> {
    let name = path.segments.last()?.ident.to_string();
    Some(match name.as_str() {
        "print" | "println" | "eprint" | "eprintln" | "format" | "format_args" => (0, false),
        "write" | "writeln" => (1, false),
        "panic" | "unreachable" | "todo" | "unimplemented" => (0, true),
        "assert" | "debug_assert" => (1, true),
        "assert_eq" | "assert_ne" | "debug_assert_eq" | "debug_assert_ne" => (2, true),
        _ => return None,
    })
}

/// Move identifier arguments of a formatting macro into its format string,
/// `println!("{}", x)` becoming `println!("{x}")`, in `mac` and in every
/// formatting call among its arguments. Returns whether `mac` changed.
fn inline_format_args(mac: &mut syn::Macro) -> bool {
    use syn::parse::Parser;

    let parser = syn::punctuated::Punctuated::<syn::Expr, syn::Token![,]>::parse_terminated;
    let Ok(mut args) = parser.parse2(mac.tokens.clone()) else {
        return false;
    };
    let mut nested = NestedFormat(false);
    for arg in &mut args {
        nested.visit_expr_mut(arg);
    }
    let inlined = inline_positional(&mac.path, &mut args);
    if nested.0 || inlined {
        mac.tokens = args.to_token_stream();
    }
    nested.0 || inlined
}

/// Inlines the formatting calls inside a macro's arguments, which the
/// drift visit never enters.
struct NestedFormat(bool);

impl VisitMut for NestedFormat {
    fn visit_macro_mut(&mut self, mac: &mut syn::Macro) {
        self.0 |= inline_format_args(mac);
    }
}

/// Inline the identifier arguments of `args` when `path` names a non-panic
/// formatting macro. Returns whether anything moved.
fn inline_positional(
    path: &syn::Path,
    args: &mut syn::punctuated::Punctuated<syn::Expr, syn::Token![,]>,
) -> bool {
    let Some((at, false)) = format_operand(path) else {
        return false;
    };
    let positional: Vec<&syn::Expr> = args.iter().skip(at + 1).collect();
    if positional
        .iter()
        .any(|arg| matches!(arg, syn::Expr::Assign(_)))
    {
        return false;
    }
    let names: Vec<Option<String>> = positional.into_iter().map(capturable).collect();
    let Some(syn::Expr::Lit(syn::ExprLit {
        lit: syn::Lit::Str(format),
        ..
    })) = args.iter_mut().nth(at)
    else {
        return false;
    };
    let Some(text) = inline_placeholders(&format.value(), &names) else {
        return false;
    };
    *format = syn::LitStr::new(&text, format.span());
    let kept = core::mem::take(args)
        .into_iter()
        .enumerate()
        .filter(|(index, _)| *index <= at || names[index - at - 1].is_none())
        .map(|(_, arg)| arg);
    args.extend(kept);
    true
}

/// The identifier a format string captures for `arg`, `None` for any other
/// expression and for the names a placeholder cannot spell.
fn capturable(arg: &syn::Expr) -> Option<String> {
    let syn::Expr::Path(path) = arg else {
        return None;
    };
    if !path.attrs.is_empty() {
        return None;
    }
    let name = path.path.get_ident()?.to_string();
    let spellable =
        !name.starts_with("r#") && !matches!(name.as_str(), "self" | "Self" | "crate" | "super");
    spellable.then_some(name)
}

/// `format` with each implicit `{}` or `{:spec}` placeholder whose argument
/// in `names` is an identifier spelled inline. `None` when nothing moves, or
/// when an explicit index, a `$` or `*` spec or a count mismatch makes the
/// argument order matter.
fn inline_placeholders(format: &str, names: &[Option<String>]) -> Option<String> {
    let mut out = String::with_capacity(format.len());
    let mut next = 0;
    let mut chars = format.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '{' | '}' if chars.peek() == Some(&c) => {
                chars.next();
                out.push(c);
                out.push(c);
            }
            '{' => {
                let mut body = String::new();
                loop {
                    match chars.next()? {
                        '}' => break,
                        c => body.push(c),
                    }
                }
                let (arg, spec) = body
                    .split_once(':')
                    .map_or((body.as_str(), None), |(a, s)| (a, Some(s)));
                let explicit_index = !arg.is_empty() && arg.bytes().all(|b| b.is_ascii_digit());
                if explicit_index || spec.is_some_and(|spec| spec.contains(['$', '*'])) {
                    return None;
                }
                out.push('{');
                if arg.is_empty() {
                    out.push_str(names.get(next)?.as_deref().unwrap_or_default());
                    next += 1;
                } else {
                    out.push_str(arg);
                }
                if let Some(spec) = spec {
                    out.push(':');
                    out.push_str(spec);
                }
                out.push('}');
            }
            '}' => return None,
            c => out.push(c),
        }
    }
    (next == names.len() && names.iter().any(Option::is_some)).then_some(out)
}
