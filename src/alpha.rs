//! Alpha-renaming of local binders in a parsed doctest.
//!
//! Every identifier the doctest itself binds (`let` patterns, function
//! names and parameters, closures, generics, lifetimes, local types,
//! `const`/`static` items, local `macro_rules!` names, and `use`
//! aliases) is rewritten to a positional name such as `_dejadoc_0`, so
//! two bodies that differ only in local names collapse to the same
//! canonical form. Free identifiers, field names, method names,
//! attribute paths, and string literals are never rewritten.

use alloc::boxed::Box;
use alloc::collections::BTreeMap;
use alloc::format;
use alloc::string::String;
use alloc::string::ToString;
use alloc::vec;
use alloc::vec::Vec;

use proc_macro2::Ident;
use syn::visit_mut::VisitMut;

/// A canonical binder name, positional in the deterministic traversal.
fn canonical(counter: &mut usize) -> String {
    let name = format!("_dejadoc_{counter}");
    *counter += 1;
    name
}

/// An identifier namespace.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Ns {
    /// Variables, function names, const items.
    Value,
    /// Types, traits, and the `Self` alias of an impl or trait.
    Type,
    /// Lifetimes.
    Lifetime,
    /// Local macro names.
    Macro,
}

/// A scope frame, one per block, function, generics list, impl, or closure.
struct Frame {
    value: BTreeMap<String, String>,
    ty: BTreeMap<String, String>,
    lifetime: BTreeMap<String, String>,
    mac: BTreeMap<String, String>,
}

impl Frame {
    fn new() -> Self {
        Self {
            value: BTreeMap::new(),
            ty: BTreeMap::new(),
            lifetime: BTreeMap::new(),
            mac: BTreeMap::new(),
        }
    }

    fn bind(&mut self, ns: Ns, name: String, canon: String) {
        match ns {
            Ns::Value => self.value.insert(name, canon),
            Ns::Type => self.ty.insert(name, canon),
            Ns::Lifetime => self.lifetime.insert(name, canon),
            Ns::Macro => self.mac.insert(name, canon),
        };
    }

    fn lookup(&self, ns: Ns, name: &str) -> Option<&String> {
        match ns {
            Ns::Value => self.value.get(name),
            Ns::Type => self.ty.get(name),
            Ns::Lifetime => self.lifetime.get(name),
            Ns::Macro => self.mac.get(name),
        }
    }
}

/// The scope stack and the positional counter.
struct Renamer {
    frames: Vec<Frame>,
    counter: usize,
}

impl Renamer {
    fn new() -> Self {
        Self {
            frames: vec![Frame::new()],
            counter: 0,
        }
    }

    fn push(&mut self) {
        self.frames.push(Frame::new());
    }

    fn pop(&mut self) {
        self.frames.pop();
    }

    /// Bind `name` in the current frame and return its canonical form.
    fn bind(&mut self, ns: Ns, name: &str) -> String {
        let canon = canonical(&mut self.counter);
        self.top_bind(ns, name, canon.clone());
        canon
    }

    /// Bind `name` to `canon` in the current frame.
    fn top_bind(&mut self, ns: Ns, name: &str, canon: String) {
        self.frames
            .last_mut()
            .expect("a scope frame")
            .bind(ns, name.to_string(), canon);
    }

    /// Nearest-binder lookup, namespace priority first.
    fn lookup(&self, ns: &[Ns], name: &str) -> Option<String> {
        for ns in ns {
            for frame in self.frames.iter().rev() {
                if let Some(canon) = frame.lookup(*ns, name) {
                    return Some(canon.clone());
                }
            }
        }
        None
    }

    /// Rewrite the tokens of a macro call against the local bindings.
    fn rename_tokens(&mut self, tokens: proc_macro2::TokenStream) -> proc_macro2::TokenStream {
        let trees: Vec<proc_macro2::TokenTree> = tokens
            .into_iter()
            .map(|tree| match tree {
                proc_macro2::TokenTree::Ident(mut ident) => {
                    let name = ident.to_string();
                    if let Some(canon) = self.lookup(&[Ns::Value, Ns::Type], &name) {
                        ident = Ident::new(&canon, ident.span());
                    }
                    proc_macro2::TokenTree::Ident(ident)
                }
                proc_macro2::TokenTree::Group(group) => {
                    let inner = self.rename_tokens(group.stream());
                    let mut rebuilt = proc_macro2::Group::new(group.delimiter(), inner);
                    rebuilt.set_span(group.span());
                    proc_macro2::TokenTree::Group(rebuilt)
                }
                other => other,
            })
            .collect();
        proc_macro2::TokenStream::from_iter(trees)
    }
}

