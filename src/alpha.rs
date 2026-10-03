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
use quote::ToTokens as _;
use syn::parse::discouraged::Speculative as _;
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
    /// Loop and block labels, a namespace rustc keeps distinct from lifetimes.
    Label,
    /// Local macro names.
    Macro,
}

/// A scope frame, one per block, function, generics list, impl, closure
/// or `for<'a>` binder, with one map per namespace.
#[derive(Default)]
struct Frame([BTreeMap<String, String>; 5]);

impl Frame {
    fn bind(&mut self, ns: Ns, name: String, canon: String) {
        // A fieldless enum's discriminant, 0 to 4.
        self.0[ns as usize].insert(name, canon);
    }

    fn lookup(&self, ns: Ns, name: &str) -> Option<&String> {
        // A fieldless enum's discriminant, 0 to 4.
        self.0[ns as usize].get(name)
    }

    /// First binder of `name` in `ns`, in priority order.
    fn lookup_any(&self, ns: &[Ns], name: &str) -> Option<&String> {
        ns.iter().find_map(|ns| self.lookup(*ns, name))
    }
}

/// Point `ident` at `canon`, keeping its span.
fn rename(ident: &mut Ident, canon: &str) {
    *ident = Ident::new(canon, ident.span());
}

/// The scope stack, the positional counter, the saved module frames,
/// and the self type of the enclosing impl.
struct Renamer {
    frames: Vec<Frame>,
    counter: usize,
    mod_frames: BTreeMap<String, Frame>,
    self_ty: Option<syn::Type>,
    in_type: bool,
}

impl Renamer {
    fn new() -> Self {
        Self {
            frames: vec![Frame::default()],
            counter: 0,
            mod_frames: BTreeMap::new(),
            self_ty: None,
            in_type: false,
        }
    }

