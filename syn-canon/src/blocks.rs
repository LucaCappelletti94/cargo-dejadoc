//! Borrowed discovery and independent normalization of lexical contexts.

use alloc::collections::BTreeMap;
use alloc::string::{String, ToString};
use alloc::vec;
use alloc::vec::Vec;

use proc_macro2::{Ident, Span, TokenStream, TokenTree};
use quote::ToTokens;
use syn::visit::Visit;
use syn::visit_mut::VisitMut;

use crate::alpha::{
    Bind, Domain, FirstSeg, Frame, Inherited, Ns, Renamer, Target, TargetSeg, is_unit_path,
    item_binding, resolve_target_path, single_segment_type_name,
};

/// The kind of lexical scope a context candidate covers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ContextKind {
    /// A function, method, or trait default body.
    FunctionBody,
    /// An explicit block, a loop body, or an `if` branch.
    Block,
    /// A complete match arm, pattern and guard included.
    Arm,
    /// A complete closure, inputs and body included.
    Closure,
}

/// The metadata of one emitted context candidate.
pub struct Context<'a> {
    /// The kind of lexical scope covered.
    pub kind: ContextKind,
    /// The candidate span from the supplied syntax tree, outer delimiters included.
    pub span: Span,
    /// The item containing the candidate, relative to the caller's module prefix.
    pub item: &'a str,
}

/// The measured work of a `contexts` walk.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ContextWork {
    /// The candidates emitted.
    pub contexts: usize,
    /// The raw leaf tokens the independent normalizations consumed, nested content repeated per containing candidate.
    pub input_tokens: usize,
}

/// Emit each original lexical context once, normalized against its visible declarations.
pub fn contexts(file: &syn::File, emit: &mut dyn FnMut(Context<'_>, TokenStream)) -> ContextWork {
    let mut walker = Walker {
        frames: vec![Frame::default()],
        mod_frames: BTreeMap::new(),
        targets: Vec::new(),
        item_path: Vec::new(),
        origin: 0,
        work: ContextWork::default(),
        emit,
    };
    walker.visit_file(file);
    walker.work
}

/// The original delimiter-inclusive span and raw leaf token count.
fn source_span_and_leaf_tokens(node: &impl ToTokens) -> (Span, usize) {
    let mut first = None;
    let mut last = None;
    let mut count = 0;
    for tree in node.to_token_stream() {
        if first.is_none() {
            first = Some(tree.span());
        }
        last = Some(tree.span());
        count += leaf_count(&tree);
    }
    (
        match (first, last) {
            (Some(first), Some(last)) => first.join(last).unwrap_or_else(Span::call_site),
            _ => Span::call_site(),
        },
        count,
    )
}

/// The leaf tokens of one tree, its inner groups counted.
fn leaf_count(tree: &TokenTree) -> usize {
    match tree {
        TokenTree::Group(group) => group
            .stream()
            .into_iter()
            .map(|inner| leaf_count(&inner))
            .sum(),
        _ => 1,
    }
}

/// Push the names bound by `pat` to `out`, in traversal order, without rewriting it.
fn pat_names(pat: &syn::Pat, out: &mut Vec<String>) {
    match pat {
        syn::Pat::Ident(id) => {
            if !is_unit_path(id) {
                out.push(id.ident.to_string());
            }
            if let Some((_, sub)) = &id.subpat {
                pat_names(sub, out);
            }
        }
        syn::Pat::Tuple(syn::PatTuple { elems, .. })
        | syn::Pat::Slice(syn::PatSlice { elems, .. })
        | syn::Pat::TupleStruct(syn::PatTupleStruct { elems, .. }) => {
            for pat in elems {
                pat_names(pat, out);
            }
        }
        syn::Pat::Struct(r#struct) => {
            for field in &r#struct.fields {
                pat_names(&field.pat, out);
            }
        }
        syn::Pat::Or(r#or) => {
            for pat in &r#or.cases {
                pat_names(pat, out);
            }
        }
        syn::Pat::Reference(syn::PatReference { pat, .. })
        | syn::Pat::Type(syn::PatType { pat, .. })
        | syn::Pat::Paren(syn::PatParen { pat, .. })
        | syn::Pat::Guard(syn::PatGuard { pat, .. }) => pat_names(pat, out),
        _ => {}
    }
}