/// Alpha-normalize a parsed file in place.
pub(crate) fn normalize_file(file: &mut syn::File) {
    let mut renamer = Renamer::new();
    renamer.visit_file_mut(file);
}

/// Alpha-normalize a parsed block in place.
pub(crate) fn normalize_block(block: &mut syn::Block) {
    let mut renamer = Renamer::new();
    renamer.visit_block_mut(block);
}

/// Alpha-normalize a parsed expression in place.
pub(crate) fn normalize_expr(expr: &mut syn::Expr) {
    let mut renamer = Renamer::new();
    renamer.visit_expr_mut(expr);
}

/// Rewrite the binder identifiers of `pat` to their canonical names.
fn rewrite_pat_bindings(renamer: &mut Renamer, pat: &mut syn::Pat) {
    match pat {
        syn::Pat::Ident(id) => {
            let name = id.ident.to_string();
            if let Some(canon) = renamer.lookup(&[Ns::Value], &name) {
                id.ident = Ident::new(&canon, id.ident.span());
            }
            if let Some((_, sub)) = &mut id.subpat {
                rewrite_pat_bindings(renamer, sub);
            }
        }
        syn::Pat::Tuple(tuple) => {
            for pat in &mut tuple.elems {
                rewrite_pat_bindings(renamer, pat);
            }
        }
        syn::Pat::Slice(slice) => {
            for pat in &mut slice.elems {
                rewrite_pat_bindings(renamer, pat);
            }
        }
        syn::Pat::TupleStruct(tuple) => {
            for pat in &mut tuple.elems {
                rewrite_pat_bindings(renamer, pat);
            }
        }
        syn::Pat::Struct(r#struct) => {
            for field in &mut r#struct.fields {
                // A shorthand `S { x }` binds the member name and
                // drops the member when rendered, so force the
                // explicit `x: x` form before the rewrite.
                if field.colon_token.is_none() {
                    field.colon_token = Some(syn::token::Colon::default());
                }
                rewrite_pat_bindings(renamer, &mut field.pat);
            }
        }
        syn::Pat::Or(r#or) => {
            for pat in &mut r#or.cases {
                rewrite_pat_bindings(renamer, pat);
            }
        }
        syn::Pat::Reference(reference) => rewrite_pat_bindings(renamer, &mut reference.pat),
        syn::Pat::Paren(paren) => rewrite_pat_bindings(renamer, &mut paren.pat),
        syn::Pat::Type(type_pat) => rewrite_pat_bindings(renamer, &mut type_pat.pat),
        syn::Pat::Guard(guard) => rewrite_pat_bindings(renamer, &mut guard.pat),
        _ => {}
    }
}

/// Collect the names bound by `pat`, in traversal order.
fn pattern_names(pat: &syn::Pat, out: &mut Vec<String>) {
    match pat {
        syn::Pat::Ident(id) => {
            out.push(id.ident.to_string());
            if let Some((_, sub)) = &id.subpat {
                pattern_names(sub, out);
            }
        }
        syn::Pat::Tuple(tuple) => {
            for pat in &tuple.elems {
                pattern_names(pat, out);
            }
        }
        syn::Pat::Slice(slice) => {
            for pat in &slice.elems {
                pattern_names(pat, out);
            }
        }
        syn::Pat::TupleStruct(tuple) => {
            for pat in &tuple.elems {
                pattern_names(pat, out);
            }
        }
        syn::Pat::Struct(r#struct) => {
            for field in &r#struct.fields {
                pattern_names(&field.pat, out);
            }
        }
        syn::Pat::Or(r#or) => {
            for pat in &r#or.cases {
                pattern_names(pat, out);
            }
        }
        syn::Pat::Reference(reference) => pattern_names(&reference.pat, out),
        syn::Pat::Paren(paren) => pattern_names(&paren.pat, out),
        syn::Pat::Type(type_pat) => pattern_names(&type_pat.pat, out),
        syn::Pat::Guard(guard) => pattern_names(&guard.pat, out),
        _ => {}
    }
}

/// The names bound by the function parameters of `inputs`.
fn fn_param_names<'a>(inputs: impl Iterator<Item = &'a syn::FnArg>) -> Vec<String> {
    let mut out = Vec::new();
    for input in inputs {
        if let syn::FnArg::Typed(pat_type) = input {
            pattern_names(&pat_type.pat, &mut out);
        }
    }
    out
}