    fn push(&mut self) {
        self.frames.push(Frame::default());
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

    /// Bind `ident` in the current frame and rename it.
    fn bind_ident(&mut self, ns: Ns, ident: &mut Ident) -> String {
        let canon = self.bind(ns, &ident.to_string());
        rename(ident, &canon);
        canon
    }

    /// Bind `name` in the value and type namespaces to one canon. A use
    /// alias or a const parameter prints once, two canons would leave a
    /// position of the other namespace pointing at an undeclared binder.
    fn bind_value_and_type(&mut self, name: &str) -> String {
        let canon = self.bind(Ns::Value, name);
        self.top_bind(Ns::Type, name, canon.clone());
        canon
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

    /// Rename `ident` to the canon it already holds in the current frame,
    /// else to a new one.
    fn rename_binder(&mut self, ns: Ns, ident: &mut Ident) -> String {
        let name = ident.to_string();
        let canon = match self.frames.last().and_then(|frame| frame.lookup(ns, &name)) {
            Some(canon) => canon.clone(),
            None => self.bind(ns, &name),
        };
        rename(ident, &canon);
        canon
    }

    /// Rename `ident` to its nearest binder in `ns`, if any.
    fn resolve(&self, ns: &[Ns], ident: &mut Ident) {
        if let Some(canon) = self.lookup(ns, &ident.to_string()) {
            rename(ident, &canon);
        }
    }

    fn visit_attrs(&mut self, attrs: &mut [syn::Attribute]) {
        for attr in attrs {
            self.visit_attribute_mut(attr);
        }
    }

    /// Bind a loop or block label in the current frame.
    fn bind_label(&mut self, label: Option<&mut syn::Label>) {
        if let Some(label) = label {
            self.bind_ident(Ns::Label, &mut label.name.ident);
        }
    }

    /// Resolve a label reference through the label namespace only, the
    /// lifetime map must not answer, rustc keeps the two namespaces apart.
    fn rename_label(&self, lifetime: &mut syn::Lifetime) {
        self.resolve(&[Ns::Label], &mut lifetime.ident);
    }

    /// Drop a leading `::` when no local item binds the first segment `first`
    /// of its path, `::std` and `std` then naming the same crate.
    fn unroot(&self, leading_colon: &mut Option<syn::Token![::]>, first: Option<&Ident>) {
        if first.is_some_and(|first| self.lookup(&[Ns::Type], &first.to_string()).is_none()) {
            *leading_colon = None;
        }
    }

    /// Rewrite macro tokens the parser cannot read, an identifier before
    /// `!` resolving through the macro namespace too.
    fn rename_tokens(&mut self, tokens: proc_macro2::TokenStream) -> proc_macro2::TokenStream {
        let mut out = Vec::new();
        let mut iter = tokens.into_iter().peekable();
        while let Some(tree) = iter.next() {
            out.push(match tree {
                proc_macro2::TokenTree::Ident(mut ident) => {
                    let bang = matches!(iter.peek(), Some(proc_macro2::TokenTree::Punct(p)) if p.as_char() == '!');
                    let ns: &[Ns] = if bang {
                        &[Ns::Value, Ns::Type, Ns::Macro]
                    } else {
                        &[Ns::Value, Ns::Type]
                    };
                    self.resolve(ns, &mut ident);
                    proc_macro2::TokenTree::Ident(ident)
                }
                proc_macro2::TokenTree::Group(group) => {
                    crate::drift::map_group(&group, |inner| self.rename_tokens(inner))
                }
                other => other,
            });
        }
        proc_macro2::TokenStream::from_iter(out)
    }

    /// Rewrite macro arguments as expressions when they parse, so binders
    /// inside them are visited, else token by token. The literal at
    /// `format_at` is a format string whose inline arguments rename too.
    fn rewrite_macro_tokens(
        &mut self,
        tokens: proc_macro2::TokenStream,
        format_at: Option<usize>,
    ) -> proc_macro2::TokenStream {
        let Ok(mut args) = syn::parse2::<MacroArgs>(tokens.clone()) else {
            return self.rename_tokens(tokens);
        };
        for (index, expr) in args.iter_mut().enumerate() {
            self.visit_expr_mut(expr);
            if Some(index) == format_at
                && let syn::Expr::Lit(syn::ExprLit {
                    lit: syn::Lit::Str(lit),
                    ..
                }) = expr
            {
                *lit = rewrite_format_str(self, lit);
            }
        }
        args.to_token_stream()
    }
}

/// Macro arguments as a comma list or the `elem; count` of `vec!`.
enum MacroArgs {
    List(syn::punctuated::Punctuated<syn::Expr, syn::Token![,]>),
    Repeat(syn::punctuated::Punctuated<syn::Expr, syn::Token![;]>),
}

impl MacroArgs {
    fn iter_mut(&mut self) -> syn::punctuated::IterMut<'_, syn::Expr> {
        match self {
            Self::List(exprs) => exprs.iter_mut(),
            Self::Repeat(exprs) => exprs.iter_mut(),
        }
    }
}

impl syn::parse::Parse for MacroArgs {
    fn parse(input: syn::parse::ParseStream) -> syn::Result<Self> {
        let list = input.fork();
        if let Ok(exprs) = list.call(syn::punctuated::Punctuated::parse_terminated) {
            input.advance_to(&list);
            return Ok(Self::List(exprs));
        }
        let span = input.span();
        let exprs: syn::punctuated::Punctuated<syn::Expr, syn::Token![;]> =
            input.call(syn::punctuated::Punctuated::parse_terminated)?;
        if exprs.len() == 2 && !exprs.trailing_punct() {
            Ok(Self::Repeat(exprs))
        } else {
            Err(syn::Error::new(span, "not a repeat"))
        }
    }
}

impl quote::ToTokens for MacroArgs {
    fn to_tokens(&self, tokens: &mut proc_macro2::TokenStream) {
        match self {
            Self::List(exprs) => exprs.to_tokens(tokens),
            Self::Repeat(exprs) => exprs.to_tokens(tokens),
        }
    }
}

/// `name` as its bound canon, else as written.
fn push_format_name(renamer: &Renamer, name: &str, out: &mut String) {
    out.push_str(
        renamer
            .lookup(&[Ns::Value], name)
            .as_deref()
            .unwrap_or(name),
    );
}

/// Rewrite one `{…}` placeholder body, its name and any `name$` width or
/// precision in the spec.
fn rewrite_format_arg(renamer: &Renamer, arg: &str, out: &mut String) {
    let (name, spec) = arg
        .split_once(':')
        .map_or((arg, None), |(n, s)| (n, Some(s)));
    push_format_name(renamer, name, out);
    let Some(spec) = spec else {
        return;
    };
    out.push(':');
    let mut rest = spec;
    while let Some((before, after)) = rest.split_once('$') {
        let start = before
            .char_indices()
            .rfind(|&(_, c)| !c.is_alphanumeric() && c != '_')
            .map_or(0, |(p, c)| p + c.len_utf8());
        out.push_str(&before[..start]);
        push_format_name(renamer, &before[start..], out);
        out.push('$');
        rest = after;
    }
    out.push_str(rest);
}

/// Rename inline `{name}` arguments of a format string, `{{` and `}}`
/// escapes untouched.
fn rewrite_format_str(renamer: &Renamer, lit: &syn::LitStr) -> syn::LitStr {
    let value = lit.value();
    let mut out = String::with_capacity(value.len());
    let mut chars = value.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '{' | '}' if chars.peek() == Some(&c) => {
                chars.next();
                out.push(c);
                out.push(c);
            }
            '{' => {
                let mut inner = String::new();
                let mut closed = false;
                for c in chars.by_ref() {
                    if c == '}' {
                        closed = true;
                        break;
                    }
                    inner.push(c);
                }
                out.push('{');
                rewrite_format_arg(renamer, &inner, &mut out);
                if closed {
                    out.push('}');
                }
            }
            other => out.push(other),
        }
    }
    syn::LitStr::new(&out, lit.span())
}