/// The names bound by the function parameters of `inputs`.
fn fn_input_names<'a>(inputs: impl Iterator<Item = &'a syn::FnArg>) -> Vec<String> {
    let mut out = Vec::new();
    for input in inputs {
        if let syn::FnArg::Typed(pat_type) = input {
            pat_names(&pat_type.pat, &mut out);
        }
    }
    out
}

/// Prebind items and module frames so references may precede declarations.
fn prebind_items<'a>(
    frames: &mut Vec<Frame>,
    mod_frames: &mut BTreeMap<usize, Frame>,
    origin: &mut usize,
    items: impl Iterator<Item = &'a syn::Item>,
) {
    for item in items {
        let Some((ns, ident)) = item_binding(item) else {
            continue;
        };
        let o = *origin;
        *origin += 1;
        frames.last_mut().expect("a scope frame").bind(
            ns,
            ident.to_string(),
            Bind {
                origin: o,
                target: None,
                raw: false,
            },
        );
        if let syn::Item::Mod(syn::ItemMod {
            content: Some((_, sub_items)),
            ..
        }) = item
        {
            frames.push(Frame::default());
            prebind_items(frames, mod_frames, origin, sub_items.iter());
            let frame = frames.pop().expect("a scope frame");
            mod_frames.insert(o, frame);
        }
    }
}

/// The written self type of an impl, as the item path's owner.
fn self_type_string(self_ty: &syn::Type) -> String {
    self_ty.to_token_stream().to_string()
}

/// The borrowed walk that records candidates and the lexical environment around them.
struct Walker<'w> {
    /// The scope stack, the file frame first.
    frames: Vec<Frame>,
    /// The item frames of local modules, by module origin.
    mod_frames: BTreeMap<usize, Frame>,
    /// The interned alias targets.
    targets: Vec<Target>,
    /// The item path segments, outermost first.
    item_path: Vec<String>,
    /// The next origin identity.
    origin: usize,
    /// The measured work.
    work: ContextWork,
    /// The candidate emitter.
    emit: &'w mut dyn FnMut(Context<'_>, TokenStream),
}