/// Bind a use name in both namespaces and return its value canon.
fn bind_use_name(renamer: &mut Renamer, name: &str) -> String {
    let canon = renamer.bind(Ns::Value, name);
    renamer.bind(Ns::Type, name);
    canon
}

impl Renamer {
    /// Bind every name a use tree introduces and rewrite the tree. A plain
    /// name is both the imported item and its local alias, so it becomes
    /// `item as canon` to keep the item and rename the alias.
    fn bind_use_tree(&mut self, tree: syn::UseTree) -> syn::UseTree {
        match tree {
            syn::UseTree::Path(mut path) => {
                path.tree = Box::new(self.bind_use_tree(*path.tree));
                syn::UseTree::Path(path)
            }
            syn::UseTree::Name(syn::UseName { ident }) => {
                let canon = bind_use_name(self, &ident.to_string());
                syn::UseTree::Rename(syn::UseRename {
                    rename: Ident::new(&canon, ident.span()),
                    ident,
                    as_token: <syn::Token![as]>::default(),
                })
            }
            syn::UseTree::Rename(mut rename) => {
                let canon = bind_use_name(self, &rename.rename.to_string());
                rename.rename = Ident::new(&canon, rename.rename.span());
                syn::UseTree::Rename(rename)
            }
            syn::UseTree::Group(mut group) => {
                group.items = group
                    .items
                    .into_pairs()
                    .map(|pair| {
                        let (tree, comma) = pair.into_tuple();
                        syn::punctuated::Pair::new(self.bind_use_tree(tree), comma)
                    })
                    .collect();
                syn::UseTree::Group(group)
            }
            glob @ syn::UseTree::Glob(_) => glob,
        }
    }
}

/// Visit function parameters and rewrite their binder patterns.
fn visit_fn_inputs<'a>(renamer: &mut Renamer, inputs: impl Iterator<Item = &'a mut syn::FnArg>) {
    for input in inputs {
        syn::visit_mut::visit_fn_arg_mut(renamer, input);
        if let syn::FnArg::Typed(pat_type) = input {
            rewrite_pat_bindings(renamer, &mut pat_type.pat);
        }
    }
}

/// The identifier of a bare single-segment type, if any.
fn single_segment_type_name(ty: &syn::Type) -> Option<String> {
    let syn::Type::Path(path) = ty else {
        return None;
    };
    if path.qself.is_some() || path.path.segments.len() != 1 {
        return None;
    }
    let segment = path.path.segments.first()?;
    if matches!(segment.arguments, syn::PathArguments::None) {
        Some(segment.ident.to_string())
    } else {
        None
    }
}

/// Push a generics frame. All parameters are bound and rewritten first
/// so that a bound may name a later parameter, then the bounds are
/// visited with every parameter in scope.
fn begin_generics(renamer: &mut Renamer, generics: &mut syn::Generics) {
    renamer.push();
    for param in &mut generics.params {
        match param {
            syn::GenericParam::Lifetime(lifetime) => {
                let name = lifetime.lifetime.ident.to_string();
                let canon = renamer.bind(Ns::Lifetime, &name);
                lifetime.lifetime.ident = Ident::new(&canon, lifetime.lifetime.ident.span());
            }
            syn::GenericParam::Type(ty_param) => {
                let name = ty_param.ident.to_string();
                let canon = renamer.bind(Ns::Type, &name);
                ty_param.ident = Ident::new(&canon, ty_param.ident.span());
            }
            syn::GenericParam::Const(r#const) => {
                let name = r#const.ident.to_string();
                let canon = renamer.bind(Ns::Value, &name);
                r#const.ident = Ident::new(&canon, r#const.ident.span());
            }
        }
    }
    for param in &mut generics.params {
        syn::visit_mut::visit_generic_param_mut(renamer, param);
    }
}

impl VisitMut for Renamer {
    fn visit_block_mut(&mut self, block: &mut syn::Block) {
        self.push();
        syn::visit_mut::visit_block_mut(self, block);
        self.pop();
    }