/// Alpha-normalize a parsed file in place.
pub(crate) fn normalize_file(file: &mut syn::File) {
    let mut renamer = Renamer::new();
    renamer.visit_file_mut(file);
}

/// True for an ident pattern that names a unit struct, variant, or
/// const rather than a binding, by the uppercase naming convention.
fn is_unit_path(id: &syn::PatIdent) -> bool {
    id.by_ref.is_none()
        && id.mutability.is_none()
        && id.subpat.is_none()
        && id.ident.to_string().starts_with(|c: char| c.is_uppercase())
}

/// Call `f` on every identifier pattern of `pat`, in traversal order.
/// A shorthand `S { x }` binds the member name and drops the member when
/// rendered, so its field takes the explicit `x: x` form on the way.
fn walk_pat_idents(pat: &mut syn::Pat, f: &mut impl FnMut(&mut syn::PatIdent)) {
    match pat {
        syn::Pat::Ident(id) => {
            f(id);
            if let Some((_, sub)) = &mut id.subpat {
                walk_pat_idents(sub, f);
            }
        }
        syn::Pat::Tuple(syn::PatTuple { elems, .. })
        | syn::Pat::Slice(syn::PatSlice { elems, .. })
        | syn::Pat::TupleStruct(syn::PatTupleStruct { elems, .. }) => {
            for pat in elems {
                walk_pat_idents(pat, f);
            }
        }
        syn::Pat::Struct(r#struct) => {
            for field in &mut r#struct.fields {
                field.colon_token.get_or_insert_with(Default::default);
                walk_pat_idents(&mut field.pat, f);
            }
        }
        syn::Pat::Or(r#or) => {
            for pat in &mut r#or.cases {
                walk_pat_idents(pat, f);
            }
        }
        syn::Pat::Reference(syn::PatReference { pat, .. })
        | syn::Pat::Type(syn::PatType { pat, .. })
        | syn::Pat::Guard(syn::PatGuard { pat, .. }) => walk_pat_idents(pat, f),
        _ => {}
    }
}

/// Rewrite the binder identifiers of `pat` to their canonical names.
fn rewrite_pat_bindings(renamer: &Renamer, pat: &mut syn::Pat) {
    walk_pat_idents(pat, &mut |id| {
        let ns: &[Ns] = if is_unit_path(id) {
            &[Ns::Value, Ns::Type]
        } else {
            &[Ns::Value]
        };
        renamer.resolve(ns, &mut id.ident);
    });
}

/// Push the names bound by `pat` to `out`, in traversal order.
fn pattern_names(pat: &mut syn::Pat, out: &mut Vec<String>) {
    walk_pat_idents(pat, &mut |id| {
        if !is_unit_path(id) {
            out.push(id.ident.to_string());
        }
    });
}