impl Walker<'_> {
    /// Bind `name` in the current frame and return its origin.
    fn bind(&mut self, ns: Ns, name: &str) -> usize {
        let origin = self.origin;
        self.origin += 1;
        self.top_bind(
            ns,
            name,
            Bind {
                origin,
                target: None,
                raw: false,
            },
        );
        origin
    }

    /// Bind `name` to `bind` in the current frame.
    fn top_bind(&mut self, ns: Ns, name: &str, bind: Bind) {
        self.frames
            .last_mut()
            .expect("a scope frame")
            .bind(ns, name.to_string(), bind);
    }

    /// Bind `name` in the value and type namespaces to one origin, a `use` alias form.
    fn bind_value_and_type(&mut self, name: &str, target: Option<usize>) -> usize {
        let origin = self.origin;
        self.origin += 1;
        let bind = Bind {
            origin,
            target,
            raw: false,
        };
        self.top_bind(Ns::Value, name, bind);
        self.top_bind(Ns::Type, name, bind);
        origin
    }

    /// Nearest-binding lookup, namespace priority first.
    fn lookup(&self, ns: &[Ns], name: &str) -> Option<Bind> {
        for ns in ns {
            for frame in self.frames.iter().rev() {
                if let Some(bind) = frame.lookup(*ns, name) {
                    return Some(*bind);
                }
            }
        }
        None
    }

    /// Reuse a prebound item's origin when its declaration is visited.
    fn bind_item_name(&mut self, ns: Ns, ident: &Ident) -> usize {
        let name = ident.to_string();
        match self.frames.last().and_then(|frame| frame.lookup(ns, &name)) {
            Some(bind) => bind.origin,
            None => self.bind(ns, &name),
        }
    }

    /// Bind one scope-level `use` item, block-wide.
    fn bind_use_item(&mut self, item: &syn::ItemUse) {
        self.bind_use_tree(item.leading_colon.is_some(), &[], &item.tree);
    }

    /// Bind the block-wide `use` leaves of a scope.
    fn bind_scope_uses<'a>(&mut self, uses: impl Iterator<Item = &'a syn::ItemUse>) {
        for use_item in uses {
            self.bind_use_item(use_item);
        }
    }

    /// Bind the leaf aliases of one `use` tree, resolving each written target.
    fn bind_use_tree(&mut self, rooted: bool, prefix: &[String], tree: &syn::UseTree) {
        match tree {
            syn::UseTree::Path(path) => {
                let mut prefix = prefix.to_vec();
                prefix.push(path.ident.to_string());
                self.bind_use_tree(rooted, &prefix, &path.tree);
            }
            syn::UseTree::Name(name) if name.ident == "self" => {
                // `a::{self}` imports the module itself under its own name.
                if let Some(alias) = prefix.last() {
                    let (rooted, segments) =
                        resolve_target_path(&|name| self.resolve_first_seg(name), rooted, prefix);
                    self.bind_alias_target(rooted, segments, alias);
                }
            }
            syn::UseTree::Name(name) => {
                self.bind_alias_leaf(rooted, prefix, &name.ident, &name.ident);
            }
            syn::UseTree::Rename(rename) => {
                self.bind_alias_leaf(rooted, prefix, &rename.ident, &rename.rename);
            }
            syn::UseTree::Glob(_) => {}
            syn::UseTree::Group(group) => {
                for tree in &group.items {
                    self.bind_use_tree(rooted, prefix, tree);
                }
            }
        }
    }

    /// Bind one written target under `alias`.
    fn bind_alias_target(&mut self, rooted: bool, segments: Vec<TargetSeg>, alias: &str) {
        let index = self.targets.len();
        self.targets.push(Target { rooted, segments });
        self.bind_value_and_type(alias, Some(index));
    }

    /// Bind one `use` leaf that imports `ident` under `alias`.
    fn bind_alias_leaf(&mut self, rooted: bool, prefix: &[String], ident: &Ident, alias: &Ident) {
        let mut path = prefix.to_vec();
        path.push(ident.to_string());
        let (rooted, segments) =
            resolve_target_path(&|name| self.resolve_first_seg(name), rooted, &path);
        let index = self.targets.len();
        self.targets.push(Target { rooted, segments });
        self.bind_value_and_type(&alias.to_string(), Some(index));
    }

    /// Resolve the first-segment question of a written `use` prefix.
    fn resolve_first_seg(&self, name: &str) -> FirstSeg<'_> {
        match self.lookup(&[Ns::Type], name) {
            Some(bind) if bind.target.is_some() => {
                let index = bind.target.expect("a target index");
                FirstSeg::Alias(self.targets.get(index).expect("a target index"))
            }
            Some(bind) => FirstSeg::Module(Domain::Inherited, bind.origin),
            None => FirstSeg::Free,
        }
    }

    /// Prebind generic names without repeating the caller's syntax walk.
    fn begin_generics(&mut self, generics: &syn::Generics) {
        self.frames.push(Frame::default());
        for param in &generics.params {
            match param {
                syn::GenericParam::Lifetime(lifetime) => {
                    self.bind(Ns::Lifetime, &lifetime.lifetime.ident.to_string());
                }
                syn::GenericParam::Type(ty_param) => {
                    self.bind(Ns::Type, &ty_param.ident.to_string());
                }
                syn::GenericParam::Const(r#const) => {
                    // A bare reference in `Foo<N>` reads as a type argument.
                    let name = r#const.ident.to_string();
                    let origin = self.bind(Ns::Value, &name);
                    self.top_bind(
                        Ns::Type,
                        &name,
                        Bind {
                            origin,
                            target: None,
                            raw: false,
                        },
                    );
                }
            }
        }
    }

    /// Walk a function's signature and body, emitting the body candidate.
    fn visit_fn(&mut self, sig: &syn::Signature, body: &syn::Block) {
        self.begin_generics(&sig.generics);
        self.frames.push(Frame::default());
        for name in fn_input_names(sig.inputs.iter()) {
            self.bind(Ns::Value, &name);
        }
        syn::visit::visit_signature(self, sig);
        let (span, input_tokens) = source_span_and_leaf_tokens(body);
        let inherited = Inherited {
            frames: &self.frames,
            mod_frames: &self.mod_frames,
            targets: &self.targets,
        };
        let form = build_function_body_form(inherited, body);
        self.emit_candidate(ContextKind::FunctionBody, span, input_tokens, form);
        self.visit_block(body);
        self.frames.pop();
        self.frames.pop();
    }

    /// Walk a condition, binding each `let` after its operand, `&&`-only.
    fn walk_let_cond(&mut self, cond: &syn::Expr) {
        match cond {
            syn::Expr::Binary(binary) => {
                self.walk_let_cond(&binary.left);
                self.walk_let_cond(&binary.right);
            }
            syn::Expr::Let(let_expr) => {
                syn::visit::visit_expr(self, &let_expr.expr);
                let mut names = Vec::new();
                pat_names(&let_expr.pat, &mut names);
                for name in &names {
                    self.bind(Ns::Value, name);
                }
            }
            other => syn::visit::visit_expr(self, other),
        }
    }

    /// Emit one block-slot candidate, a loop body or an `if` branch.
    fn emit_block_slot(&mut self, block: &syn::Block) {
        let (span, input_tokens) = source_span_and_leaf_tokens(block);
        let inherited = Inherited {
            frames: &self.frames,
            mod_frames: &self.mod_frames,
            targets: &self.targets,
        };
        let form = build_block_slot_form(inherited, block);
        self.emit_candidate(ContextKind::Block, span, input_tokens, form);
    }

    /// Emit one block-expression candidate, the label and block included.
    fn emit_block_expr(&mut self, expr: syn::Expr) {
        let (span, input_tokens) = source_span_and_leaf_tokens(&expr);
        let inherited = Inherited {
            frames: &self.frames,
            mod_frames: &self.mod_frames,
            targets: &self.targets,
        };
        let form = build_block_expr_form(inherited, expr);
        self.emit_candidate(ContextKind::Block, span, input_tokens, form);
    }

    /// Emit one candidate with its measured input work.
    fn emit_candidate(
        &mut self,
        kind: ContextKind,
        span: Span,
        input_tokens: usize,
        form: TokenStream,
    ) {
        let item = self.item_path.join("::");
        (self.emit)(
            Context {
                kind,
                span,
                item: item.as_str(),
            },
            form,
        );
        self.work.contexts += 1;
        self.work.input_tokens += input_tokens;
    }

    /// Push one item path segment.
    fn push_item_path(&mut self, ident: &Ident) {
        self.item_path.push(ident.to_string());
    }

    /// Push one written item path segment.
    fn push_item_path_str(&mut self, name: String) {
        self.item_path.push(name);
    }

    /// Pop one item path segment.
    fn pop_item_path(&mut self) {
        self.item_path.pop();
    }
}

