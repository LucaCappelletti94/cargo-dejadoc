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

/// Fold arm braces, doc attributes, and `use` shapes in place. When `compiles`, also fold the
/// spellings only a compiling program makes equivalent.
pub(crate) fn normalize_file(file: &mut syn::File, compiles: bool) {
    if compiles {
        crate::const_items::inline_array_constants(file);
    }
    Drift {
        compiles,
        ..Drift::default()
    }
    .visit_file_mut(file);
}

#[derive(Default)]
struct Drift {
    /// The code compiles, so an unneeded `mut` and the semicolon of a `()` block fold.
    compiles: bool,
    /// Inside an impl header's trait path or self type, where a path may not
    /// elide its lifetime (E0726).
    in_impl_header: bool,
    /// Inside syntax read by a procedural macro.
    in_macro_input: bool,
}

impl VisitMut for Drift {
    #[expect(
        clippy::result_large_err,
        reason = "a non-use entry goes back to the list"
    )]
    fn visit_file_mut(&mut self, file: &mut syn::File) {
        let outer = self.enter_attrs(&file.attrs);
        hoist_uses(
            &mut file.items,
            |item| match item {
                syn::Item::Use(u) => Ok(u),
                other => Err(other),
            },
            syn::Item::Use,
        );
        syn::visit_mut::visit_file_mut(self, file);
        self.in_macro_input = outer;
    }

    #[expect(
        clippy::result_large_err,
        reason = "a non-use entry goes back to the list"
    )]
    fn visit_block_mut(&mut self, block: &mut syn::Block) {
        drop_empty_stmts(&mut block.stmts);
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
        if self.compiles {
            drop_block_statement_semicolons(&mut block.stmts);
        }
        syn::visit_mut::visit_block_mut(self, block);
    }

    fn visit_item_fn_mut(&mut self, node: &mut syn::ItemFn) {
        syn::visit_mut::visit_item_fn_mut(self, node);
        self.fold_fn_body(&node.sig, &mut node.block);
    }

    fn visit_impl_item_fn_mut(&mut self, node: &mut syn::ImplItemFn) {
        syn::visit_mut::visit_impl_item_fn_mut(self, node);
        self.fold_fn_body(&node.sig, &mut node.block);
    }

    fn visit_trait_item_fn_mut(&mut self, node: &mut syn::TraitItemFn) {
        syn::visit_mut::visit_trait_item_fn_mut(self, node);
        if let Some(block) = &mut node.default {
            self.fold_fn_body(&node.sig, block);
        }
    }

    fn visit_expr_async_mut(&mut self, node: &mut syn::ExprAsync) {
        syn::visit_mut::visit_expr_async_mut(self, node);
        fold_tail_return(&mut node.block.stmts);
    }

    fn visit_arm_mut(&mut self, arm: &mut syn::Arm) {
        strip_inert_attrs(&mut arm.attrs);
        let outer = self.enter_attrs(&arm.attrs);
        unwrap_arm_block(arm);
        // The printer writes the comma a non-block arm needs, a written one is drift.
        arm.comma = None;
        syn::visit_mut::visit_arm_mut(self, arm);
        if self.compiles {
            drop_binding_mut(&mut arm.pat);
        }
        self.in_macro_input = outer;
    }

    fn visit_local_mut(&mut self, local: &mut syn::Local) {
        let outer = self.enter_attrs(&local.attrs);
        syn::visit_mut::visit_local_mut(self, local);
        if self.compiles {
            drop_binding_mut(&mut local.pat);
        }
        self.in_macro_input = outer;
    }

    fn visit_expr_mut(&mut self, expr: &mut syn::Expr) {
        let outer = self.enter_attrs(expr_attrs(expr));
        syn::visit_mut::visit_expr_mut(self, expr);
        fold_paren_expr(expr);
        if let syn::Expr::If(expr_if) = expr {
            collapse_else_if(expr_if);
            if self.compiles && !self.in_macro_input {
                fold_empty_else(expr_if);
            }
        }
        if let syn::Expr::Closure(closure) = expr {
            let protected_inputs = closure
                .inputs
                .iter_mut()
                .any(|pat| pat_attrs(pat).is_some_and(|attrs| has_macro_attribute(attrs)));
            if !self.in_macro_input && !protected_inputs {
                if closure.inputs.trailing_punct() {
                    closure.inputs.pop_punct();
                }
                if matches!(
                    closure.body.as_ref(),
                    syn::Expr::Block(block)
                        if block.attrs.is_empty()
                            && block.label.is_none()
                            && block.block.stmts.is_empty()
                ) {
                    fold_unit_return(&mut closure.output);
                }
            }
            if let syn::Expr::Block(block) = closure.body.as_mut() {
                fold_tail_return(&mut block.block.stmts);
            }
            unwrap_single_expr_block(&mut closure.body);
        }
        if self.compiles {
            if !self.in_macro_input
                && let syn::Expr::Return(ret) = expr
                && ret.expr.as_ref().is_some_and(
                    |value| matches!(value.as_ref(), syn::Expr::Tuple(tuple) if tuple.elems.is_empty() && tuple.attrs.is_empty()),
                )
            {
                ret.expr = None;
            }
            match expr {
                syn::Expr::Closure(closure) => closure.inputs.iter_mut().for_each(drop_binding_mut),
                syn::Expr::ForLoop(for_loop) => drop_binding_mut(&mut for_loop.pat),
                syn::Expr::Let(expr_let) => drop_binding_mut(&mut expr_let.pat),
                _ => {}
            }
            if let Some(block) = unit_block(expr) {
                drop_tail_semicolon(&mut block.stmts);
            }
        }
        if !self.in_macro_input {
            fold_signed_zero(expr);
            if self.compiles {
                fold_scalar_const(expr);
            }
        }
        self.in_macro_input = outer;
    }

    fn visit_pat_mut(&mut self, pat: &mut syn::Pat) {
        let outer = pat_attrs(pat).map_or(self.in_macro_input, |attrs| {
            strip_inert_attrs(attrs);
            self.enter_attrs(attrs)
        });
        syn::visit_mut::visit_pat_mut(self, pat);
        fold_paren_pat(pat);
        if self.compiles && !self.in_macro_input {
            fold_pattern_spelling(pat);
        }
        self.in_macro_input = outer;
    }

    fn visit_fn_arg_mut(&mut self, node: &mut syn::FnArg) {
        match node {
            syn::FnArg::Receiver(receiver) => strip_inert_attrs(&mut receiver.attrs),
            syn::FnArg::Typed(typed) => strip_inert_attrs(&mut typed.attrs),
        }
        let outer = self.enter_attrs(match node {
            syn::FnArg::Receiver(receiver) => &receiver.attrs,
            syn::FnArg::Typed(typed) => &typed.attrs,
        });
        syn::visit_mut::visit_fn_arg_mut(self, node);
        if self.compiles {
            match node {
                syn::FnArg::Receiver(receiver) => receiver.mutability = None,
                syn::FnArg::Typed(typed) => drop_binding_mut(&mut typed.pat),
            }
        }
        if !self.in_macro_input
            && let syn::FnArg::Receiver(receiver) = node
        {
            fold_explicit_receiver(receiver);
        }
        self.in_macro_input = outer;
    }

    fn visit_named_arg_mut(&mut self, node: &mut syn::NamedArg) {
        strip_inert_attrs(&mut node.attrs);
        let outer = self.enter_attrs(&node.attrs);
        syn::visit_mut::visit_named_arg_mut(self, node);
        self.in_macro_input = outer;
    }

    fn visit_type_mut(&mut self, ty: &mut syn::Type) {
        let outer = self.enter_attrs(type_attrs(ty));
        syn::visit_mut::visit_type_mut(self, ty);
        fold_paren_type(ty);
        self.in_macro_input = outer;
    }

    fn visit_type_path_mut(&mut self, node: &mut syn::TypePath) {
        syn::visit_mut::visit_type_path_mut(self, node);
        if !self.in_macro_input {
            fold_type_path(&mut node.path);
        }
    }

    fn visit_type_fn_ptr_mut(&mut self, node: &mut syn::TypeFnPtr) {
        syn::visit_mut::visit_type_fn_ptr_mut(self, node);
        if self.compiles && !self.in_macro_input {
            fold_fn_pointer_lifetime(node);
        }
        if !self.in_macro_input {
            fold_unit_return(&mut node.output);
        }
    }

    fn visit_abi_mut(&mut self, node: &mut syn::Abi) {
        if self.compiles && !self.in_macro_input && node.name.is_none() {
            node.name = Some(syn::LitStr::new("C", node.extern_token.span));
        }
    }

    fn visit_vis_restricted_mut(&mut self, node: &mut syn::VisRestricted) {
        syn::visit_mut::visit_vis_restricted_mut(self, node);
        if !self.in_macro_input
            && ["crate", "self", "super"]
                .iter()
                .any(|name| node.path.is_ident(name))
        {
            node.in_token = None;
        }
    }

    fn visit_parenthesized_generic_arguments_mut(
        &mut self,
        node: &mut syn::ParenthesizedGenericArguments,
    ) {
        syn::visit_mut::visit_parenthesized_generic_arguments_mut(self, node);
        if !self.in_macro_input {
            fold_unit_return(&mut node.output);
        }
    }

    fn visit_expr_method_call_mut(&mut self, node: &mut syn::ExprMethodCall) {
        syn::visit_mut::visit_expr_method_call_mut(self, node);
        if !self.in_macro_input
            && node
                .turbofish
                .as_ref()
                .is_some_and(|args| args.args.is_empty())
        {
            node.turbofish = None;
        }
    }

    // `syn::Attribute` payloads are opaque macro input.
    fn visit_attribute_mut(&mut self, _node: &mut syn::Attribute) {}

    fn visit_angle_bracketed_generic_arguments_mut(
        &mut self,
        node: &mut syn::AngleBracketedGenericArguments,
    ) {
        syn::visit_mut::visit_angle_bracketed_generic_arguments_mut(self, node);
        if !self.in_macro_input {
            node.args.pop_punct();
            if self.compiles {
                for arg in &mut node.args {
                    if let syn::GenericArgument::Const(expr) = arg {
                        fold_scalar_argument_block(expr);
                    }
                }
            }
        }
    }

    fn visit_generics_mut(&mut self, generics: &mut syn::Generics) {
        let outer = self.in_macro_input;
        self.in_macro_input = outer
            || generics.params.iter().any(|param| {
                has_macro_attribute(match param {
                    syn::GenericParam::Lifetime(param) => &param.attrs,
                    syn::GenericParam::Type(param) => &param.attrs,
                    syn::GenericParam::Const(param) => &param.attrs,
                })
            });
        syn::visit_mut::visit_generics_mut(self, generics);
        bounds_to_where(generics);
        if !self.in_macro_input {
            generics.params.pop_punct();
        }
        if !self.in_macro_input
            && let Some(clause) = &mut generics.where_clause
        {
            for predicate in &mut clause.predicates {
                match predicate {
                    syn::WherePredicate::Type(predicate) => {
                        drop_trailing_plus(&mut predicate.bounds);
                    }
                    syn::WherePredicate::Lifetime(predicate) => {
                        drop_trailing_plus(&mut predicate.bounds);
                    }
                    _ => {}
                }
            }
        }
        self.in_macro_input = outer;
    }

    fn visit_item_trait_mut(&mut self, node: &mut syn::ItemTrait) {
        syn::visit_mut::visit_item_trait_mut(self, node);
        if !self.in_macro_input {
            drop_trailing_plus(&mut node.supertraits);
        }
    }

    fn visit_item_trait_alias_mut(&mut self, node: &mut syn::ItemTraitAlias) {
        syn::visit_mut::visit_item_trait_alias_mut(self, node);
        if !self.in_macro_input {
            drop_trailing_plus(&mut node.bounds);
        }
    }

    fn visit_trait_bound_mut(&mut self, node: &mut syn::TraitBound) {
        syn::visit_mut::visit_trait_bound_mut(self, node);
        if !self.in_macro_input {
            fold_type_path(&mut node.path);
            if node.paren_token.is_some() && !fn_sugar_output(&node.path) {
                node.paren_token = None;
            }
        }
    }

    fn visit_trait_item_type_mut(&mut self, node: &mut syn::TraitItemType) {
        syn::visit_mut::visit_trait_item_type_mut(self, node);
        if !self.in_macro_input {
            drop_trailing_plus(&mut node.bounds);
        }
    }

    fn visit_type_impl_trait_mut(&mut self, node: &mut syn::TypeImplTrait) {
        syn::visit_mut::visit_type_impl_trait_mut(self, node);
        if !self.in_macro_input {
            drop_trailing_plus(&mut node.bounds);
        }
    }

    fn visit_type_trait_object_mut(&mut self, node: &mut syn::TypeTraitObject) {
        syn::visit_mut::visit_type_trait_object_mut(self, node);
        if !self.in_macro_input {
            drop_trailing_plus(&mut node.bounds);
        }
    }

    fn visit_constraint_mut(&mut self, node: &mut syn::Constraint) {
        syn::visit_mut::visit_constraint_mut(self, node);
        if !self.in_macro_input {
            drop_trailing_plus(&mut node.bounds);
        }
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
        if !self.in_macro_input
            && matches!(node, syn::PathArguments::AngleBracketed(args) if args.args.is_empty())
        {
            *node = syn::PathArguments::None;
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
            if !self.in_macro_input {
                fold_type_path(path);
            }
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
        let derived = match item {
            syn::Item::Struct(syn::ItemStruct { attrs, .. })
            | syn::Item::Enum(syn::ItemEnum { attrs, .. })
            | syn::Item::Union(syn::ItemUnion { attrs, .. }) => Some(attrs),
            _ => None,
        };
        let macro_derived = derived
            .as_ref()
            .is_some_and(|attrs| has_macro_derive(attrs));
        if let Some(attrs) = derived {
            fold_std_derives(attrs);
        }
        if !macro_derived && let Some(attrs) = item_attrs(item) {
            strip_inert_attrs(attrs);
        }
        let macro_input = self.in_macro_input
            || macro_derived
            || item_attrs(item).is_some_and(|attrs| has_macro_attribute(attrs));
        let outer = core::mem::replace(&mut self.in_macro_input, macro_input);
        syn::visit_mut::visit_item_mut(self, item);
        self.in_macro_input = outer;
    }

    fn visit_impl_item_mut(&mut self, item: &mut syn::ImplItem) {
        let attrs = match item {
            syn::ImplItem::Const(v) => Some(&mut v.attrs),
            syn::ImplItem::Fn(v) => Some(&mut v.attrs),
            syn::ImplItem::Type(v) => Some(&mut v.attrs),
            syn::ImplItem::Macro(v) => Some(&mut v.attrs),
            _ => None,
        };
        let macro_input = self.in_macro_input
            || attrs
                .as_ref()
                .is_some_and(|attrs| has_macro_attribute(attrs));
        if let Some(attrs) = attrs {
            strip_inert_attrs(attrs);
        }
        let outer = core::mem::replace(&mut self.in_macro_input, macro_input);
        syn::visit_mut::visit_impl_item_mut(self, item);
        self.in_macro_input = outer;
    }

    fn visit_trait_item_mut(&mut self, item: &mut syn::TraitItem) {
        let attrs = match item {
            syn::TraitItem::Const(v) => Some(&mut v.attrs),
            syn::TraitItem::Fn(v) => Some(&mut v.attrs),
            syn::TraitItem::Type(v) => Some(&mut v.attrs),
            syn::TraitItem::Macro(v) => Some(&mut v.attrs),
            _ => None,
        };
        let macro_input = self.in_macro_input
            || attrs
                .as_ref()
                .is_some_and(|attrs| has_macro_attribute(attrs));
        if let Some(attrs) = attrs {
            strip_inert_attrs(attrs);
        }
        let outer = core::mem::replace(&mut self.in_macro_input, macro_input);
        syn::visit_mut::visit_trait_item_mut(self, item);
        self.in_macro_input = outer;
    }

    fn visit_foreign_item_mut(&mut self, item: &mut syn::ForeignItem) {
        let attrs = match item {
            syn::ForeignItem::Fn(v) => Some(&v.attrs),
            syn::ForeignItem::Static(v) => Some(&v.attrs),
            syn::ForeignItem::Type(v) => Some(&v.attrs),
            syn::ForeignItem::Macro(v) => Some(&v.attrs),
            _ => None,
        };
        let macro_input =
            self.in_macro_input || attrs.is_some_and(|attrs| has_macro_attribute(attrs));
        let outer = core::mem::replace(&mut self.in_macro_input, macro_input);
        syn::visit_mut::visit_foreign_item_mut(self, item);
        self.in_macro_input = outer;
    }

    fn visit_field_mut(&mut self, field: &mut syn::Field) {
        if !self.in_macro_input {
            strip_inert_attrs(&mut field.attrs);
        }
        let outer = self.enter_attrs(&field.attrs);
        syn::visit_mut::visit_field_mut(self, field);
        self.in_macro_input = outer;
    }

    fn visit_variant_mut(&mut self, variant: &mut syn::Variant) {
        if !self.in_macro_input {
            strip_inert_attrs(&mut variant.attrs);
        }
        let outer = self.enter_attrs(&variant.attrs);
        syn::visit_mut::visit_variant_mut(self, variant);
        self.in_macro_input = outer;
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

impl Drift {
    fn enter_attrs(&mut self, attrs: &[syn::Attribute]) -> bool {
        let outer = self.in_macro_input;
        self.in_macro_input = outer || has_macro_attribute(attrs);
        outer
    }

    /// Fold a function body's tail `return`, and its tail semicolon when the
    /// signature makes the value `()`, before and after the return fold since
    /// each can expose the other.
    fn fold_fn_body(&self, sig: &syn::Signature, block: &mut syn::Block) {
        let unit = self.compiles && matches!(sig.output, syn::ReturnType::Default);
        if unit {
            drop_tail_semicolon(&mut block.stmts);
        }
        fold_tail_return(&mut block.stmts);
        if unit {
            drop_tail_semicolon(&mut block.stmts);
        }
    }
}

/// The block of a loop, or the then block of an `if` chain without a final
/// `else`, whose value rustc requires to be `()`.
fn unit_block(expr: &mut syn::Expr) -> Option<&mut syn::Block> {
    match expr {
        syn::Expr::ForLoop(e) => Some(&mut e.body),
        syn::Expr::While(e) => Some(&mut e.body),
        syn::Expr::Loop(e) => Some(&mut e.body),
        syn::Expr::If(e) if discards_value(e) => Some(&mut e.then_branch),
        _ => None,
    }
}

/// Whether the `if` chain from `expr_if` ends without an `else` block.
fn discards_value(mut expr_if: &syn::ExprIf) -> bool {
    loop {
        match expr_if.else_branch.as_ref().map(|(_, e)| e.as_ref()) {
            None => return true,
            Some(syn::Expr::If(inner)) => expr_if = inner,
            Some(_) => return false,
        }
    }
}

/// Drop the `mut` of a pattern that is one binding, by-value whatever the
/// scrutinee. Nested under a reference, `mut` resets a borrowing binding mode
/// before edition 2024, so a nested one stays.
fn drop_binding_mut(pat: &mut syn::Pat) {
    let pat = match pat {
        syn::Pat::Type(typed) => typed.pat.as_mut(),
        syn::Pat::Guard(guarded) => guarded.pat.as_mut(),
        other => other,
    };
    if let syn::Pat::Ident(ident) = pat
        && ident.by_ref.is_none()
    {
        ident.mutability = None;
    }
}

/// Normalize an explicit `Self` receiver without changing its binding or borrow kind.
fn fold_explicit_receiver(receiver: &mut syn::Receiver) {
    if matches!(
        &receiver.kind,
        syn::ReceiverKind::Typed(_, ty)
            if matches!(ty.as_ref(), syn::Type::Path(path)
                if path.attrs.is_empty() && path.qself.is_none() && path.path.is_ident("Self"))
    ) {
        receiver.kind = syn::ReceiverKind::Value;
        return;
    }
    if receiver.mutability.is_some() {
        return;
    }
    let syn::ReceiverKind::Typed(_colon, ty) = &mut receiver.kind else {
        return;
    };
    let syn::Type::Reference(reference) = &mut **ty else {
        return;
    };
    if reference.mutability.is_some() {
        return;
    }
    let syn::Type::Path(path) = reference.elem.as_ref() else {
        return;
    };
    if path.attrs.is_empty() && path.qself.is_none() && path.path.is_ident("Self") {
        let (ampersand, lifetime) = (
            reference.and_token,
            core::mem::take(&mut reference.lifetime),
        );
        receiver.kind = syn::ReceiverKind::Reference(ampersand, lifetime, None);
    }
}

fn fold_pattern_spelling(pat: &mut syn::Pat) {
    match pat {
        syn::Pat::Or(pat) => pat.leading_vert = None,
        syn::Pat::Ident(pat)
            if pat.subpat.as_ref().is_some_and(
                |(_, sub)| matches!(sub.as_ref(), syn::Pat::Wild(wild) if wild.attrs.is_empty()),
            ) =>
        {
            let name = pat.ident.to_string();
            let name = name.strip_prefix("r#").unwrap_or(&name);
            if !name.starts_with(char::is_uppercase) {
                pat.subpat = None;
            }
        }
        _ => {}
    }
}

/// Drop the semicolon of a tail statement whose value is `()` either way, the
/// rewrite clippy's `semicolon_if_nothing_returned` makes in reverse.
fn drop_tail_semicolon(stmts: &mut [syn::Stmt]) {
    match stmts.last_mut() {
        Some(syn::Stmt::Expr(_, semi)) => *semi = None,
        Some(syn::Stmt::Macro(mac)) => mac.semi_token = None,
        _ => {}
    }
}

/// Drop the semicolon after every non-tail statement ending in a block, an
/// empty statement rustc's `redundant_semicolons` reports.
fn drop_block_statement_semicolons(stmts: &mut [syn::Stmt]) {
    let Some((_, rest)) = stmts.split_last_mut() else {
        return;
    };
    for stmt in rest {
        if let syn::Stmt::Expr(expr, semi) = stmt
            && matches!(
                expr,
                syn::Expr::If(_)
                    | syn::Expr::Match(_)
                    | syn::Expr::Block(_)
                    | syn::Expr::Unsafe(_)
                    | syn::Expr::While(_)
                    | syn::Expr::Loop(_)
                    | syn::Expr::ForLoop(_)
                    | syn::Expr::TryBlock(_)
                    | syn::Expr::Const(_)
            )
        {
            *semi = None;
        }
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
        && matches!(ty.as_ref(), syn::Type::Tuple(t) if t.attrs.is_empty() && t.elems.is_empty())
    {
        *output = syn::ReturnType::Default;
    }
}

fn fold_fn_pointer_lifetime(node: &mut syn::TypeFnPtr) {
    use syn::visit::Visit;

    let Some(bound) = &node.lifetimes else {
        return;
    };
    let Some(syn::GenericParam::Lifetime(binder)) = bound.lifetimes.first() else {
        return;
    };
    if bound.lifetimes.len() != 1
        || node.inputs.len() != 1
        || node.variadic.is_some()
        || !binder.attrs.is_empty()
        || !binder.bounds.is_empty()
        || !node.inputs[0].attrs.is_empty()
    {
        return;
    }
    let spelling = binder.lifetime.ident.to_string();
    if spelling.starts_with("r#") || matches!(spelling.as_str(), "static" | "_") {
        return;
    }
    let syn::Type::Reference(reference) = &mut node.inputs[0].ty else {
        return;
    };
    if reference.lifetime.as_ref() != Some(&binder.lifetime) {
        return;
    }
    let mut uses = LifetimeUses {
        name: &binder.lifetime.ident,
        count: 0,
        opaque: false,
    };
    uses.visit_type_reference(reference);
    uses.visit_return_type(&node.output);
    if uses.opaque || uses.count != 1 {
        return;
    }
    reference.lifetime = None;
    node.lifetimes = None;
}

struct LifetimeUses<'a> {
    name: &'a syn::Ident,
    count: usize,
    opaque: bool,
}

impl<'ast> syn::visit::Visit<'ast> for LifetimeUses<'_> {
    fn visit_lifetime(&mut self, lifetime: &'ast syn::Lifetime) {
        self.count += usize::from(lifetime.ident == *self.name);
    }

    fn visit_attribute(&mut self, _: &'ast syn::Attribute) {
        self.opaque = true;
    }

    fn visit_bound_lifetimes(&mut self, _: &'ast syn::BoundLifetimes) {
        self.opaque = true;
    }

    fn visit_type(&mut self, ty: &'ast syn::Type) {
        if matches!(
            ty,
            syn::Type::FnPtr(_) | syn::Type::Group(_) | syn::Type::Verbatim(_)
        ) {
            self.opaque = true;
        }
        syn::visit::visit_type(self, ty);
    }

    fn visit_macro(&mut self, _: &'ast syn::Macro) {
        self.opaque = true;
    }

    fn visit_expr(&mut self, expr: &'ast syn::Expr) {
        if matches!(expr, syn::Expr::Group(_) | syn::Expr::Verbatim(_)) {
            self.opaque = true;
        }
        syn::visit::visit_expr(self, expr);
    }

    fn visit_type_param_bound(&mut self, bound: &'ast syn::TypeParamBound) {
        if matches!(bound, syn::TypeParamBound::Verbatim(_)) {
            self.opaque = true;
        }
        syn::visit::visit_type_param_bound(self, bound);
    }

    fn visit_token_stream(&mut self, _: &'ast TokenStream) {
        self.opaque = true;
    }

    fn visit_lit(&mut self, literal: &'ast syn::Lit) {
        if matches!(literal, syn::Lit::Verbatim(_)) {
            self.opaque = true;
        }
        syn::visit::visit_lit(self, literal);
    }
}

fn fold_empty_else(expr_if: &mut syn::ExprIf) {
    if expr_if
        .else_branch
        .as_ref()
        .is_some_and(|(_, alternative)| {
            matches!(
                alternative.as_ref(),
                syn::Expr::Block(block)
                    if block.attrs.is_empty()
                        && block.label.is_none()
                        && block.block.stmts.is_empty()
            )
        })
    {
        expr_if.else_branch = None;
    }
}

fn fold_scalar_const(expr: &mut syn::Expr) {
    let syn::Expr::Const(block) = expr else {
        return;
    };
    if block.attrs.is_empty()
        && scalar_tail(&block.block.stmts)
        && let Some(syn::Stmt::Expr(value, None)) = block.block.stmts.pop()
    {
        *expr = value;
    }
}

fn fold_scalar_argument_block(expr: &mut syn::Expr) {
    let syn::Expr::Block(block) = expr else {
        return;
    };
    if block.attrs.is_empty()
        && block.label.is_none()
        && scalar_tail(&block.block.stmts)
        && let Some(syn::Stmt::Expr(value, None)) = block.block.stmts.pop()
    {
        *expr = value;
    }
}

fn scalar_tail(stmts: &[syn::Stmt]) -> bool {
    matches!(
        stmts,
        [syn::Stmt::Expr(syn::Expr::Lit(literal), None)]
            if literal.attrs.is_empty()
                && matches!(
                    literal.lit,
                    syn::Lit::Int(_)
                        | syn::Lit::Float(_)
                        | syn::Lit::Bool(_)
                        | syn::Lit::Char(_)
                        | syn::Lit::Byte(_)
                )
    )
}

fn fold_signed_zero(expr: &mut syn::Expr) {
    let syn::Expr::Unary(unary) = expr else {
        return;
    };
    let syn::Expr::Lit(literal) = unary.expr.as_ref() else {
        return;
    };
    if unary.attrs.is_empty()
        && literal.attrs.is_empty()
        && matches!(unary.op, syn::UnOp::Neg(_))
        && matches!(
            &literal.lit,
            syn::Lit::Int(integer)
                if integer.base10_digits().bytes().all(|digit| digit == b'0')
                    && ["i8", "i16", "i32", "i64", "i128", "isize"]
                        .contains(&integer.suffix())
        )
    {
        *expr = core::mem::replace(unary.expr.as_mut(), syn::Expr::Verbatim(TokenStream::new()));
    }
}

fn fold_type_path(path: &mut syn::Path) {
    for segment in &mut path.segments {
        if let syn::PathArguments::AngleBracketed(args) = &mut segment.arguments {
            args.colon2_token = None;
        }
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

/// Bounds retain their order and values.
fn drop_trailing_plus<T>(bounds: &mut syn::punctuated::Punctuated<T, syn::Token![+]>) {
    bounds.pop_punct();
}

/// Function-trait output types can absorb a following bound separator.
fn fn_sugar_output(path: &syn::Path) -> bool {
    matches!(
        path.segments.last().map(|segment| &segment.arguments),
        Some(syn::PathArguments::Parenthesized(args)) if matches!(args.output, syn::ReturnType::Type(..))
    )
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
pub(crate) const STD_DERIVES: [&str; 9] = [
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

/// Whether `attrs` derive anything outside the std derives, a proc macro that reads the item.
pub(crate) fn has_macro_derive(attrs: &[syn::Attribute]) -> bool {
    use syn::parse::Parser;

    let parser = syn::punctuated::Punctuated::<syn::Path, syn::Token![,]>::parse_terminated;
    attrs
        .iter()
        .filter(|attr| attr.path().is_ident("derive"))
        .any(|attr| {
            let syn::Meta::List(list) = &attr.meta else {
                return true;
            };
            parser.parse2(list.tokens.clone()).map_or(true, |paths| {
                paths.iter().any(|path| {
                    path.get_ident()
                        .is_none_or(|name| !STD_DERIVES.iter().any(|std| name == std))
                })
            })
        })
}

pub(crate) fn has_macro_attribute(attrs: &[syn::Attribute]) -> bool {
    attrs.iter().any(|attr| {
        !is_inert_attr(attr)
            && ![
                "cfg",
                "derive",
                "inline",
                "cold",
                "repr",
                "must_use",
                "deprecated",
                "non_exhaustive",
                "track_caller",
                "no_mangle",
                "export_name",
                "link_name",
                "link_section",
                "automatically_derived",
            ]
            .iter()
            .any(|name| attr.path().is_ident(name))
    })
}

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
/// comments, lint-level directives, and the `rustfmt::` tool attributes. A
/// `doc` whose value is a macro call runs at compile time and can fail, so it
/// stays.
fn is_inert_attr(attr: &syn::Attribute) -> bool {
    let p = attr.path();
    (p.is_ident("doc") && !computed_doc(&attr.meta))
        || p.is_ident("allow")
        || p.is_ident("expect")
        || p.is_ident("warn")
        || p.segments.first().is_some_and(|s| s.ident == "rustfmt")
}

/// Whether `meta` is a `doc = …` whose value is a macro call.
fn computed_doc(meta: &syn::Meta) -> bool {
    matches!(meta, syn::Meta::NameValue(nv) if matches!(nv.value, syn::Expr::Macro(_)))
}

/// Drop inert attributes from the list.
fn strip_inert_attrs(attrs: &mut Vec<syn::Attribute>) {
    attrs.retain(|attr| !is_inert_attr(attr));
}

fn pat_attrs(pat: &mut syn::Pat) -> Option<&mut Vec<syn::Attribute>> {
    Some(match pat {
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
        _ => return None,
    })
}

fn type_attrs(ty: &syn::Type) -> &[syn::Attribute] {
    match ty {
        syn::Type::Array(ty) => &ty.attrs,
        syn::Type::FnPtr(ty) => &ty.attrs,
        syn::Type::Group(ty) => &ty.attrs,
        syn::Type::ImplTrait(ty) => &ty.attrs,
        syn::Type::Macro(ty) => &ty.attrs,
        syn::Type::Never(ty) => &ty.attrs,
        syn::Type::Paren(ty) => &ty.attrs,
        syn::Type::Path(ty) => &ty.attrs,
        syn::Type::Ptr(ty) => &ty.attrs,
        syn::Type::Reference(ty) => &ty.attrs,
        syn::Type::Slice(ty) => &ty.attrs,
        syn::Type::TraitObject(ty) => &ty.attrs,
        syn::Type::Tuple(ty) => &ty.attrs,
        _ => &[],
    }
}

fn expr_attrs(expr: &syn::Expr) -> &[syn::Attribute] {
    match expr {
        syn::Expr::Array(expr) => &expr.attrs,
        syn::Expr::Assign(expr) => &expr.attrs,
        syn::Expr::Async(expr) => &expr.attrs,
        syn::Expr::Await(expr) => &expr.attrs,
        syn::Expr::Binary(expr) => &expr.attrs,
        syn::Expr::Block(expr) => &expr.attrs,
        syn::Expr::Break(expr) => &expr.attrs,
        syn::Expr::Call(expr) => &expr.attrs,
        syn::Expr::Cast(expr) => &expr.attrs,
        syn::Expr::Closure(expr) => &expr.attrs,
        syn::Expr::Const(expr) => &expr.attrs,
        syn::Expr::Continue(expr) => &expr.attrs,
        syn::Expr::Field(expr) => &expr.attrs,
        syn::Expr::ForLoop(expr) => &expr.attrs,
        syn::Expr::Group(expr) => &expr.attrs,
        syn::Expr::If(expr) => &expr.attrs,
        syn::Expr::Index(expr) => &expr.attrs,
        syn::Expr::Let(expr) => &expr.attrs,
        syn::Expr::Lit(expr) => &expr.attrs,
        syn::Expr::Loop(expr) => &expr.attrs,
        syn::Expr::Macro(expr) => &expr.attrs,
        syn::Expr::Match(expr) => &expr.attrs,
        syn::Expr::MethodCall(expr) => &expr.attrs,
        syn::Expr::Paren(expr) => &expr.attrs,
        syn::Expr::Path(expr) => &expr.attrs,
        syn::Expr::Range(expr) => &expr.attrs,
        syn::Expr::RawAddr(expr) => &expr.attrs,
        syn::Expr::Reference(expr) => &expr.attrs,
        syn::Expr::Repeat(expr) => &expr.attrs,
        syn::Expr::Return(expr) => &expr.attrs,
        syn::Expr::Struct(expr) => &expr.attrs,
        syn::Expr::Try(expr) => &expr.attrs,
        syn::Expr::TryBlock(expr) => &expr.attrs,
        syn::Expr::Tuple(expr) => &expr.attrs,
        syn::Expr::Unary(expr) => &expr.attrs,
        syn::Expr::Unsafe(expr) => &expr.attrs,
        syn::Expr::While(expr) => &expr.attrs,
        syn::Expr::Yield(expr) => &expr.attrs,
        _ => &[],
    }
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