/// The names bound by the function parameters of `inputs`.
fn fn_param_names<'a>(inputs: impl Iterator<Item = &'a mut syn::FnArg>) -> Vec<String> {
    let mut out = Vec::new();
    for input in inputs {
        if let syn::FnArg::Typed(pat_type) = input {
            pattern_names(&mut pat_type.pat, &mut out);
        }
    }
    out
}

/// Pre-bind item names so a use may precede its definition, skipping
/// `macro_rules!` and `use`, which are visible only after their line.
/// A module's items go into its own frame, saved under its canon.
fn prebind<'a>(renamer: &mut Renamer, items: impl Iterator<Item = &'a syn::Item>) {
    for item in items {
        let (ns, ident) = match item {
            syn::Item::Fn(i) => (Ns::Value, &i.sig.ident),
            syn::Item::Const(i) => (Ns::Value, &i.ident),
            syn::Item::Static(i) => (Ns::Value, &i.ident),
            syn::Item::Struct(i) => (Ns::Type, &i.ident),
            syn::Item::Enum(i) => (Ns::Type, &i.ident),
            syn::Item::Union(i) => (Ns::Type, &i.ident),
            syn::Item::Type(i) => (Ns::Type, &i.ident),
            syn::Item::Trait(i) => (Ns::Type, &i.ident),
            syn::Item::TraitAlias(i) => (Ns::Type, &i.ident),
            syn::Item::Mod(i) => (Ns::Type, &i.ident),
            _ => continue,
        };
        let canon = renamer.bind(ns, &ident.to_string());
        if let syn::Item::Mod(syn::ItemMod {
            content: Some((_, items)),
            ..
        }) = item
        {
            renamer.push();
            prebind(renamer, items.iter());
            let frame = renamer.frames.pop().expect("a scope frame");
            renamer.mod_frames.insert(canon, frame);
        }
    }
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
                let canon = self.bind_value_and_type(&ident.to_string());
                syn::UseTree::Rename(syn::UseRename {
                    rename: Ident::new(&canon, ident.span()),
                    ident,
                    as_token: <syn::Token![as]>::default(),
                })
            }
            syn::UseTree::Rename(mut alias) => {
                let canon = self.bind_value_and_type(&alias.rename.to_string());
                rename(&mut alias.rename, &canon);
                syn::UseTree::Rename(alias)
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

    /// Bind every name `pat` binds, then visit and rewrite it.
    fn bind_pat(&mut self, pat: &mut syn::Pat) {
        let mut names = Vec::new();
        pattern_names(pat, &mut names);
        for name in &names {
            self.bind(Ns::Value, name);
        }
        syn::visit_mut::visit_pat_mut(self, pat);
        rewrite_pat_bindings(self, pat);
    }

    /// A function's generics frame and parameter frame around its
    /// signature and body, its own name already handled.
    fn visit_fn(
        &mut self,
        attrs: &mut [syn::Attribute],
        sig: &mut syn::Signature,
        body: Option<&mut syn::Block>,
    ) {
        begin_generics(self, &mut sig.generics);
        self.visit_attrs(attrs);
        self.push();
        for param in fn_param_names(sig.inputs.iter_mut()) {
            self.bind(Ns::Value, &param);
        }
        visit_fn_inputs(self, sig.inputs.iter_mut());
        syn::visit_mut::visit_return_type_mut(self, &mut sig.output);
        if let Some(body) = body {
            self.visit_block_mut(body);
        }
        self.pop();
        self.pop();
    }

    /// Rename a type item, then open its generics frame and visit its
    /// attributes. The caller visits the rest and pops the frame.
    fn begin_type_item(
        &mut self,
        ident: &mut Ident,
        generics: &mut syn::Generics,
        attrs: &mut [syn::Attribute],
    ) {
        self.rename_binder(Ns::Type, ident);
        begin_generics(self, generics);
        self.visit_attrs(attrs);
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
    if matches!(
        segment.arguments,
        syn::PathArguments::None | syn::PathArguments::AngleBracketed(_)
    ) {
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
                renamer.bind_ident(Ns::Lifetime, &mut lifetime.lifetime.ident);
            }
            syn::GenericParam::Type(ty_param) => {
                renamer.bind_ident(Ns::Type, &mut ty_param.ident);
            }
            syn::GenericParam::Const(r#const) => {
                // A bare reference in `Foo<N>` reads as a type argument.
                let canon = renamer.bind_value_and_type(&r#const.ident.to_string());
                rename(&mut r#const.ident, &canon);
            }
        }
    }
    for param in &mut generics.params {
        syn::visit_mut::visit_generic_param_mut(renamer, param);
    }
    if let Some(where_clause) = &mut generics.where_clause {
        syn::visit_mut::visit_where_clause_mut(renamer, where_clause);
    }
}

/// Walk a condition, binding each `let` pattern before the operands to
/// its right. Only `&&` may carry a `let`, so every binary splits.
fn walk_let_cond(renamer: &mut Renamer, cond: &mut syn::Expr) {
    match cond {
        syn::Expr::Binary(binary) => {
            walk_let_cond(renamer, &mut binary.left);
            walk_let_cond(renamer, &mut binary.right);
        }
        syn::Expr::Let(let_expr) => bind_let(renamer, let_expr),
        other => syn::visit_mut::visit_expr_mut(renamer, other),
    }
}

/// Visit a `let` expression, binding its pattern names before the pattern
/// is rewritten.
fn bind_let(renamer: &mut Renamer, let_expr: &mut syn::ExprLet) {
    syn::visit_mut::visit_expr_mut(renamer, &mut let_expr.expr);
    renamer.bind_pat(&mut let_expr.pat);
}

impl VisitMut for Renamer {
    fn visit_file_mut(&mut self, file: &mut syn::File) {
        prebind(self, file.items.iter());
        syn::visit_mut::visit_file_mut(self, file);
    }

    fn visit_block_mut(&mut self, block: &mut syn::Block) {
        self.push();
        prebind(
            self,
            block.stmts.iter().filter_map(|stmt| match stmt {
                syn::Stmt::Item(item) => Some(item),
                _ => None,
            }),
        );
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
        self.bind_pat(&mut local.pat);
    }

    fn visit_item_fn_mut(&mut self, item: &mut syn::ItemFn) {
        self.rename_binder(Ns::Value, &mut item.sig.ident);
        self.visit_fn(&mut item.attrs, &mut item.sig, Some(&mut item.block));
    }

    fn visit_impl_item_fn_mut(&mut self, item: &mut syn::ImplItemFn) {
        self.visit_fn(&mut item.attrs, &mut item.sig, Some(&mut item.block));
    }

    fn visit_trait_item_fn_mut(&mut self, item: &mut syn::TraitItemFn) {
        self.visit_fn(&mut item.attrs, &mut item.sig, item.default.as_mut());
    }

    fn visit_item_struct_mut(&mut self, item: &mut syn::ItemStruct) {
        self.begin_type_item(&mut item.ident, &mut item.generics, &mut item.attrs);
        syn::visit_mut::visit_fields_mut(self, &mut item.fields);
        self.pop();
    }

    fn visit_item_enum_mut(&mut self, item: &mut syn::ItemEnum) {
        self.begin_type_item(&mut item.ident, &mut item.generics, &mut item.attrs);
        for variant in &mut item.variants {
            syn::visit_mut::visit_variant_mut(self, variant);
        }
        self.pop();
    }

    fn visit_item_union_mut(&mut self, item: &mut syn::ItemUnion) {
        self.begin_type_item(&mut item.ident, &mut item.generics, &mut item.attrs);
        syn::visit_mut::visit_fields_named_mut(self, &mut item.fields);
        self.pop();
    }

    fn visit_item_type_mut(&mut self, item: &mut syn::ItemType) {
        self.begin_type_item(&mut item.ident, &mut item.generics, &mut item.attrs);
        syn::visit_mut::visit_type_mut(self, &mut item.ty);
        self.pop();
    }

    fn visit_item_trait_mut(&mut self, item: &mut syn::ItemTrait) {
        let canon = self.rename_binder(Ns::Type, &mut item.ident);
        self.top_bind(Ns::Type, "Self", canon);
        begin_generics(self, &mut item.generics);
        self.visit_attrs(&mut item.attrs);
        for trait_item in &mut item.items {
            syn::visit_mut::visit_trait_item_mut(self, trait_item);
        }
        self.pop();
    }

    fn visit_item_trait_alias_mut(&mut self, item: &mut syn::ItemTraitAlias) {
        self.begin_type_item(&mut item.ident, &mut item.generics, &mut item.attrs);
        for bound in &mut item.bounds {
            syn::visit_mut::visit_type_param_bound_mut(self, bound);
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
        self.visit_attrs(&mut item.attrs);
        if let Some((path, _)) = &mut item.trait_ {
            // The trait path of `impl Trait for Type` is a type position
            // given as a bare path, no visit_type_path_mut wraps it.
            let outer = core::mem::replace(&mut self.in_type, true);
            self.visit_path_mut(path);
            self.in_type = outer;
        }
        syn::visit_mut::visit_type_mut(self, &mut item.self_ty);
        let outer = self.self_ty.replace(item.self_ty.as_ref().clone());
        for impl_item in &mut item.items {
            syn::visit_mut::visit_impl_item_mut(self, impl_item);
        }
        self.self_ty = outer;
        if has_generics {
            self.pop();
        }
        self.pop();
    }

    fn visit_item_const_mut(&mut self, item: &mut syn::ItemConst) {
        self.rename_binder(Ns::Value, &mut item.ident);
        self.visit_attrs(&mut item.attrs);
        syn::visit_mut::visit_type_mut(self, &mut item.ty);
        syn::visit_mut::visit_expr_mut(self, &mut item.expr);
    }

    fn visit_item_static_mut(&mut self, item: &mut syn::ItemStatic) {
        self.rename_binder(Ns::Value, &mut item.ident);
        self.visit_attrs(&mut item.attrs);
        syn::visit_mut::visit_type_mut(self, &mut item.ty);
        syn::visit_mut::visit_expr_mut(self, &mut item.expr);
    }

    fn visit_item_macro_mut(&mut self, item: &mut syn::ItemMacro) {
        // `macro_rules! n {}` carries the name in `ident` and a body of
        // macro pattern syntax. The legacy `macro n {}` form does not occur
        // in doctests. A call at item level is visited like any other.
        if let Some(ident) = item.ident.as_mut() {
            self.bind_ident(Ns::Macro, ident);
            self.visit_attrs(&mut item.attrs);
        } else {
            self.visit_attrs(&mut item.attrs);
            self.visit_macro_mut(&mut item.mac);
        }
    }

    fn visit_item_use_mut(&mut self, item: &mut syn::ItemUse) {
        self.visit_attrs(&mut item.attrs);
        let first = match &item.tree {
            syn::UseTree::Path(path) => Some(&path.ident),
            syn::UseTree::Name(name) => Some(&name.ident),
            syn::UseTree::Rename(rename) => Some(&rename.ident),
            syn::UseTree::Glob(_) | syn::UseTree::Group(_) => None,
        };
        self.unroot(&mut item.leading_colon, first);
        let glob = syn::UseTree::Glob(syn::UseGlob {
            star_token: <syn::Token![*]>::default(),
        });
        item.tree = self.bind_use_tree(core::mem::replace(&mut item.tree, glob));
    }

    fn visit_item_mod_mut(&mut self, item: &mut syn::ItemMod) {
        let canon = self.rename_binder(Ns::Type, &mut item.ident);
        self.visit_attrs(&mut item.attrs);
        if let Some((_, items)) = &mut item.content {
            let frame = self.mod_frames.remove(&canon).unwrap_or_default();
            self.frames.push(frame);
            for sub_item in items.iter_mut() {
                syn::visit_mut::visit_item_mut(self, sub_item);
            }
            let frame = self.frames.pop().expect("a scope frame");
            self.mod_frames.insert(canon, frame);
        }
    }

    fn visit_expr_closure_mut(&mut self, closure: &mut syn::ExprClosure) {
        self.push();
        if let Some(bound) = &mut closure.lifetimes {
            self.visit_bound_lifetimes_mut(bound);
        }
        let mut names = Vec::new();
        for input in &mut closure.inputs {
            pattern_names(input, &mut names);
        }
        for name in &names {
            self.bind(Ns::Value, name);
        }
        for input in &mut closure.inputs {
            syn::visit_mut::visit_pat_mut(self, input);
            rewrite_pat_bindings(self, input);
        }
        syn::visit_mut::visit_return_type_mut(self, &mut closure.output);
        syn::visit_mut::visit_expr_mut(self, &mut closure.body);
        self.pop();
    }

    fn visit_arm_mut(&mut self, arm: &mut syn::Arm) {
        self.push();
        self.bind_pat(&mut arm.pat);
        syn::visit_mut::visit_expr_mut(self, &mut arm.body);
        self.pop();
    }

    fn visit_expr_for_loop_mut(&mut self, for_loop: &mut syn::ExprForLoop) {
        // The iterable resolves under the outer bindings.
        syn::visit_mut::visit_expr_mut(self, &mut for_loop.expr);
        self.push();
        self.bind_label(for_loop.label.as_mut());
        self.bind_pat(&mut for_loop.pat);
        self.visit_block_mut(&mut for_loop.body);
        self.pop();
    }

    fn visit_expr_if_mut(&mut self, expr: &mut syn::ExprIf) {
        // Condition bindings are visible in the then branch only.
        self.push();
        walk_let_cond(self, &mut expr.cond);
        self.visit_block_mut(&mut expr.then_branch);
        self.pop();
        if let Some((_, else_expr)) = &mut expr.else_branch {
            syn::visit_mut::visit_expr_mut(self, else_expr);
        }
    }

    fn visit_expr_while_mut(&mut self, expr: &mut syn::ExprWhile) {
        self.push();
        self.bind_label(expr.label.as_mut());
        walk_let_cond(self, &mut expr.cond);
        self.visit_block_mut(&mut expr.body);
        self.pop();
    }

    fn visit_expr_loop_mut(&mut self, expr: &mut syn::ExprLoop) {
        self.push();
        self.bind_label(expr.label.as_mut());
        self.visit_block_mut(&mut expr.body);
        self.pop();
    }

    fn visit_expr_block_mut(&mut self, expr: &mut syn::ExprBlock) {
        self.push();
        self.bind_label(expr.label.as_mut());
        self.visit_block_mut(&mut expr.block);
        self.pop();
    }

    fn visit_expr_struct_mut(&mut self, expr: &mut syn::ExprStruct) {
        // A shorthand `S { x }` prints only the member, so a bound `x`
        // takes the explicit form before the rewrite.
        for field in &mut expr.fields {
            if field.colon_token.is_none()
                && let syn::Member::Named(ident) = &field.member
                && self.lookup(&[Ns::Value], &ident.to_string()).is_some()
            {
                field.colon_token = Some(syn::token::Colon::default());
            }
        }
        syn::visit_mut::visit_expr_struct_mut(self, expr);
    }

    fn visit_path_mut(&mut self, path: &mut syn::Path) {
        // Only the first segment may name a local binder, and a leading
        // colon always names an extern crate. Each later segment resolves
        // through the saved frame of the local module before it. A type
        // position reads the type namespace only, rustc-valid programs
        // bind every name they show there in it.
        self.unroot(
            &mut path.leading_colon,
            path.segments.first().map(|s| &s.ident),
        );
        let ns: &[Ns] = if self.in_type {
            &[Ns::Type]
        } else {
            &[Ns::Value, Ns::Type]
        };
        if path.leading_colon.is_none()
            && let Some(first) = path.segments.first_mut()
            && let Some(mut canon) = self.lookup(ns, &first.ident.to_string())
        {
            rename(&mut first.ident, &canon);
            for segment in path.segments.iter_mut().skip(1) {
                let Some(next) = self
                    .mod_frames
                    .get(&canon)
                    .and_then(|frame| frame.lookup_any(ns, &segment.ident.to_string()))
                else {
                    break;
                };
                canon = next.clone();
                rename(&mut segment.ident, &canon);
            }
        }
        for segment in &mut path.segments {
            match &mut segment.arguments {
                syn::PathArguments::AngleBracketed(args) => {
                    for arg in &mut args.args {
                        if matches!(arg, syn::GenericArgument::Const(_)) {
                            // A const argument expression resolves values
                            // again, the type context ends at its braces.
                            let outer = core::mem::replace(&mut self.in_type, false);
                            syn::visit_mut::visit_generic_argument_mut(self, arg);
                            self.in_type = outer;
                        } else {
                            syn::visit_mut::visit_generic_argument_mut(self, arg);
                        }
                    }
                }
                syn::PathArguments::Parenthesized(args) => {
                    for input in &mut args.inputs {
                        self.visit_named_arg_mut(input);
                    }
                    syn::visit_mut::visit_return_type_mut(self, &mut args.output);
                }
                syn::PathArguments::None => {}
            }
        }
    }

    fn visit_macro_mut(&mut self, mac: &mut syn::Macro) {
        self.unroot(
            &mut mac.path.leading_colon,
            mac.path.segments.first().map(|s| &s.ident),
        );
        if let Some(first) = mac.path.segments.first_mut() {
            self.resolve(&[Ns::Macro], &mut first.ident);
        }
        let format_at = crate::drift::format_operand(&mac.path).map(|(at, _)| at);
        mac.tokens = self.rewrite_macro_tokens(core::mem::take(&mut mac.tokens), format_at);
    }

    fn visit_type_path_mut(&mut self, node: &mut syn::TypePath) {
        // Type positions resolve through the type namespace, a local value
        // binder of the same name must not capture the annotation.
        let outer = core::mem::replace(&mut self.in_type, true);
        syn::visit_mut::visit_type_path_mut(self, node);
        self.in_type = outer;
    }

    fn visit_type_mut(&mut self, ty: &mut syn::Type) {
        if let Some(self_ty) = &self.self_ty
            && let syn::Type::Path(path) = &*ty
            && path.qself.is_none()
            && path.path.is_ident("Self")
        {
            *ty = self_ty.clone();
            return;
        }
        syn::visit_mut::visit_type_mut(self, ty);
    }

    fn visit_lifetime_mut(&mut self, lifetime: &mut syn::Lifetime) {
        self.resolve(&[Ns::Lifetime], &mut lifetime.ident);
    }

    fn visit_expr_break_mut(&mut self, node: &mut syn::ExprBreak) {
        self.visit_attrs(&mut node.attrs);
        if let Some(label) = &mut node.label {
            self.rename_label(label);
        }
        if let Some(expr) = &mut node.expr {
            syn::visit_mut::visit_expr_mut(self, expr);
        }
    }

    fn visit_expr_continue_mut(&mut self, node: &mut syn::ExprContinue) {
        self.visit_attrs(&mut node.attrs);
        if let Some(label) = &mut node.label {
            self.rename_label(label);
        }
    }
    fn visit_pat_guard_mut(&mut self, node: &mut syn::PatGuard) {
        syn::visit_mut::visit_pat_mut(self, &mut node.pat);
        walk_let_cond(self, &mut node.guard);
    }

    fn visit_bound_lifetimes_mut(&mut self, node: &mut syn::BoundLifetimes) {
        for param in &mut node.lifetimes {
            if let syn::GenericParam::Lifetime(lifetime) = param {
                self.bind_ident(Ns::Lifetime, &mut lifetime.lifetime.ident);
            }
        }
        syn::visit_mut::visit_bound_lifetimes_mut(self, node);
    }

    fn visit_type_fn_ptr_mut(&mut self, node: &mut syn::TypeFnPtr) {
        // A `for<'a>` binder lives in its own frame, an outer loop label
        // named `'a` must not collide with it.
        self.push();
        syn::visit_mut::visit_type_fn_ptr_mut(self, node);
        self.pop();
    }

    fn visit_trait_bound_mut(&mut self, node: &mut syn::TraitBound) {
        // Same owner scoping for `for<'a>` on `dyn` and `impl` bounds, and
        // the bound path is a type position.
        self.push();
        let outer = core::mem::replace(&mut self.in_type, true);
        syn::visit_mut::visit_trait_bound_mut(self, node);
        self.in_type = outer;
        self.pop();
    }

    fn visit_named_arg_mut(&mut self, node: &mut syn::NamedArg) {
        // Parameter names in a function pointer type bind nothing, rustc
        // reads `fn(a: u8)`, `fn(_: u8)` and `fn(u8)` as one signature.
        node.name = None;
        syn::visit_mut::visit_named_arg_mut(self, node);
    }

    fn visit_attribute_mut(&mut self, attribute: &mut syn::Attribute) {
        // Attribute paths name external items, not local binders, but a
        // `#[attr = expr]` value may reference the local bindings.
        if let syn::Meta::NameValue(name_value) = &mut attribute.meta {
            syn::visit_mut::visit_expr_mut(self, &mut name_value.value);
        }
    }
}