impl Visit<'_> for Walker<'_> {
    fn visit_file(&mut self, file: &syn::File) {
        prebind_items(
            &mut self.frames,
            &mut self.mod_frames,
            &mut self.origin,
            file.items.iter(),
        );
        self.bind_scope_uses(file.items.iter().filter_map(|item| match item {
            syn::Item::Use(u) => Some(u),
            _ => None,
        }));
        syn::visit::visit_file(self, file);
    }

    fn visit_block(&mut self, block: &syn::Block) {
        self.frames.push(Frame::default());
        prebind_items(
            &mut self.frames,
            &mut self.mod_frames,
            &mut self.origin,
            block.stmts.iter().filter_map(|stmt| match stmt {
                syn::Stmt::Item(item) => Some(item),
                _ => None,
            }),
        );
        self.bind_scope_uses(block.stmts.iter().filter_map(|stmt| match stmt {
            syn::Stmt::Item(syn::Item::Use(u)) => Some(u),
            _ => None,
        }));
        syn::visit::visit_block(self, block);
        self.frames.pop();
    }

    fn visit_local(&mut self, local: &syn::Local) {
        // The initializer resolves under the outer bindings.
        if let Some(init) = &local.init {
            syn::visit::visit_expr(self, &init.expr);
            if let Some(diverge) = &init.diverge {
                syn::visit::visit_expr(self, &diverge.1);
            }
        }
        let mut names = Vec::new();
        pat_names(&local.pat, &mut names);
        for name in &names {
            self.bind(Ns::Value, name);
        }
        syn::visit::visit_pat(self, &local.pat);
    }