    fn visit_local_mut(&mut self, local: &mut syn::Local) {
        // The initializer, and a `let .. else` branch, resolve under the
        // outer bindings.
        if let Some(init) = &mut local.init {
            syn::visit_mut::visit_expr_mut(self, &mut init.expr);
            if let Some(diverge) = &mut init.diverge {
                syn::visit_mut::visit_expr_mut(self, &mut diverge.1);
            }
        }
        // The pattern's names become visible from here on.
        let mut names = Vec::new();
        pattern_names(&local.pat, &mut names);
        for name in &names {
            self.bind(Ns::Value, name);
        }
        syn::visit_mut::visit_pat_mut(self, &mut local.pat);
        rewrite_pat_bindings(self, &mut local.pat);
    }

    fn visit_item_fn_mut(&mut self, item: &mut syn::ItemFn) {
        let name = item.sig.ident.to_string();
        let canon = self.bind(Ns::Value, &name);
        item.sig.ident = Ident::new(&canon, item.sig.ident.span());
        begin_generics(self, &mut item.sig.generics);
        for attr in &mut item.attrs {
            self.visit_attribute_mut(attr);
        }
        self.push();
        for param in fn_param_names(item.sig.inputs.iter()) {
            self.bind(Ns::Value, &param);
        }
        visit_fn_inputs(self, item.sig.inputs.iter_mut());
        syn::visit_mut::visit_return_type_mut(self, &mut item.sig.output);
        self.visit_block_mut(&mut item.block);
        self.pop();
        self.pop();
    }

    fn visit_impl_item_fn_mut(&mut self, item: &mut syn::ImplItemFn) {
        begin_generics(self, &mut item.sig.generics);
        for attr in &mut item.attrs {
            self.visit_attribute_mut(attr);
        }
        self.push();
        for param in fn_param_names(item.sig.inputs.iter()) {
            self.bind(Ns::Value, &param);
        }
        visit_fn_inputs(self, item.sig.inputs.iter_mut());
        syn::visit_mut::visit_return_type_mut(self, &mut item.sig.output);
        self.visit_block_mut(&mut item.block);
        self.pop();
        self.pop();
    }

    fn visit_trait_item_fn_mut(&mut self, item: &mut syn::TraitItemFn) {
        begin_generics(self, &mut item.sig.generics);
        for attr in &mut item.attrs {
            self.visit_attribute_mut(attr);
        }
        self.push();
        for param in fn_param_names(item.sig.inputs.iter()) {
            self.bind(Ns::Value, &param);
        }
        visit_fn_inputs(self, item.sig.inputs.iter_mut());
        syn::visit_mut::visit_return_type_mut(self, &mut item.sig.output);
        if let Some(default) = &mut item.default {
            self.visit_block_mut(default);
        }
        self.pop();
        self.pop();
    }

    fn visit_item_struct_mut(&mut self, item: &mut syn::ItemStruct) {
        let name = item.ident.to_string();
        let canon = self.bind(Ns::Type, &name);
        item.ident = Ident::new(&canon, item.ident.span());
        begin_generics(self, &mut item.generics);
        for attr in &mut item.attrs {
            self.visit_attribute_mut(attr);
        }
        syn::visit_mut::visit_fields_mut(self, &mut item.fields);
        self.pop();
    }

    fn visit_item_enum_mut(&mut self, item: &mut syn::ItemEnum) {
        let name = item.ident.to_string();
        let canon = self.bind(Ns::Type, &name);
        item.ident = Ident::new(&canon, item.ident.span());
        begin_generics(self, &mut item.generics);
        for attr in &mut item.attrs {
            self.visit_attribute_mut(attr);
        }
        for variant in &mut item.variants {
            syn::visit_mut::visit_variant_mut(self, variant);
        }
        self.pop();
    }

    fn visit_item_union_mut(&mut self, item: &mut syn::ItemUnion) {
        let name = item.ident.to_string();
        let canon = self.bind(Ns::Type, &name);
        item.ident = Ident::new(&canon, item.ident.span());
        begin_generics(self, &mut item.generics);
        for attr in &mut item.attrs {
            self.visit_attribute_mut(attr);
        }
        syn::visit_mut::visit_fields_named_mut(self, &mut item.fields);
        self.pop();
    }

    fn visit_item_type_mut(&mut self, item: &mut syn::ItemType) {
        let name = item.ident.to_string();
        let canon = self.bind(Ns::Type, &name);
        item.ident = Ident::new(&canon, item.ident.span());
        begin_generics(self, &mut item.generics);
        for attr in &mut item.attrs {
            self.visit_attribute_mut(attr);
        }
        syn::visit_mut::visit_type_mut(self, &mut item.ty);
        self.pop();
    }