    // Imports are bound by their enclosing scope.
    fn visit_item_use(&mut self, _: &syn::ItemUse) {}

    fn visit_item_fn(&mut self, item: &syn::ItemFn) {
        self.push_item_path(&item.sig.ident);
        self.visit_fn(&item.sig, &item.block);
        self.pop_item_path();
    }

    fn visit_item_mod(&mut self, item: &syn::ItemMod) {
        self.push_item_path(&item.ident);
        if let Some((_, items)) = &item.content {
            let origin = self
                .lookup(&[Ns::Type], &item.ident.to_string())
                .expect("a module is prebound")
                .origin;
            let frame = self.mod_frames.remove(&origin).unwrap_or_default();
            self.frames.push(frame);
            self.bind_scope_uses(items.iter().filter_map(|item| match item {
                syn::Item::Use(u) => Some(u),
                _ => None,
            }));
            for item in items {
                syn::visit::visit_item(self, item);
            }
            let frame = self.frames.pop().expect("a scope frame");
            self.mod_frames.insert(origin, frame);
        }
        self.pop_item_path();
    }

    fn visit_item_impl(&mut self, item: &syn::ItemImpl) {
        self.frames.push(Frame::default());
        if let Some(name) = single_segment_type_name(&item.self_ty)
            && let Some(bind) = self.lookup(&[Ns::Type], &name)
        {
            self.top_bind(Ns::Type, "Self", bind);
        }
        self.push_item_path_str(self_type_string(&item.self_ty));
        let has_generics = item.generics.lt_token.is_some();
        if has_generics {
            self.begin_generics(&item.generics);
        }
        syn::visit::visit_item_impl(self, item);
        if has_generics {
            self.frames.pop();
        }
        self.frames.pop();
        self.pop_item_path();
    }

    fn visit_item_trait(&mut self, item: &syn::ItemTrait) {
        self.push_item_path(&item.ident);
        let origin = self.bind_item_name(Ns::Type, &item.ident);
        self.begin_generics(&item.generics);
        self.top_bind(
            Ns::Type,
            "Self",
            Bind {
                origin,
                target: None,
                raw: false,
            },
        );
        syn::visit::visit_item_trait(self, item);
        self.frames.pop();
        self.pop_item_path();
    }

    fn visit_item_struct(&mut self, item: &syn::ItemStruct) {
        self.push_item_path(&item.ident);
        self.bind_item_name(Ns::Type, &item.ident);
        self.begin_generics(&item.generics);
        syn::visit::visit_item_struct(self, item);
        self.frames.pop();
        self.pop_item_path();
    }

    fn visit_item_enum(&mut self, item: &syn::ItemEnum) {
        self.push_item_path(&item.ident);
        self.bind_item_name(Ns::Type, &item.ident);
        self.begin_generics(&item.generics);
        syn::visit::visit_item_enum(self, item);
        self.frames.pop();
        self.pop_item_path();
    }

    fn visit_item_union(&mut self, item: &syn::ItemUnion) {
        self.push_item_path(&item.ident);
        self.bind_item_name(Ns::Type, &item.ident);
        self.begin_generics(&item.generics);
        syn::visit::visit_item_union(self, item);
        self.frames.pop();
        self.pop_item_path();
    }

    fn visit_item_type(&mut self, item: &syn::ItemType) {
        self.push_item_path(&item.ident);
        self.bind_item_name(Ns::Type, &item.ident);
        self.begin_generics(&item.generics);
        syn::visit::visit_item_type(self, item);
        self.frames.pop();
        self.pop_item_path();
    }

    fn visit_item_trait_alias(&mut self, item: &syn::ItemTraitAlias) {
        self.push_item_path(&item.ident);
        self.bind_item_name(Ns::Type, &item.ident);
        self.begin_generics(&item.generics);
        syn::visit::visit_item_trait_alias(self, item);
        self.frames.pop();
        self.pop_item_path();
    }

    fn visit_item_const(&mut self, item: &syn::ItemConst) {
        self.push_item_path(&item.ident);
        self.bind_item_name(Ns::Value, &item.ident);
        syn::visit::visit_item_const(self, item);
        self.pop_item_path();
    }

    fn visit_item_static(&mut self, item: &syn::ItemStatic) {
        self.push_item_path(&item.ident);
        self.bind_item_name(Ns::Value, &item.ident);
        syn::visit::visit_item_static(self, item);
        self.pop_item_path();
    }

    fn visit_item_macro(&mut self, item: &syn::ItemMacro) {
        // `macro_rules!` binds from its declaration onward.
        if let Some(ident) = &item.ident {
            self.bind_item_name(Ns::Macro, ident);
        }
        syn::visit::visit_item_macro(self, item);
    }

    fn visit_foreign_item_fn(&mut self, item: &syn::ForeignItemFn) {
        self.push_item_path(&item.sig.ident);
        self.begin_generics(&item.sig.generics);
        syn::visit::visit_foreign_item_fn(self, item);
        self.frames.pop();
        self.pop_item_path();
    }

    fn visit_impl_item(&mut self, item: &syn::ImplItem) {
        match item {
            syn::ImplItem::Fn(f) => {
                self.push_item_path(&f.sig.ident);
                self.visit_fn(&f.sig, &f.block);
                self.pop_item_path();
            }
            syn::ImplItem::Const(c) => {
                self.push_item_path(&c.ident);
                syn::visit::visit_impl_item_const(self, c);
                self.pop_item_path();
            }
            syn::ImplItem::Type(t) => {
                self.push_item_path(&t.ident);
                syn::visit::visit_impl_item_type(self, t);
                self.pop_item_path();
            }
            syn::ImplItem::Macro(m) => {
                syn::visit::visit_impl_item_macro(self, m);
            }
            syn::ImplItem::Verbatim(_) | _ => {}
        }
    }

    fn visit_trait_item(&mut self, item: &syn::TraitItem) {
        match item {
            syn::TraitItem::Fn(f) => {
                self.push_item_path(&f.sig.ident);
                if let Some(body) = &f.default {
                    self.visit_fn(&f.sig, body);
                } else {
                    self.begin_generics(&f.sig.generics);
                    syn::visit::visit_signature(self, &f.sig);
                    self.frames.pop();
                }
                self.pop_item_path();
            }
            syn::TraitItem::Const(c) => {
                self.push_item_path(&c.ident);
                syn::visit::visit_trait_item_const(self, c);
                self.pop_item_path();
            }
            syn::TraitItem::Type(t) => {
                self.push_item_path(&t.ident);
                syn::visit::visit_trait_item_type(self, t);
                self.pop_item_path();
            }
            syn::TraitItem::Macro(m) => {
                syn::visit::visit_trait_item_macro(self, m);
            }
            syn::TraitItem::Verbatim(_) | _ => {}
        }
    }