    fn visit_item_trait_mut(&mut self, item: &mut syn::ItemTrait) {
        let name = item.ident.to_string();
        let canon = self.bind(Ns::Type, &name);
        item.ident = Ident::new(&canon, item.ident.span());
        self.top_bind(Ns::Type, "Self", canon);
        begin_generics(self, &mut item.generics);
        for attr in &mut item.attrs {
            self.visit_attribute_mut(attr);
        }
        for trait_item in &mut item.items {
            syn::visit_mut::visit_trait_item_mut(self, trait_item);
        }
        self.pop();
    }

    fn visit_item_impl_mut(&mut self, item: &mut syn::ItemImpl) {
        self.push();
        if let Some(name) = single_segment_type_name(&item.self_ty)
            && let Some(canon) = self.lookup(&[Ns::Type], &name)
        {
            self.top_bind(Ns::Type, "Self", canon);
        }
        let has_generics = item.generics.lt_token.is_some();
        if has_generics {
            begin_generics(self, &mut item.generics);
        }
        for attr in &mut item.attrs {
            self.visit_attribute_mut(attr);
        }
        syn::visit_mut::visit_type_mut(self, &mut item.self_ty);
        for impl_item in &mut item.items {
            syn::visit_mut::visit_impl_item_mut(self, impl_item);
        }
        if has_generics {
            self.pop();
        }
        self.pop();
    }

    fn visit_item_const_mut(&mut self, item: &mut syn::ItemConst) {
        let name = item.ident.to_string();
        let canon = self.bind(Ns::Value, &name);
        item.ident = Ident::new(&canon, item.ident.span());
        for attr in &mut item.attrs {
            self.visit_attribute_mut(attr);
        }
        syn::visit_mut::visit_type_mut(self, &mut item.ty);
        syn::visit_mut::visit_expr_mut(self, &mut item.expr);
    }

    fn visit_item_static_mut(&mut self, item: &mut syn::ItemStatic) {
        let name = item.ident.to_string();
        let canon = self.bind(Ns::Value, &name);
        item.ident = Ident::new(&canon, item.ident.span());
        for attr in &mut item.attrs {
            self.visit_attribute_mut(attr);
        }
        syn::visit_mut::visit_type_mut(self, &mut item.ty);
        syn::visit_mut::visit_expr_mut(self, &mut item.expr);
    }

    fn visit_item_macro_mut(&mut self, item: &mut syn::ItemMacro) {
        // `macro_rules! n {}` carries the name in `ident`; the legacy
        // `macro n {}` form does not occur in doctests.
        let Some(ident) = item.ident.as_mut() else {
            return;
        };
        let name = ident.to_string();
        let canon = self.bind(Ns::Macro, &name);
        *ident = Ident::new(&canon, ident.span());
        for attr in &mut item.attrs {
            self.visit_attribute_mut(attr);
        }
        // The definition body is macro pattern syntax, not Rust.
    }

    fn visit_item_use_mut(&mut self, item: &mut syn::ItemUse) {
        for attr in &mut item.attrs {
            self.visit_attribute_mut(attr);
        }
        let glob = syn::UseTree::Glob(syn::UseGlob {
            star_token: <syn::Token![*]>::default(),
        });
        item.tree = self.bind_use_tree(core::mem::replace(&mut item.tree, glob));
    }

    fn visit_expr_closure_mut(&mut self, closure: &mut syn::ExprClosure) {
        self.push();
        let mut names = Vec::new();
        for input in &closure.inputs {
            pattern_names(input, &mut names);
        }
        for name in &names {
            self.bind(Ns::Value, name);
        }
        for input in &mut closure.inputs {
            rewrite_pat_bindings(self, input);
        }
        syn::visit_mut::visit_expr_mut(self, &mut closure.body);
        self.pop();
    }

    fn visit_arm_mut(&mut self, arm: &mut syn::Arm) {
        let mut names = Vec::new();
        pattern_names(&arm.pat, &mut names);
        self.push();
        for name in &names {
            self.bind(Ns::Value, name);
        }
        syn::visit_mut::visit_pat_mut(self, &mut arm.pat);
        rewrite_pat_bindings(self, &mut arm.pat);
        syn::visit_mut::visit_expr_mut(self, &mut arm.body);
        self.pop();
    }

    fn visit_expr_for_loop_mut(&mut self, for_loop: &mut syn::ExprForLoop) {
        // The iterable resolves under the outer bindings.
        syn::visit_mut::visit_expr_mut(self, &mut for_loop.expr);
        let mut names = Vec::new();
        pattern_names(&for_loop.pat, &mut names);
        self.push();
        for name in &names {
            self.bind(Ns::Value, name);
        }
        syn::visit_mut::visit_pat_mut(self, &mut for_loop.pat);
        rewrite_pat_bindings(self, &mut for_loop.pat);
        self.visit_block_mut(&mut for_loop.body);
        self.pop();
    }