    fn visit_expr_closure(&mut self, closure: &syn::ExprClosure) {
        let (span, input_tokens) = source_span_and_leaf_tokens(closure);
        let inherited = Inherited {
            frames: &self.frames,
            mod_frames: &self.mod_frames,
            targets: &self.targets,
        };
        let form = build_closure_form(inherited, closure);
        self.emit_candidate(ContextKind::Closure, span, input_tokens, form);
        self.frames.push(Frame::default());
        if let Some(bound) = &closure.lifetimes {
            syn::visit::visit_bound_lifetimes(self, bound);
        }
        let mut names = Vec::new();
        for input in &closure.inputs {
            pat_names(input, &mut names);
        }
        for name in &names {
            self.bind(Ns::Value, name);
        }
        for input in &closure.inputs {
            syn::visit::visit_pat(self, input);
        }
        syn::visit::visit_return_type(self, &closure.output);
        syn::visit::visit_expr(self, &closure.body);
        self.frames.pop();
    }

    fn visit_expr_block(&mut self, expr: &syn::ExprBlock) {
        self.emit_block_expr(syn::Expr::Block(expr.clone()));
        if let Some(label) = &expr.label {
            self.bind(Ns::Label, &label.name.ident.to_string());
        }
        self.visit_block(&expr.block);
    }

    fn visit_expr_unsafe(&mut self, expr: &syn::ExprUnsafe) {
        self.emit_block_expr(syn::Expr::Unsafe(expr.clone()));
        syn::visit::visit_expr_unsafe(self, expr);
    }

    fn visit_expr_async(&mut self, expr: &syn::ExprAsync) {
        self.emit_block_expr(syn::Expr::Async(expr.clone()));
        syn::visit::visit_expr_async(self, expr);
    }

    fn visit_expr_try_block(&mut self, expr: &syn::ExprTryBlock) {
        self.emit_block_expr(syn::Expr::TryBlock(expr.clone()));
        syn::visit::visit_expr_try_block(self, expr);
    }

    fn visit_expr_const(&mut self, expr: &syn::ExprConst) {
        self.emit_block_expr(syn::Expr::Const(expr.clone()));
        syn::visit::visit_expr_const(self, expr);
    }

    fn visit_expr_if(&mut self, expr: &syn::ExprIf) {
        // Condition bindings are visible in the then branch only.
        self.frames.push(Frame::default());
        self.walk_let_cond(&expr.cond);
        self.emit_block_slot(&expr.then_branch);
        self.visit_block(&expr.then_branch);
        self.frames.pop();
        if let Some((_, otherwise)) = &expr.else_branch {
            syn::visit::visit_expr(self, otherwise);
        }
    }

    fn visit_expr_for_loop(&mut self, for_loop: &syn::ExprForLoop) {
        // The iterable resolves under the outer bindings.
        syn::visit::visit_expr(self, &for_loop.expr);
        self.frames.push(Frame::default());
        if let Some(label) = &for_loop.label {
            self.bind(Ns::Label, &label.name.ident.to_string());
        }
        let mut names = Vec::new();
        pat_names(&for_loop.pat, &mut names);
        for name in &names {
            self.bind(Ns::Value, name);
        }
        self.emit_block_slot(&for_loop.body);
        self.visit_block(&for_loop.body);
        self.frames.pop();
    }

    fn visit_expr_while(&mut self, expr: &syn::ExprWhile) {
        self.frames.push(Frame::default());
        if let Some(label) = &expr.label {
            self.bind(Ns::Label, &label.name.ident.to_string());
        }
        self.walk_let_cond(&expr.cond);
        self.emit_block_slot(&expr.body);
        self.visit_block(&expr.body);
        self.frames.pop();
    }

    fn visit_expr_loop(&mut self, expr: &syn::ExprLoop) {
        self.frames.push(Frame::default());
        if let Some(label) = &expr.label {
            self.bind(Ns::Label, &label.name.ident.to_string());
        }
        self.emit_block_slot(&expr.body);
        self.visit_block(&expr.body);
        self.frames.pop();
    }

    fn visit_arm(&mut self, arm: &syn::Arm) {
        let (span, input_tokens) = source_span_and_leaf_tokens(arm);
        let inherited = Inherited {
            frames: &self.frames,
            mod_frames: &self.mod_frames,
            targets: &self.targets,
        };
        let form = build_arm_form(inherited, arm);
        self.emit_candidate(ContextKind::Arm, span, input_tokens, form);
        self.frames.push(Frame::default());
        let mut names = Vec::new();
        pat_names(&arm.pat, &mut names);
        for name in &names {
            self.bind(Ns::Value, name);
        }
        syn::visit::visit_arm(self, arm);
        self.frames.pop();
    }

    fn visit_pat_guard(&mut self, node: &syn::PatGuard) {
        syn::visit::visit_pat(self, &node.pat);
        self.walk_let_cond(&node.guard);
    }

    fn visit_bound_lifetimes(&mut self, node: &syn::BoundLifetimes) {
        for param in &node.lifetimes {
            if let syn::GenericParam::Lifetime(lifetime) = param {
                self.bind(Ns::Lifetime, &lifetime.lifetime.ident.to_string());
            }
        }
        syn::visit::visit_bound_lifetimes(self, node);
    }

    fn visit_trait_bound(&mut self, node: &syn::TraitBound) {
        // Same owner scoping for `for<'a>` on `dyn` and `impl` bounds.
        self.frames.push(Frame::default());
        syn::visit::visit_trait_bound(self, node);
        self.frames.pop();
    }

    fn visit_type_fn_ptr(&mut self, node: &syn::TypeFnPtr) {
        // Higher-ranked lifetimes cannot capture outer loop labels.
        self.frames.push(Frame::default());
        syn::visit::visit_type_fn_ptr(self, node);
        self.frames.pop();
    }
}

/// Normalize one cloned function body against the inherited environment.
fn build_function_body_form(inherited: Inherited<'_>, body: &syn::Block) -> TokenStream {
    let mut block = body.clone();
    crate::drift::normalize_block(&mut block);
    crate::drift::fold_fn_body(&mut block);
    let mut renamer = Renamer::contextual(inherited);
    renamer.visit_block_mut(&mut block);
    fold_form(block)
}

/// Normalize one cloned block slot, a loop body or an `if` branch.
fn build_block_slot_form(inherited: Inherited<'_>, block: &syn::Block) -> TokenStream {
    let mut block = block.clone();
    crate::drift::normalize_block(&mut block);
    let mut renamer = Renamer::contextual(inherited);
    renamer.visit_block_mut(&mut block);
    fold_form(block)
}

/// Normalize one cloned block expression, the label and block included.
fn build_block_expr_form(inherited: Inherited<'_>, expr: syn::Expr) -> TokenStream {
    let mut expr = expr;
    crate::drift::normalize_expr(&mut expr);
    let mut renamer = Renamer::contextual(inherited);
    renamer.visit_expr_mut(&mut expr);
    fold_form(expr)
}

/// Normalize one cloned match arm.
fn build_arm_form(inherited: Inherited<'_>, arm: &syn::Arm) -> TokenStream {
    let mut arm = arm.clone();
    crate::drift::normalize_arm(&mut arm);
    let mut renamer = Renamer::contextual(inherited);
    renamer.visit_arm_mut(&mut arm);
    fold_form(arm)
}

/// Normalize one cloned closure, inputs and body included.
fn build_closure_form(inherited: Inherited<'_>, closure: &syn::ExprClosure) -> TokenStream {
    let mut expr = syn::Expr::Closure(closure.clone());
    crate::drift::normalize_expr(&mut expr);
    let mut renamer = Renamer::contextual(inherited);
    renamer.visit_expr_mut(&mut expr);
    fold_form(expr)
}

/// Fold the normalized candidate's tokens the way the file pipeline folds its own.
fn fold_form(node: impl ToTokens) -> TokenStream {
    crate::fold_tokens(node.to_token_stream(), false, false)
        .into_iter()
        .collect()
}