    fn visit_expr_if_mut(&mut self, expr: &mut syn::ExprIf) {
        // An `if let` binding is visible in the then branch only.
        if let syn::Expr::Let(let_expr) = expr.cond.as_mut() {
            let mut names = Vec::new();
            pattern_names(&let_expr.pat, &mut names);
            syn::visit_mut::visit_expr_mut(self, &mut let_expr.expr);
            self.push();
            for name in &names {
                self.bind(Ns::Value, name);
            }
            syn::visit_mut::visit_pat_mut(self, &mut let_expr.pat);
            rewrite_pat_bindings(self, &mut let_expr.pat);
            self.visit_block_mut(&mut expr.then_branch);
            self.pop();
            if let Some((_, else_expr)) = &mut expr.else_branch {
                syn::visit_mut::visit_expr_mut(self, else_expr);
            }
        } else {
            syn::visit_mut::visit_expr_mut(self, &mut expr.cond);
            self.visit_block_mut(&mut expr.then_branch);
            if let Some((_, else_expr)) = &mut expr.else_branch {
                syn::visit_mut::visit_expr_mut(self, else_expr);
            }
        }
    }

    fn visit_expr_while_mut(&mut self, expr: &mut syn::ExprWhile) {
        // A `while let` binding is visible in the body only.
        if let syn::Expr::Let(let_expr) = expr.cond.as_mut() {
            let mut names = Vec::new();
            pattern_names(&let_expr.pat, &mut names);
            syn::visit_mut::visit_expr_mut(self, &mut let_expr.expr);
            self.push();
            for name in &names {
                self.bind(Ns::Value, name);
            }
            syn::visit_mut::visit_pat_mut(self, &mut let_expr.pat);
            rewrite_pat_bindings(self, &mut let_expr.pat);
            self.visit_block_mut(&mut expr.body);
            self.pop();
        } else {
            syn::visit_mut::visit_expr_mut(self, &mut expr.cond);
            self.visit_block_mut(&mut expr.body);
        }
    }

    fn visit_path_mut(&mut self, path: &mut syn::Path) {
        // Only the first segment may name a local binder, and a leading
        // colon always names an extern crate.
        if path.leading_colon.is_none()
            && let Some(first) = path.segments.first_mut()
        {
            let name = first.ident.to_string();
            if let Some(canon) = self.lookup(&[Ns::Value, Ns::Type], &name) {
                first.ident = Ident::new(&canon, first.ident.span());
            }
        }
        for segment in &mut path.segments {
            match &mut segment.arguments {
                syn::PathArguments::AngleBracketed(args) => {
                    for arg in &mut args.args {
                        syn::visit_mut::visit_generic_argument_mut(self, arg);
                    }
                }
                syn::PathArguments::Parenthesized(args) => {
                    for input in &mut args.inputs {
                        syn::visit_mut::visit_named_arg_mut(self, input);
                    }
                    syn::visit_mut::visit_return_type_mut(self, &mut args.output);
                }
                syn::PathArguments::None => {}
            }
        }
    }

    fn visit_macro_mut(&mut self, mac: &mut syn::Macro) {
        if let Some(first) = mac.path.segments.first_mut() {
            let name = first.ident.to_string();
            if let Some(canon) = self.lookup(&[Ns::Macro], &name) {
                first.ident = Ident::new(&canon, first.ident.span());
            }
        }
        // Macro arguments may reference the local bindings.
        mac.tokens = self.rename_tokens(mac.tokens.clone());
    }

    fn visit_lifetime_mut(&mut self, lifetime: &mut syn::Lifetime) {
        let name = lifetime.ident.to_string();
        if let Some(canon) = self.lookup(&[Ns::Lifetime], &name) {
            lifetime.ident = Ident::new(&canon, lifetime.ident.span());
        }
    }

    fn visit_attribute_mut(&mut self, attribute: &mut syn::Attribute) {
        // Attribute paths name external items, not local binders, but a
        // `#[attr = expr]` value may reference the local bindings.
        if let syn::Meta::NameValue(name_value) = &mut attribute.meta {
            syn::visit_mut::visit_expr_mut(self, &mut name_value.value);
        }
    }
}
