//! Closed item graphs and compatible inherent partitions.

use alloc::collections::{BTreeMap, BTreeSet};
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec;
use alloc::vec::Vec;
use proc_macro2::Ident;
use quote::ToTokens;
use syn::visit::{self, Visit};
use syn::visit_mut::{self, VisitMut};

use crate::reference::{ReferenceFacts, Resolution, ResolvedOrigin};
use crate::schedule::Lbl;
use crate::scope::{Binding, BindingId, Domain, Frame, Ns, Origin};

fn identity(origin: ResolvedOrigin) -> (bool, BindingId) {
    (origin.domain == Domain::Inherited, origin.id)
}

fn namespace(ns: Ns) -> &'static str {
    match ns {
        Ns::Value => "v",
        Ns::Type => "t",
        Ns::Lifetime => "l",
        Ns::Label => "b",
        Ns::Macro => "m",
    }
}

#[derive(Default)]
struct Budget {
    nodes: usize,
    depth: usize,
    deepest: usize,
}

macro_rules! count_nodes {
    ($($method:ident: $ty:ident),* $(,)?) => {
        $(fn $method(&mut self, value: &'ast syn::$ty) {
            self.nodes += 1;
            self.depth += 1;
            self.deepest = self.deepest.max(self.depth);
            if self.nodes <= 16_384 && self.depth <= 64 {
                visit::$method(self, value);
            }
            self.depth -= 1;
        })*
    };
}

impl<'ast> Visit<'ast> for Budget {
    count_nodes! {
        visit_item: Item, visit_item_fn: ItemFn, visit_signature: Signature,
        visit_generics: Generics, visit_return_type: ReturnType,
        visit_type: Type, visit_type_path: TypePath, visit_path: Path,
        visit_path_segment: PathSegment, visit_block: Block, visit_stmt: Stmt,
        visit_expr: Expr, visit_expr_lit: ExprLit, visit_lit: Lit,
        visit_pat: Pat, visit_attribute: Attribute, visit_generic_argument: GenericArgument,
        visit_angle_bracketed_generic_arguments: AngleBracketedGenericArguments,
        visit_field: Field, visit_variant: Variant, visit_impl_item: ImplItem,
        visit_item_impl: ItemImpl, visit_item_struct: ItemStruct,
        visit_item_enum: ItemEnum, visit_item_type: ItemType, visit_item_const: ItemConst,
    }
    fn visit_ident(&mut self, _: &'ast Ident) {
        self.nodes += 1;
        self.deepest = self.deepest.max(self.depth + 1);
    }

    fn visit_macro(&mut self, mac: &'ast syn::Macro) {
        visit::visit_macro(self, mac);
        self.tokens(mac.tokens.clone(), self.depth + 1);
    }
}

impl Budget {
    fn tokens(&mut self, stream: proc_macro2::TokenStream, depth: usize) {
        for tree in stream {
            self.nodes += 1;
            self.deepest = self.deepest.max(depth);
            if self.nodes > 16_384 || depth > 64 {
                return;
            }
            if let proc_macro2::TokenTree::Group(group) = tree {
                self.tokens(group.stream(), depth + 1);
            }
        }
    }

    fn accepts(items: &[&syn::Item]) -> bool {
        let mut budget = Self::default();
        for item in items {
            budget.visit_item(item);
        }
        budget.nodes <= 16_384 && budget.deepest <= 64
    }

    fn within(&self) -> bool {
        self.nodes <= crate::schedule::MAX_REGION_NODES
            && self.deepest <= crate::schedule::MAX_BLOCK_DEPTH
    }

    fn label(&mut self, label: &Lbl, depth: usize) {
        if !self.within() {
            return;
        }
        self.nodes += 1;
        self.deepest = self.deepest.max(depth);
        if !self.within() {
            return;
        }
        match label {
            Lbl::Not(inner) => self.label(inner, depth + 1),
            Lbl::Op(_, left, right, _) => {
                self.label(left, depth + 1);
                self.label(right, depth + 1);
            }
            Lbl::Flat(_, children, _) | Lbl::Other(_, children) => {
                for child in children {
                    self.label(child, depth + 1);
                }
            }
            Lbl::Call(callee, arguments, _) => {
                self.label(callee, depth + 1);
                for argument in arguments {
                    self.label(argument, depth + 1);
                }
            }
            _ => {}
        }
    }
}

struct Check<'a> {
    facts: &'a ReferenceFacts,
    blocked: bool,
    public: bool,
}

impl Visit<'_> for Check<'_> {
    fn visit_attribute(&mut self, attr: &syn::Attribute) {
        let path = attr.path();
        self.blocked |= path.is_ident("cfg")
            || path.is_ident("cfg_attr")
            || path.is_ident("track_caller")
            || crate::context::live_attr(core::slice::from_ref(attr));
    }
    fn visit_use_glob(&mut self, _: &syn::UseGlob) {
        self.blocked = true;
    }
    fn visit_item_macro(&mut self, _: &syn::ItemMacro) {
        self.blocked = true;
    }
    fn visit_macro(&mut self, mac: &syn::Macro) {
        let Some(first) = mac.path.segments.first() else {
            self.blocked = true;
            return;
        };
        let key = core::ptr::from_ref(first);
        if !self.facts.known_macros.contains(&key)
            || syn::parse2::<crate::alpha::MacroArgs>(mac.tokens.clone()).is_err()
            || !self.facts.original_macros.contains_key(&key)
        {
            self.blocked = true;
        }
    }
    fn visit_path(&mut self, path: &syn::Path) {
        if let Some(first) = path.segments.first() {
            let key = core::ptr::from_ref(first);
            self.blocked |= self
                .facts
                .original_paths
                .get(&key)
                .is_none_or(|resolution| {
                    resolution.origin.is_none()
                        && resolution.prim.is_none()
                        && !self.facts.portable_paths.contains(&key)
                        && !self.facts.known_macros.contains(&key)
                });
        }
        visit::visit_path(self, path);
    }
    fn visit_item(&mut self, item: &syn::Item) {
        let visibility = match item {
            syn::Item::Fn(item) => Some(&item.vis),
            syn::Item::Const(item) => Some(&item.vis),
            syn::Item::Type(item) => Some(&item.vis),
            syn::Item::Struct(item) => Some(&item.vis),
            syn::Item::Enum(item) => Some(&item.vis),
            syn::Item::Union(item) => Some(&item.vis),
            _ => None,
        };
        self.public |= visibility.is_some_and(|vis| !matches!(vis, syn::Visibility::Inherited));
        visit::visit_item(self, item);
    }

    fn visit_visibility(&mut self, _: &syn::Visibility) {}
}

fn admitted(item: &syn::Item) -> bool {
    matches!(
        item,
        syn::Item::Fn(_)
            | syn::Item::Const(_)
            | syn::Item::Type(_)
            | syn::Item::Struct(_)
            | syn::Item::Enum(_)
            | syn::Item::Union(_)
            | syn::Item::Impl(syn::ItemImpl { trait_: None, .. })
    )
}

fn group_member(item: &syn::Item) -> bool {
    admitted(item) || matches!(item, syn::Item::Use(_))
}

/// The proof facts recorded for one path occurrence, in traversal order.
#[derive(Copy, Clone, Default)]
struct PathProof {
    resolution: Option<Resolution>,
    portable: bool,
    known: bool,
    helper: Option<crate::scope::HelperId>,
    scope: Option<crate::helpers::ScopeId>,
}

#[derive(Default)]
struct Snapshot {
    idents: Vec<Option<(ResolvedOrigin, bool)>>,
    macros: Vec<Vec<Resolution>>,
    paths: Vec<PathProof>,
    parameters: BTreeMap<(bool, BindingId), usize>,
    owner_routes: BTreeMap<(bool, BindingId), crate::reference::OwnerRoute>,
}

impl Snapshot {
    fn collect(item: &syn::Item, facts: &ReferenceFacts) -> Self {
        let mut collect = Selected::new(facts);
        collect.visit_item(item);
        collect.finish()
    }

    fn statement(stmt: &syn::Stmt, facts: &ReferenceFacts) -> Self {
        let mut collect = Selected::new(facts);
        collect.visit_stmt(stmt);
        collect.finish()
    }

    fn header(item: &syn::ItemImpl, facts: &ReferenceFacts) -> Self {
        let mut collect = Selected::new(facts);
        for attr in &item.attrs {
            collect.visit_attribute(attr);
        }
        collect.visit_generics(&item.generics);
        collect.visit_type(&item.self_ty);
        collect.finish()
    }

    fn member(item: &syn::ImplItem, facts: &ReferenceFacts) -> Self {
        let mut collect = Selected::new(facts);
        collect.visit_impl_item(item);
        collect.finish()
    }

    fn append(&mut self, other: Self) {
        self.idents.extend(other.idents);
        self.macros.extend(other.macros);
        self.paths.extend(other.paths);
        self.parameters.extend(other.parameters);
        self.owner_routes.extend(other.owner_routes);
    }

    fn attach_item(&self, item: &mut syn::Item, facts: &mut ReferenceFacts) {
        self.attacher(facts).visit_item_mut(item);
    }

    fn attach_stmt(&self, stmt: &mut syn::Stmt, facts: &mut ReferenceFacts) {
        self.attacher(facts).visit_stmt_mut(stmt);
    }

    fn attach_impl(&self, item: &mut syn::ItemImpl, facts: &mut ReferenceFacts) {
        self.attacher(facts).visit_item_impl_mut(item);
    }

    /// Record the proof's parameters and owner routes in `facts`, returning the visitor
    /// that reattaches its identifier, macro and path records.
    fn attacher<'a>(&'a self, facts: &'a mut ReferenceFacts) -> Attach<'a> {
        facts.original_parameters.extend(
            self.parameters
                .iter()
                .map(|(&origin, &port)| (origin, port)),
        );
        facts.owner_routes.extend(
            self.owner_routes
                .iter()
                .map(|(&origin, route)| (origin, route.clone())),
        );
        Attach {
            facts,
            records: self.idents.iter(),
            macros: self.macros.iter(),
            paths: self.paths.iter(),
        }
    }
}

struct Attach<'a> {
    facts: &'a mut ReferenceFacts,
    records: core::slice::Iter<'a, Option<(ResolvedOrigin, bool)>>,
    macros: core::slice::Iter<'a, Vec<Resolution>>,
    paths: core::slice::Iter<'a, PathProof>,
}

impl VisitMut for Attach<'_> {
    fn visit_ident_mut(&mut self, ident: &mut Ident) {
        let key = core::ptr::from_ref(ident);
        self.facts.idents.remove(&key);
        self.facts.declarations.remove(&key);
        if let Some(Some((origin, declaration))) = self.records.next() {
            self.facts.idents.insert(key, *origin);
            if *declaration {
                self.facts.declarations.insert(key);
            }
        }
    }

    fn visit_path_mut(&mut self, path: &mut syn::Path) {
        let proof = self.paths.next().copied();
        visit_mut::visit_path_mut(self, path);
        let Some(proof) = proof else {
            return;
        };
        let Some(first) = path.segments.first() else {
            return;
        };
        let key = core::ptr::from_ref(first);
        if let Some(resolution) = proof.resolution {
            self.facts.original_paths.insert(key, resolution);
        }
        if proof.portable {
            self.facts.portable_paths.insert(key);
        }
        if proof.known {
            self.facts.known_macros.insert(key);
        }
        if let Some(helper) = proof.helper {
            self.facts.original_calls.insert(key, helper);
        }
        if let Some(scope) = proof.scope {
            self.facts.original_call_scopes.insert(key, scope);
        }
    }

    fn visit_macro_mut(&mut self, mac: &mut syn::Macro) {
        let records = self.macros.next();
        visit_mut::visit_macro_mut(self, mac);
        if let Some(records) = records
            && let Some(first) = mac.path.segments.first()
        {
            self.facts
                .original_macros
                .insert(core::ptr::from_ref(first), records.clone());
        }
    }
}

struct Clear<'a>(&'a mut ReferenceFacts);

impl Visit<'_> for Clear<'_> {
    fn visit_ident(&mut self, ident: &Ident) {
        let key = core::ptr::from_ref(ident);
        self.0.idents.remove(&key);
        self.0.declarations.remove(&key);
    }
}

fn clone_header(item: &syn::ItemImpl) -> syn::ItemImpl {
    syn::ItemImpl {
        attrs: item.attrs.clone(),
        modifiers: item.modifiers.clone(),
        unsafety: item.unsafety,
        impl_token: item.impl_token,
        generics: item.generics.clone(),
        trait_: None,
        self_ty: item.self_ty.clone(),
        brace_token: item.brace_token,
        items: Vec::new(),
    }
}

struct OriginalItem {
    item: syn::Item,
    proof: Snapshot,
}

struct OriginalStmt {
    stmt: syn::Stmt,
    proof: Snapshot,
}

/// A schedulable region source, a top-level item or a block statement.
trait RegionSource {
    type Restored;
    fn item(&self) -> &syn::Item;
    fn visit(&mut self, normalize: &mut Normalize<'_>);
    fn reprove(&mut self, facts: &ReferenceFacts);
    fn clear(&self, facts: &mut ReferenceFacts);
    fn attach(&mut self, facts: &mut ReferenceFacts);
    fn admitted(&self) -> bool;
    fn group_member(&self) -> bool;
    fn observe(&self, observations: &mut Observations<'_>);
    fn into_original(self) -> Option<OriginalItem>;
    fn from_original(source: OriginalItem) -> Self;
    fn split(self) -> (Self::Restored, Snapshot);
    fn attach_restored(restored: &mut Self::Restored, proof: &Snapshot, facts: &mut ReferenceFacts);
}

impl RegionSource for OriginalItem {
    type Restored = syn::Item;
    fn item(&self) -> &syn::Item {
        &self.item
    }
    fn visit(&mut self, normalize: &mut Normalize<'_>) {
        normalize.visit_item_mut(&mut self.item);
    }
    fn reprove(&mut self, facts: &ReferenceFacts) {
        self.proof = Snapshot::collect(&self.item, facts);
    }
    fn clear(&self, facts: &mut ReferenceFacts) {
        Clear(facts).visit_item(&self.item);
    }
    fn attach(&mut self, facts: &mut ReferenceFacts) {
        self.proof.attach_item(&mut self.item, facts);
    }
    fn admitted(&self) -> bool {
        admitted(&self.item)
    }
    fn group_member(&self) -> bool {
        group_member(&self.item)
    }
    fn observe(&self, observations: &mut Observations<'_>) {
        observations.visit_item(&self.item);
    }
    fn into_original(self) -> Option<OriginalItem> {
        Some(self)
    }
    fn from_original(source: OriginalItem) -> Self {
        source
    }
    fn split(self) -> (Self::Restored, Snapshot) {
        (self.item, self.proof)
    }
    fn attach_restored(
        restored: &mut Self::Restored,
        proof: &Snapshot,
        facts: &mut ReferenceFacts,
    ) {
        proof.attach_item(restored, facts);
    }
}

impl RegionSource for OriginalStmt {
    type Restored = syn::Stmt;
    fn item(&self) -> &syn::Item {
        match &self.stmt {
            syn::Stmt::Item(item) => item,
            _ => unreachable!(),
        }
    }
    fn visit(&mut self, normalize: &mut Normalize<'_>) {
        normalize.visit_stmt_mut(&mut self.stmt);
    }
    fn reprove(&mut self, facts: &ReferenceFacts) {
        self.proof = Snapshot::statement(&self.stmt, facts);
    }
    fn clear(&self, facts: &mut ReferenceFacts) {
        Clear(facts).visit_stmt(&self.stmt);
    }
    fn attach(&mut self, facts: &mut ReferenceFacts) {
        self.proof.attach_stmt(&mut self.stmt, facts);
    }
    fn admitted(&self) -> bool {
        matches!(&self.stmt, syn::Stmt::Item(item) if admitted(item))
    }
    fn group_member(&self) -> bool {
        matches!(&self.stmt, syn::Stmt::Item(item) if group_member(item))
    }
    fn observe(&self, observations: &mut Observations<'_>) {
        observations.visit_stmt(&self.stmt);
    }
    fn into_original(self) -> Option<OriginalItem> {
        match self.stmt {
            syn::Stmt::Item(item) => Some(OriginalItem {
                item,
                proof: self.proof,
            }),
            _ => unreachable!(),
        }
    }
    fn from_original(source: OriginalItem) -> Self {
        OriginalStmt {
            stmt: syn::Stmt::Item(source.item),
            proof: source.proof,
        }
    }
    fn split(self) -> (Self::Restored, Snapshot) {
        (self.stmt, self.proof)
    }
    fn attach_restored(
        restored: &mut Self::Restored,
        proof: &Snapshot,
        facts: &mut ReferenceFacts,
    ) {
        proof.attach_stmt(restored, facts);
    }
}

fn generic_ports(item: &syn::ItemImpl, facts: &ReferenceFacts) -> Vec<(ResolvedOrigin, Ident)> {
    item.generics
        .params
        .iter()
        .filter_map(|param| {
            let ident = match param {
                syn::GenericParam::Lifetime(param) => &param.lifetime.ident,
                syn::GenericParam::Type(param) => &param.ident,
                syn::GenericParam::Const(param) => &param.ident,
            };
            facts
                .idents
                .get(&core::ptr::from_ref(ident))
                .map(|&origin| (origin, ident.clone()))
        })
        .collect()
}

struct Rebind<'a> {
    records: core::slice::IterMut<'a, Option<(ResolvedOrigin, bool)>>,
    macros: core::slice::IterMut<'a, Vec<Resolution>>,
    aliases: &'a BTreeMap<(bool, BindingId), (ResolvedOrigin, Ident)>,
}

impl VisitMut for Rebind<'_> {
    fn visit_ident_mut(&mut self, ident: &mut Ident) {
        if let Some(Some((origin, _))) = self.records.next()
            && let Some((target, name)) = self.aliases.get(&identity(*origin))
        {
            let span = ident.span();
            *ident = name.clone();
            ident.set_span(span);
            *origin = ResolvedOrigin {
                ns: origin.ns,
                ..*target
            };
        }
    }

    fn visit_macro_mut(&mut self, mac: &mut syn::Macro) {
        visit_mut::visit_macro_mut(self, mac);
        if let Some(records) = self.macros.next() {
            let aliases = self.aliases;
            let mut replacement = |origin: ResolvedOrigin| {
                aliases
                    .get(&identity(origin))
                    .map(|(_, name)| name.to_string())
            };
            // apply proves full stream consumption before rebinding, rebase only with the rewrite.
            rewrite_original_macro(mac, records, &mut replacement);
            for resolution in records {
                if let Some(origin) = resolution.origin
                    && let Some((target, _)) = aliases.get(&identity(origin))
                {
                    resolution.origin = Some(ResolvedOrigin {
                        ns: origin.ns,
                        ..*target
                    });
                }
            }
        }
    }
}

struct Key<'a> {
    group: &'a BTreeMap<(bool, BindingId), usize>,
    locals: BTreeMap<(bool, BindingId), usize>,
    idents: alloc::vec::IntoIter<Option<(ResolvedOrigin, bool)>>,
    macros: alloc::vec::IntoIter<Vec<Resolution>>,
    paths: alloc::vec::IntoIter<PathProof>,
    markers: BTreeMap<String, Ns>,
    proofs: &'a mut crate::helpers::HelperSyntax,
    edges: &'a mut crate::form::MarkerEdges,
    pending: Option<(crate::scope::HelperId, crate::helpers::ScopeId)>,
}

impl Key<'_> {
    fn replacement(&mut self, origin: ResolvedOrigin) -> Option<String> {
        if self.group.contains_key(&identity(origin)) {
            let name = format!("__dejadoc_m8_group_{}", namespace(origin.ns));
            self.markers.entry(name.clone()).or_insert(origin.ns);
            Some(name)
        } else {
            self.locals.get(&identity(origin)).map(|&slot| {
                let name = format!("__dejadoc_m8_local_{}_{}", namespace(origin.ns), slot);
                self.markers.entry(name.clone()).or_insert(origin.ns);
                name
            })
        }
    }
}

impl VisitMut for Key<'_> {
    fn visit_path_mut(&mut self, path: &mut syn::Path) {
        let proof = self
            .paths
            .next()
            .and_then(|proof| proof.helper.zip(proof.scope));
        let parent = core::mem::replace(&mut self.pending, proof);
        visit_mut::visit_path_mut(self, path);
        self.pending = parent;
    }
    fn visit_ident_mut(&mut self, ident: &mut Ident) {
        if let Some(Some((origin, declaration))) = self.idents.next()
            && let Some(replacement) = self.replacement(origin)
        {
            if !declaration {
                if let Some(&slot) = self.group.get(&identity(origin)) {
                    self.edges.record_ident(ident, origin.ns, slot);
                }
                if let Some((helper, scope)) = self.pending.take() {
                    self.proofs.put_call_proof(ident, helper, scope);
                    self.proofs.put_proof_canon(helper, &replacement);
                }
            }
            *ident = Ident::new(&replacement, ident.span());
        }
    }
    fn visit_macro_mut(&mut self, mac: &mut syn::Macro) {
        let resolutions = self
            .macros
            .next()
            .expect("one macro record per visited macro node");
        let mut replacement = |origin: ResolvedOrigin| self.replacement(origin);
        rewrite_original_macro(mac, &resolutions, &mut replacement);
        self.edges.record_macro(
            mac,
            resolutions
                .iter()
                .map(|resolution| {
                    resolution.origin.and_then(|origin| {
                        self.group
                            .get(&identity(origin))
                            .map(|&slot| (origin.ns, slot))
                    })
                })
                .collect(),
        );
        visit_mut::visit_macro_mut(self, mac);
    }
}
/// Rewrite the macro's arguments through the exact original occurrence stream.
pub(super) fn rewrite_original_macro(
    mac: &mut syn::Macro,
    resolutions: &[Resolution],
    replacement: &mut dyn FnMut(ResolvedOrigin) -> Option<String>,
) {
    let mut rewriter = MacroRewriter {
        resolutions,
        replacement,
        cursor: 0,
        after_bare_qself: false,
    };
    mac.tokens = rewriter.rewrite_tokens(mac);
}
struct MacroRewriter<'a> {
    resolutions: &'a [Resolution],
    replacement: &'a mut dyn FnMut(ResolvedOrigin) -> Option<String>,
    cursor: usize,
    after_bare_qself: bool,
}

impl MacroRewriter<'_> {
    /// Take the next recorded occurrence, in order.
    fn take(&mut self) -> Resolution {
        let resolution = self
            .resolutions
            .get(self.cursor)
            .copied()
            .expect("the recorded occurrence stream covers every consumed position");
        self.cursor += 1;
        resolution
    }

    /// The replacement as an identifier, a leading `r#` making it raw.
    fn marker(text: &str, span: proc_macro2::Span) -> Ident {
        if let Some(raw) = text.strip_prefix("r#") {
            Ident::new_raw(raw, span)
        } else {
            Ident::new(text, span)
        }
    }

    /// The replacement as a format capture spelling, raw names unmarked.
    fn capture(text: &str) -> &str {
        text.strip_prefix("r#").unwrap_or(text)
    }

    /// Rewrite one argument stream through the recorded occurrences.
    fn rewrite_tokens(&mut self, mac: &syn::Macro) -> proc_macro2::TokenStream {
        let format_at = crate::drift::format_operand(&mac.path).map(|(at, _)| at);
        let mut args = syn::parse2::<crate::alpha::MacroArgs>(mac.tokens.clone())
            .expect("scope_blocked rejects macro arguments that fail to parse");
        for (index, expr) in args.iter_mut().enumerate() {
            visit_mut::visit_expr_mut(self, expr);
            if Some(index) == format_at
                && let syn::Expr::Lit(syn::ExprLit {
                    lit: syn::Lit::Str(lit),
                    ..
                }) = expr
            {
                self.rewrite_format_str(lit);
            }
        }
        args.to_token_stream()
    }

    /// The inline `{name}` references of a format string, `{{` and `}}`
    /// escapes untouched.
    fn rewrite_format_str(&mut self, lit: &mut syn::LitStr) {
        let value = lit.value();
        let mut out = String::with_capacity(value.len());
        let mut chars = value.chars().peekable();
        while let Some(ch) = chars.next() {
            if matches!(ch, '{' | '}') && chars.peek() == Some(&ch) {
                chars.next();
                out.push(ch);
                out.push(ch);
                continue;
            }
            if ch != '{' {
                out.push(ch);
                continue;
            }
            let mut body = String::new();
            let mut closed = false;
            for ch in chars.by_ref() {
                if ch == '}' {
                    closed = true;
                    break;
                }
                body.push(ch);
            }
            out.push('{');
            self.rewrite_format_arg(&body, &mut out);
            if closed {
                out.push('}');
            }
        }
        *lit = syn::LitStr::new(&out, lit.span());
    }

    /// One placeholder body, its name and any `name$` width or precision.
    fn rewrite_format_arg(&mut self, arg: &str, out: &mut String) {
        let (name, spec) = arg
            .split_once(':')
            .map_or((arg, None), |(name, spec)| (name, Some(spec)));
        self.rewrite_format_name(name, out);
        let Some(spec) = spec else {
            return;
        };
        out.push(':');
        let mut rest = spec;
        while let Some((before, after)) = rest.split_once('$') {
            let start = before
                .char_indices()
                .rfind(|&(_, ch)| !ch.is_alphanumeric() && ch != '_')
                .map_or(0, |(pos, ch)| pos + ch.len_utf8());
            out.push_str(&before[..start]);
            self.rewrite_format_name(&before[start..], out);
            out.push('$');
            rest = after;
        }
        out.push_str(rest);
    }

    /// One capture name, one occurrence.
    fn rewrite_format_name(&mut self, name: &str, out: &mut String) {
        let replacement = self
            .take()
            .origin
            .and_then(|origin| (self.replacement)(origin));
        out.push_str(replacement.as_deref().map_or(name, Self::capture));
    }

    /// Bind each let after its operand, only through `&&`.
    fn walk_let_cond(&mut self, cond: &mut syn::Expr) {
        match cond {
            syn::Expr::Binary(binary) => {
                self.walk_let_cond(&mut binary.left);
                self.walk_let_cond(&mut binary.right);
            }
            syn::Expr::Let(let_expr) => {
                visit_mut::visit_expr_mut(self, &mut let_expr.expr);
                visit_mut::visit_pat_mut(self, &mut let_expr.pat);
            }
            other => visit_mut::visit_expr_mut(self, other),
        }
    }
}

impl VisitMut for MacroRewriter<'_> {
    fn visit_path_mut(&mut self, path: &mut syn::Path) {
        if !core::mem::take(&mut self.after_bare_qself) {
            let resolution = self.take();
            if let Some(origin) = resolution.origin
                && let Some(text) = (self.replacement)(origin)
                && let Some(first) = path.segments.first_mut()
            {
                first.ident = Self::marker(&text, first.ident.span());
            }
        }
        visit_mut::visit_path_mut(self, path);
    }

    /// syn visits a qualified path's `<T>` right before its path.
    fn visit_qself_mut(&mut self, qself: &mut syn::QSelf) {
        visit_mut::visit_qself_mut(self, qself);
        self.after_bare_qself = qself.position == 0;
    }

    /// The collector records a nested macro's name before its arguments.
    fn visit_macro_mut(&mut self, mac: &mut syn::Macro) {
        let resolution = self.take();
        if let Some(first) = mac.path.segments.first_mut()
            && let Some(origin) = resolution.origin
            && let Some(text) = (self.replacement)(origin)
        {
            first.ident = Self::marker(&text, first.ident.span());
        }
        mac.tokens = self.rewrite_tokens(mac);
    }

    /// The collector does not resolve a match arm's guard.
    fn visit_arm_mut(&mut self, arm: &mut syn::Arm) {
        visit_mut::visit_pat_mut(self, &mut arm.pat);
        visit_mut::visit_expr_mut(self, &mut arm.body);
    }

    fn visit_expr_if_mut(&mut self, expr: &mut syn::ExprIf) {
        self.walk_let_cond(&mut expr.cond);
        visit_mut::visit_block_mut(self, &mut expr.then_branch);
        if let Some((_, else_expr)) = &mut expr.else_branch {
            visit_mut::visit_expr_mut(self, else_expr);
        }
    }

    fn visit_expr_while_mut(&mut self, expr: &mut syn::ExprWhile) {
        self.walk_let_cond(&mut expr.cond);
        visit_mut::visit_block_mut(self, &mut expr.body);
    }

    /// The iterable resolves under the outer bindings.
    fn visit_expr_for_loop_mut(&mut self, for_loop: &mut syn::ExprForLoop) {
        visit_mut::visit_expr_mut(self, &mut for_loop.expr);
        visit_mut::visit_pat_mut(self, &mut for_loop.pat);
        visit_mut::visit_block_mut(self, &mut for_loop.body);
    }

    /// A local's initializer resolves under the outer bindings.
    fn visit_local_mut(&mut self, local: &mut syn::Local) {
        if let Some(init) = &mut local.init {
            visit_mut::visit_expr_mut(self, &mut init.expr);
            if let Some(diverge) = &mut init.diverge {
                visit_mut::visit_expr_mut(self, &mut diverge.1);
            }
        }
        visit_mut::visit_pat_mut(self, &mut local.pat);
    }

    /// An attribute path names an external item, but a `#[attr = expr]`
    /// value reads the local bindings.
    fn visit_attribute_mut(&mut self, attribute: &mut syn::Attribute) {
        if let syn::Meta::NameValue(name_value) = &mut attribute.meta {
            visit_mut::visit_expr_mut(self, &mut name_value.value);
        }
    }
}

fn label(
    item: &syn::Item,
    facts: &ReferenceFacts,
    group: &BTreeMap<(bool, BindingId), usize>,
    table: Option<&crate::helpers::HelperTable>,
) -> Lbl {
    label_snapshot(item.clone(), Snapshot::collect(item, facts), group, table)
}

fn declaration(item: &syn::Item, facts: &ReferenceFacts) -> Option<ResolvedOrigin> {
    let (_, ident) = crate::alpha::item_binding(item)?;
    facts.idents.get(&core::ptr::from_ref(ident)).copied()
}

fn owner(item: &syn::ItemImpl, facts: &ReferenceFacts) -> Option<ResolvedOrigin> {
    let syn::Type::Path(ty) = item.self_ty.as_ref() else {
        return None;
    };
    if ty.qself.is_some() {
        return None;
    }
    let last = ty.path.segments.last()?;
    facts.idents.get(&core::ptr::from_ref(&last.ident)).copied()
}

fn normalize_owner(item: &mut syn::ItemImpl, facts: &mut ReferenceFacts) {
    let origin = owner(item, facts)
        .expect("every normalize_owner caller re-proves the owner that partitions() already found");
    let Some(route) = facts.owner_routes.get(&identity(origin)) else {
        return;
    };
    let syn::Type::Path(ty) = item.self_ty.as_mut() else {
        return;
    };
    if route.segments.len() == ty.path.segments.len()
        && route
            .segments
            .iter()
            .zip(&ty.path.segments)
            .all(|(name, segment)| segment.ident == *name)
    {
        return;
    }
    let last = ty.path.segments.last_mut().expect("a proven owner");
    let span = last.ident.span();
    let arguments = core::mem::replace(&mut last.arguments, syn::PathArguments::None);
    ty.path.segments = route
        .segments
        .iter()
        .map(|name| syn::PathSegment::from(MacroRewriter::marker(name, span)))
        .collect();
    ty.path
        .segments
        .last_mut()
        .expect("a proven owner")
        .arguments = arguments;
    for (segment, origin) in ty.path.segments.iter().zip(&route.origins) {
        facts
            .idents
            .insert(core::ptr::from_ref(&segment.ident), *origin);
    }
    facts.original_paths.insert(
        core::ptr::from_ref(ty.path.segments.first().expect("a proven owner")),
        route.first,
    );
}

fn normalized_header(item: &syn::ItemImpl, facts: &ReferenceFacts) -> (syn::ItemImpl, Snapshot) {
    let mut header = clone_header(item);
    let mut original = ReferenceFacts::default();
    Snapshot::header(item, facts).attach_impl(&mut header, &mut original);
    normalize_owner(&mut header, &mut original);
    let proof = Snapshot::header(&header, &original);
    (header, proof)
}

fn member_name(member: &syn::ImplItem) -> Option<(u8, String)> {
    Some(match member {
        syn::ImplItem::Fn(item) => (0, crate::scope::ident_name(&item.sig.ident)),
        syn::ImplItem::Const(item) => (0, crate::scope::ident_name(&item.ident)),
        syn::ImplItem::Type(item) => (1, crate::scope::ident_name(&item.ident)),
        _ => return None,
    })
}

struct Partition {
    indices: Vec<usize>,
    members: Vec<(usize, usize)>,
    label: Lbl,
}

struct MemberLabel {
    name: (u8, String),
    label: Lbl,
    index: usize,
    member_index: usize,
}

struct RebindPlan {
    aliases: BTreeMap<(bool, BindingId), (ResolvedOrigin, Ident)>,
    proofs: Vec<Snapshot>,
}

fn partitions(
    items: &[&syn::Item],
    facts: &ReferenceFacts,
    group: &BTreeMap<(bool, BindingId), usize>,
    table: Option<&crate::helpers::HelperTable>,
) -> Option<Vec<Partition>> {
    let mut output: Vec<Partition> = Vec::new();
    let mut headers: BTreeMap<((bool, BindingId), Lbl), usize> = BTreeMap::new();
    let mut member_sets: Vec<BTreeSet<(u8, String)>> = Vec::new();
    let mut member_labels: Vec<Vec<MemberLabel>> = Vec::new();
    for (index, item) in items.iter().enumerate() {
        if let syn::Item::Impl(implementation) = item {
            let resolved_owner = owner(implementation, facts)?;
            let (header, proof) = normalized_header(implementation, facts);
            let header_label = label_snapshot(syn::Item::Impl(header), proof, group, table);
            let key = (identity(resolved_owner), header_label.clone());
            let partition = if let Some(&partition) = headers.get(&key) {
                partition
            } else {
                let partition = output.len();
                headers.insert(key, partition);
                output.push(Partition {
                    indices: Vec::new(),
                    members: Vec::new(),
                    label: header_label,
                });
                member_sets.push(BTreeSet::new());
                member_labels.push(Vec::new());
                partition
            };
            output[partition].indices.push(index);
            for (member_index, member) in implementation.items.iter().enumerate() {
                let name = member_name(member)?;
                if !member_sets[partition].insert(name.clone()) {
                    return None;
                }
                let (mut single, mut records) = normalized_header(implementation, facts);
                single.items.push(member.clone());
                records.append(Snapshot::member(member, facts));
                let member_label = label_snapshot(syn::Item::Impl(single), records, group, table);
                member_labels[partition].push(MemberLabel {
                    name,
                    label: member_label,
                    index,
                    member_index,
                });
            }
        } else {
            output.push(Partition {
                indices: vec![index],
                members: Vec::new(),
                label: label(item, facts, group, table),
            });
            member_sets.push(BTreeSet::new());
            member_labels.push(Vec::new());
        }
    }
    for (partition, mut members) in output.iter_mut().zip(member_labels) {
        if !members.is_empty() {
            members.sort_by(|left, right| left.name.cmp(&right.name));
            partition.members = members
                .iter()
                .map(|member| (member.index, member.member_index))
                .collect();
            partition.label = Lbl::Other(
                "impl".into(),
                vec![
                    partition.label.clone(),
                    Lbl::Other(
                        "members".into(),
                        members.into_iter().map(|member| member.label).collect(),
                    ),
                ],
            );
        }
    }
    Some(output)
}

struct Selected<'a> {
    facts: &'a ReferenceFacts,
    idents: Vec<Option<(ResolvedOrigin, bool)>>,
    macros: Vec<Vec<Resolution>>,
    paths: Vec<PathProof>,
}

impl<'a> Selected<'a> {
    fn new(facts: &'a ReferenceFacts) -> Self {
        Self {
            facts,
            idents: Vec::new(),
            macros: Vec::new(),
            paths: Vec::new(),
        }
    }

    fn finish(self) -> Snapshot {
        let parameters = self
            .idents
            .iter()
            .flatten()
            .filter_map(|&(origin, _)| {
                self.facts
                    .original_parameters
                    .get(&identity(origin))
                    .map(|&port| (identity(origin), port))
            })
            .collect();
        let owner_routes = self
            .idents
            .iter()
            .flatten()
            .filter_map(|&(origin, _)| {
                self.facts
                    .owner_routes
                    .get(&identity(origin))
                    .map(|route| (identity(origin), route.clone()))
            })
            .collect();
        Snapshot {
            idents: self.idents,
            macros: self.macros,
            paths: self.paths,
            parameters,
            owner_routes,
        }
    }
}

impl<'ast> Visit<'ast> for Selected<'_> {
    fn visit_ident(&mut self, ident: &'ast Ident) {
        let key = core::ptr::from_ref(ident);
        self.idents.push(
            self.facts
                .idents
                .get(&key)
                .map(|&origin| (origin, self.facts.declarations.contains(&key))),
        );
    }
    fn visit_macro(&mut self, mac: &'ast syn::Macro) {
        self.macros.push(
            mac.path
                .segments
                .first()
                .and_then(|first| self.facts.original_macros.get(&core::ptr::from_ref(first)))
                .cloned()
                .unwrap_or_default(),
        );
        visit::visit_macro(self, mac);
    }
    fn visit_path(&mut self, path: &'ast syn::Path) {
        let proof = path.segments.first().map_or(PathProof::default(), |first| {
            let key = core::ptr::from_ref(first);
            PathProof {
                resolution: self.facts.original_paths.get(&key).copied(),
                portable: self.facts.portable_paths.contains(&key),
                known: self.facts.known_macros.contains(&key),
                helper: self.facts.original_calls.get(&key).copied(),
                scope: self.facts.original_call_scopes.get(&key).copied(),
            }
        });
        self.paths.push(proof);
        visit::visit_path(self, path);
    }
}

fn label_snapshot(
    mut item: syn::Item,
    snapshot: Snapshot,
    group: &BTreeMap<(bool, BindingId), usize>,
    table: Option<&crate::helpers::HelperTable>,
) -> Lbl {
    struct Nested(bool);
    impl Visit<'_> for Nested {
        fn visit_block(&mut self, block: &syn::Block) {
            self.0 |= block
                .stmts
                .iter()
                .any(|stmt| matches!(stmt, syn::Stmt::Item(_)));
            visit::visit_block(self, block);
        }
        fn visit_item_mod(&mut self, item: &syn::ItemMod) {
            self.0 |= item.content.is_some();
        }
    }
    let mut nested = Nested(false);
    nested.visit_item(&item);
    let snapshot = if nested.0 {
        let mut facts = ReferenceFacts::default();
        snapshot.attach_item(&mut item, &mut facts);
        let mut helpers = crate::helpers::HelperSyntax::default();
        let mut normalize = Normalize {
            facts,
            helpers: &mut helpers,
            table,
            blocked: false,
        };
        normalize.visit_item_mut(&mut item);
        Snapshot::collect(&item, &normalize.facts)
    } else {
        snapshot
    };
    let mut locals = BTreeMap::new();
    for &(origin, declaration) in snapshot.idents.iter().flatten() {
        if declaration && !group.contains_key(&identity(origin)) {
            let next = locals.len();
            locals.entry(identity(origin)).or_insert(next);
        }
    }
    let mut proofs = crate::helpers::HelperSyntax::default();
    let mut edges = crate::form::MarkerEdges::new();
    let mut key = Key {
        group,
        locals,
        idents: snapshot.idents.into_iter(),
        macros: snapshot.macros.into_iter(),
        paths: snapshot.paths.into_iter(),
        markers: BTreeMap::new(),
        proofs: &mut proofs,
        edges: &mut edges,
        pending: None,
    };
    key.visit_item_mut(&mut item);
    let frame = marker_frame(&key.markers);
    let (payload, references) = crate::form::item_payload(
        syn::File {
            shebang: None,
            frontmatter: None,
            attrs: Vec::new(),
            items: vec![item],
        },
        crate::form::PayloadProof {
            table,
            frame: &frame,
            syntax: proofs,
            edges,
        },
    );
    let mut labels: Vec<_> = references
        .into_iter()
        .map(|(ns, slot)| Lbl::Other(namespace(ns).into(), vec![Lbl::Future(slot, None)]))
        .collect();
    labels.insert(0, Lbl::Lit(payload.into_key()));
    Lbl::Other("item".into(), labels)
}

fn marker_frame(markers: &BTreeMap<String, Ns>) -> Frame {
    let mut frame = Frame::default();
    for (id, (name, &ns)) in markers.iter().enumerate() {
        frame.bind(
            ns,
            name,
            Binding {
                id: BindingId(id),
                canon: name.clone(),
                origin: Origin::Item,
                prim: None,
                port: 0,
                value_port: None,
                target: None,
                raw: false,
                helper: None,
            },
        );
    }
    frame
}

fn rank_payloads(labels: &mut [Lbl], root: &mut Lbl) {
    fn extract(label: &mut Lbl, texts: &mut Vec<String>) {
        match label {
            Lbl::Lit(text) => {
                let slot = texts.len();
                texts.push(core::mem::take(text));
                *label = Lbl::Int(slot as u128, None);
            }
            Lbl::Other(_, children) => {
                for child in children {
                    extract(child, texts);
                }
            }
            _ => {}
        }
    }
    fn substitute(label: &mut Lbl, ranks: &[usize]) {
        match label {
            Lbl::Int(slot, None) => *slot = ranks[*slot as usize] as u128,
            Lbl::Other(_, children) => {
                for child in children {
                    substitute(child, ranks);
                }
            }
            _ => {}
        }
    }
    let mut texts = Vec::new();
    for label in labels.iter_mut() {
        extract(label, &mut texts);
    }
    extract(root, &mut texts);
    let mut sorted: Vec<_> = (0..texts.len()).collect();
    sorted.sort_unstable_by(|&left, &right| texts[left].cmp(&texts[right]));
    let mut ranks = vec![0; texts.len()];
    let mut rank = 0;
    for (position, &slot) in sorted.iter().enumerate() {
        if position > 0 && texts[slot] != texts[sorted[position - 1]] {
            rank += 1;
        }
        ranks[slot] = rank;
    }
    for label in labels {
        substitute(label, &ranks);
    }
    substitute(root, &ranks);
}

fn remap(label: &mut Lbl, positions: &[usize]) {
    match label {
        Lbl::Future(slot, _) => *slot = positions[*slot],
        Lbl::Other(_, children) | Lbl::Flat(_, children, _) => {
            for child in children {
                remap(child, positions);
            }
        }
        Lbl::Not(inner) => remap(inner, positions),
        Lbl::Op(_, left, right, _) => {
            remap(left, positions);
            remap(right, positions);
        }
        Lbl::Call(callee, arguments, _) => {
            remap(callee, positions);
            for argument in arguments {
                remap(argument, positions);
            }
        }
        _ => {}
    }
}

struct RootContext {
    root: Lbl,
    definitions: Vec<Lbl>,
    budget: Budget,
}

struct Observations<'a> {
    facts: &'a ReferenceFacts,
    table: Option<&'a crate::helpers::HelperTable>,
    group: BTreeMap<(bool, BindingId), usize>,
    producers: BTreeMap<(bool, BindingId), (usize, crate::scope::PrimTy)>,
    edges: Vec<Lbl>,
    definitions: Vec<Lbl>,
    tainted: BTreeSet<(bool, BindingId)>,
    locals: BTreeSet<(bool, BindingId)>,
    budget: Budget,
}

impl<'a> Observations<'a> {
    fn new(
        items: &[&syn::Item],
        facts: &'a ReferenceFacts,
        table: Option<&'a crate::helpers::HelperTable>,
    ) -> Self {
        let mut group = BTreeMap::new();
        for item in items {
            if let Some(origin) = declaration(item, facts) {
                let slot = group.len();
                group.insert(identity(origin), slot);
            }
        }
        Self {
            facts,
            table,
            group,
            producers: BTreeMap::new(),
            edges: Vec::new(),
            definitions: Vec::new(),
            tainted: BTreeSet::new(),
            locals: BTreeSet::new(),
            budget: Budget {
                nodes: 1,
                ..Budget::default()
            },
        }
    }

    fn port(&self, origin: ResolvedOrigin) -> Option<Lbl> {
        let slot = self
            .group
            .get(&identity(origin))
            .copied()
            .or_else(|| self.producers.get(&identity(origin)).map(|&(slot, _)| slot))?;
        Some(Lbl::Other(
            namespace(origin.ns).into(),
            vec![Lbl::Future(slot, None)],
        ))
    }

    fn reference(&mut self, origin: ResolvedOrigin) {
        if !self.budget.within() {
            return;
        }
        if let Some(port) = self.port(origin) {
            self.budget.label(&port, 1);
            self.edges.push(port);
        }
    }

    fn define(&mut self, label: Lbl) {
        self.budget.label(&label, 0);
        self.definitions.push(label);
    }

    fn scalar(&mut self, expr: &syn::Expr) -> Option<(Lbl, crate::scope::PrimTy)> {
        if !self.budget.within() {
            return None;
        }
        if self.budget.depth == crate::schedule::MAX_BLOCK_DEPTH {
            self.budget.deepest = crate::schedule::MAX_BLOCK_DEPTH + 1;
            return None;
        }
        self.budget.depth += 1;
        let result = self.scalar_value(expr);
        self.budget.depth -= 1;
        result
    }

    fn scalar_value(&mut self, expr: &syn::Expr) -> Option<(Lbl, crate::scope::PrimTy)> {
        if !crate::drift::expr_attrs(expr).is_empty() {
            return None;
        }
        match expr {
            syn::Expr::Paren(expr) => self.scalar(&expr.expr),
            syn::Expr::Group(expr) => self.scalar(&expr.expr),
            syn::Expr::Path(path) if path.qself.is_none() && path.path.segments.len() == 1 => {
                self.scalar_path(path)
            }
            syn::Expr::Call(call) => self.scalar_call(call),
            syn::Expr::Unary(unary) if matches!(unary.op, syn::UnOp::Not(_)) => {
                let (operand, prim) = self.scalar(&unary.expr)?;
                (prim == crate::scope::PrimTy::Bool)
                    .then(|| (crate::schedule::not_label(operand), prim))
            }
            syn::Expr::Binary(binary) => self.scalar_binary(binary),
            syn::Expr::If(branch) => self.scalar_if(branch),
            _ => {
                let label = crate::schedule::scalar_leaf(&BTreeMap::new(), expr)?;
                let prim = label.prim()?;
                Some((label, prim))
            }
        }
    }

    fn scalar_path(&mut self, path: &syn::ExprPath) -> Option<(Lbl, crate::scope::PrimTy)> {
        let first = path.path.segments.first()?;
        let resolution = self.facts.original_paths.get(&core::ptr::from_ref(first))?;
        let origin = resolution.origin?;
        if let Some(&(slot, prim)) = self.producers.get(&identity(origin)) {
            return Some((Lbl::Future(slot, Some(prim)), prim));
        }
        let prim = resolution.prim?;
        let port = self.facts.original_parameters.get(&identity(origin))?;
        let slot = self.group.len() + self.definitions.len();
        self.producers.insert(identity(origin), (slot, prim));
        self.define(Lbl::Other(format!("parameter-{port}"), Vec::new()));
        Some((Lbl::Future(slot, Some(prim)), prim))
    }

    fn scalar_call(&mut self, call: &syn::ExprCall) -> Option<(Lbl, crate::scope::PrimTy)> {
        let syn::Expr::Path(path) = call.func.as_ref() else {
            return None;
        };
        let first = path.path.segments.first()?;
        let helper = self.facts.original_calls.get(&core::ptr::from_ref(first))?;
        let summary = self.table?.get(*helper)?;
        if call.args.len() != summary.params.len() {
            return None;
        }
        let origin = path
            .path
            .segments
            .last()
            .and_then(|last| self.facts.idents.get(&core::ptr::from_ref(&last.ident)))
            .copied()?;
        let callee = self.port(origin)?;
        let mut arguments = Vec::with_capacity(call.args.len());
        for (arg, param) in call.args.iter().zip(&summary.params) {
            let (label, prim) = self.scalar(arg)?;
            if prim != *param {
                return None;
            }
            arguments.push(label);
        }
        Some((
            Lbl::Call(alloc::boxed::Box::new(callee), arguments, summary.result),
            summary.result,
        ))
    }

    fn scalar_binary(&mut self, binary: &syn::ExprBinary) -> Option<(Lbl, crate::scope::PrimTy)> {
        let (left, _) = self.scalar(&binary.left)?;
        let (right, _) = self.scalar(&binary.right)?;
        let label =
            if let Some(reduced) = crate::schedule::true_comparison(binary.op, &left, &right) {
                reduced
            } else {
                match crate::schedule::total_binary(binary.op, &left, &right)? {
                    crate::schedule::TotalBinary::Ordered { tag, reverse, prim } => {
                        let (left, right) = if reverse.is_some() {
                            (right, left)
                        } else {
                            (left, right)
                        };
                        Lbl::Op(
                            tag,
                            alloc::boxed::Box::new(left),
                            alloc::boxed::Box::new(right),
                            prim,
                        )
                    }
                    crate::schedule::TotalBinary::Flat(op, prim) => {
                        let mut leaves = Vec::new();
                        crate::schedule::collect_leaves(op, prim, left, &mut leaves);
                        crate::schedule::collect_leaves(op, prim, right, &mut leaves);
                        crate::schedule::flat_label(op, prim, leaves)
                    }
                }
            };
        let prim = label.prim()?;
        Some((label, prim))
    }

    fn scalar_if(&mut self, branch: &syn::ExprIf) -> Option<(Lbl, crate::scope::PrimTy)> {
        let (_, otherwise) = branch.else_branch.as_ref()?;
        let syn::Expr::Block(otherwise) = otherwise.as_ref() else {
            return None;
        };
        if !crate::schedule::branches_unattributed(&branch.attrs, &branch.then_branch, otherwise) {
            return None;
        }
        let (condition, _) = self.scalar(&branch.cond)?;
        let (then, _) = self.scalar(crate::schedule::scalar_tail(&branch.then_branch)?)?;
        let (otherwise, _) = self.scalar(crate::schedule::scalar_tail(&otherwise.block)?)?;
        let label = crate::schedule::branch_value(&condition, &then, &otherwise)?;
        let prim = label.prim()?;
        Some((label, prim))
    }

    fn finish(self) -> Option<RootContext> {
        if !self.budget.within()
            || self
                .tainted
                .iter()
                .any(|origin| self.producers.contains_key(origin))
        {
            return None;
        }
        Some(RootContext {
            root: Lbl::Other("roots".into(), self.edges),
            definitions: self.definitions,
            budget: self.budget,
        })
    }

    fn record_write(&mut self, left: &syn::Expr, right: &syn::Expr) {
        IdentOrigins {
            facts: self.facts,
            out: &mut self.tainted,
        }
        .visit_expr(left);
        let local = match left {
            syn::Expr::Path(path)
                if path.qself.is_none()
                    && path.path.leading_colon.is_none()
                    && path.path.segments.len() == 1 =>
            {
                self.facts
                    .idents
                    .get(&core::ptr::from_ref(&path.path.segments[0].ident))
                    .is_some_and(|origin| {
                        self.locals.contains(&identity(*origin))
                            || self
                                .facts
                                .original_parameters
                                .contains_key(&identity(*origin))
                    })
            }
            _ => false,
        };
        if !local {
            IdentOrigins {
                facts: self.facts,
                out: &mut self.tainted,
            }
            .visit_expr(right);
        }
    }
}

struct IdentOrigins<'a> {
    facts: &'a ReferenceFacts,
    out: &'a mut BTreeSet<(bool, BindingId)>,
}

impl Visit<'_> for IdentOrigins<'_> {
    fn visit_ident(&mut self, ident: &Ident) {
        if let Some(&origin) = self.facts.idents.get(&core::ptr::from_ref(ident))
            && origin.ns == Ns::Value
        {
            self.out.insert(identity(origin));
        }
    }
}

impl Visit<'_> for Observations<'_> {
    fn visit_item(&mut self, item: &syn::Item) {
        if !self.budget.within() {
            return;
        }
        let outer = core::mem::take(&mut self.locals);
        visit::visit_item(self, item);
        self.locals = outer;
    }

    fn visit_stmt(&mut self, stmt: &syn::Stmt) {
        if self.budget.within() {
            visit::visit_stmt(self, stmt);
        }
    }
    fn visit_local(&mut self, local: &syn::Local) {
        let pattern = match &local.pat {
            syn::Pat::Type(pattern) => pattern.pat.as_ref(),
            pattern => pattern,
        };
        if let syn::Pat::Ident(pattern) = pattern
            && let Some(&origin) = self.facts.idents.get(&core::ptr::from_ref(&pattern.ident))
            && origin.ns == Ns::Value
        {
            self.locals.insert(identity(origin));
        }
        if local.attrs.is_empty()
            && let syn::Pat::Ident(pattern) = pattern
            && pattern.by_ref.is_none()
            && pattern.mutability.is_none()
            && pattern.subpat.is_none()
            && let Some(init) = &local.init
            && init.diverge.is_none()
            && let Some(&origin) = self.facts.idents.get(&core::ptr::from_ref(&pattern.ident))
            && let Some((definition, prim)) = self.scalar(&init.expr)
        {
            let slot = self.group.len() + self.definitions.len();
            if let Lbl::Future(source, Some(_)) = definition {
                self.producers.insert(identity(origin), (source, prim));
                return;
            }
            self.producers.insert(identity(origin), (slot, prim));
            self.define(Lbl::Other(
                format!("producer-{}", prim.suffix()),
                vec![definition],
            ));
            return;
        }
        visit::visit_local(self, local);
    }
    fn visit_ident(&mut self, ident: &Ident) {
        let key = core::ptr::from_ref(ident);
        if self.facts.declarations.contains(&key) {
            if let Some(&origin) = self.facts.idents.get(&key)
                && origin.ns == Ns::Value
            {
                self.locals.insert(identity(origin));
            }
        } else if let Some(&origin) = self.facts.idents.get(&key) {
            self.reference(origin);
        }
    }

    fn visit_expr_assign(&mut self, assign: &syn::ExprAssign) {
        self.record_write(&assign.left, &assign.right);
        visit::visit_expr_assign(self, assign);
    }

    fn visit_expr_binary(&mut self, binary: &syn::ExprBinary) {
        if crate::schedule::is_assign_op(binary.op) {
            self.record_write(&binary.left, &binary.right);
        }
        visit::visit_expr_binary(self, binary);
    }

    fn visit_expr_reference(&mut self, reference: &syn::ExprReference) {
        if reference.mutability.is_some() {
            IdentOrigins {
                facts: self.facts,
                out: &mut self.tainted,
            }
            .visit_expr(&reference.expr);
        }
        visit::visit_expr_reference(self, reference);
    }

    fn visit_macro(&mut self, mac: &syn::Macro) {
        if let Some(first) = mac.path.segments.first()
            && let Some(records) = self.facts.original_macros.get(&core::ptr::from_ref(first))
        {
            for resolution in records {
                if let Some(origin) = resolution.origin {
                    self.reference(origin);
                }
            }
        }
        visit::visit_macro(self, mac);
    }
}

fn group_order(
    items: &[&syn::Item],
    facts: &ReferenceFacts,
    roots: &RootContext,
    table: Option<&crate::helpers::HelperTable>,
) -> Option<Vec<Partition>> {
    if items.len() == 1 && !matches!(items[0], syn::Item::Impl(block) if block.items.len() > 1) {
        return Some(vec![Partition {
            indices: vec![0],
            members: Vec::new(),
            label: Lbl::Other("fixed".into(), Vec::new()),
        }]);
    }
    if !Budget::accepts(items) {
        return None;
    }
    let mut group = BTreeMap::new();
    let mut next = 0;
    for item in items {
        if let Some(origin) = declaration(item, facts) {
            if group.insert(identity(origin), next).is_some() {
                return None;
            }
            next += 1;
        }
    }
    let mut partitions = partitions(items, facts, &group, table)?;
    let mut budget = Budget {
        nodes: roots.budget.nodes,
        deepest: roots.budget.deepest,
        ..Budget::default()
    };
    for partition in &partitions {
        budget.label(&partition.label, 0);
    }
    if !budget.within() {
        return None;
    }
    let original_to_partition: BTreeMap<_, _> = partitions
        .iter()
        .enumerate()
        .flat_map(|(slot, partition)| partition.indices.iter().map(move |&index| (index, slot)))
        .collect();
    let mut remapping = vec![0; group.len() + roots.definitions.len()];
    for (index, item) in items.iter().enumerate() {
        if let Some(origin) = declaration(item, facts) {
            remapping[group[&identity(origin)]] = original_to_partition[&index];
        }
    }
    for index in 0..roots.definitions.len() {
        remapping[group.len() + index] = partitions.len() + index;
    }
    let mut root = roots.root.clone();
    remap(&mut root, &remapping);
    for partition in &mut partitions {
        remap(&mut partition.label, &remapping);
    }
    let mut labels: Vec<_> = partitions
        .iter()
        .map(|partition| partition.label.clone())
        .collect();
    let count = labels.len();
    for definition in &roots.definitions {
        let mut definition = definition.clone();
        remap(&mut definition, &remapping);
        labels.push(definition);
    }
    rank_payloads(&mut labels, &mut root);
    let order = crate::schedule::item_order(&labels, &root)?;
    let mut partitions: Vec<_> = partitions.into_iter().map(Some).collect();
    Some(
        order
            .into_iter()
            .filter(|&slot| slot < count)
            .map(|slot| partitions[slot].take().expect("a distinct item slot"))
            .collect(),
    )
}

struct Normalize<'a> {
    facts: ReferenceFacts,
    helpers: &'a mut crate::helpers::HelperSyntax,
    table: Option<&'a crate::helpers::HelperTable>,
    blocked: bool,
}

impl Normalize<'_> {
    fn scope_blocked(&self, items: &[&syn::Item]) -> bool {
        let mut check = Check {
            facts: &self.facts,
            blocked: self.blocked,
            public: false,
        };
        for item in items {
            check.visit_item(item);
        }
        check.blocked
    }

    fn revisit_region<S: RegionSource>(&mut self, region: &mut [S]) {
        for source in region.iter_mut() {
            source.visit(self);
            source.reprove(&self.facts);
        }
    }

    fn region_plan<'s, S: RegionSource + 's>(
        &self,
        refs: &[&'s syn::Item],
        context: impl Iterator<Item = &'s S>,
    ) -> Option<Vec<Partition>> {
        let mut observations = Observations::new(refs, &self.facts, self.table);
        for source in context {
            source.observe(&mut observations);
        }
        let roots = observations.finish()?;
        group_order(refs, &self.facts, &roots, self.table)
    }

    fn schedule_region<S: RegionSource>(
        &mut self,
        original: &mut Vec<S>,
        start: usize,
        end: usize,
        order: Vec<Partition>,
    ) -> usize {
        for source in original.iter() {
            source.clear(&mut self.facts);
        }
        let old = original
            .drain(start..end)
            .map(RegionSource::into_original)
            .collect();
        let scheduled = self.apply_region(old, order);
        let count = scheduled.len();
        original.splice(
            start..start,
            scheduled.into_iter().map(RegionSource::from_original),
        );
        for source in original.iter_mut() {
            source.attach(&mut self.facts);
        }
        start + count
    }

    fn restore<S: RegionSource>(&mut self, sources: Vec<S>) -> Vec<S::Restored> {
        for source in &sources {
            source.clear(&mut self.facts);
        }
        let (mut values, proofs): (Vec<S::Restored>, Vec<Snapshot>) =
            sources.into_iter().map(RegionSource::split).unzip();
        for (value, proof) in values.iter_mut().zip(proofs) {
            S::attach_restored(value, &proof, &mut self.facts);
        }
        values
    }

    fn reorder(&mut self, items: &mut Vec<syn::Item>) {
        let refs: Vec<_> = items.iter().collect();
        if self.scope_blocked(&refs) {
            return;
        }
        let proofs: Vec<_> = items
            .iter()
            .map(|item| Snapshot::collect(item, &self.facts))
            .collect();
        for item in items.iter() {
            Clear(&mut self.facts).visit_item(item);
        }
        let mut original: Vec<_> = core::mem::take(items)
            .into_iter()
            .zip(proofs)
            .map(|(item, proof)| OriginalItem { item, proof })
            .collect();
        self.order_regions(&mut original);
        *items = self.restore(original);
    }

    /// Reorder every admitted region of `original` that has a plan, and normalize every
    /// source, a region holding a public item in its source order.
    fn order_regions<S: RegionSource>(&mut self, original: &mut Vec<S>) {
        for source in original.iter_mut() {
            source.attach(&mut self.facts);
        }
        let mut start = 0;
        while start < original.len() {
            if !original[start].admitted() {
                original[start].visit(self);
                original[start].reprove(&self.facts);
                start += 1;
                continue;
            }
            let end = start
                + original[start..]
                    .iter()
                    .take_while(|source| source.group_member())
                    .count();
            let refs: Vec<_> = original[start..end]
                .iter()
                .filter(|source| source.admitted())
                .map(RegionSource::item)
                .collect();
            let mut check = Check {
                facts: &self.facts,
                blocked: false,
                public: false,
            };
            for item in &refs {
                check.visit_item(item);
            }
            if check.public {
                self.revisit_region(&mut original[start..end]);
                start = end;
                continue;
            }
            let context = original[..start]
                .iter()
                .chain(original[end..].iter())
                .chain(
                    original[start..end]
                        .iter()
                        .filter(|source| !source.admitted()),
                );
            let Some(order) = self.region_plan(&refs, context) else {
                start = end;
                continue;
            };
            let unchanged = order.len() == refs.len()
                && order.iter().enumerate().all(|(index, partition)| {
                    partition.indices == [index] && partition.members.is_empty()
                });
            self.revisit_region(&mut original[start..end]);
            if unchanged {
                start = end;
                continue;
            }
            start = self.schedule_region(original, start, end, order);
        }
    }

    fn apply_region(
        &mut self,
        mut region: Vec<Option<OriginalItem>>,
        order: Vec<Partition>,
    ) -> Vec<OriginalItem> {
        let slots: Vec<_> = region
            .iter()
            .enumerate()
            .filter_map(|(slot, source)| {
                source
                    .as_ref()
                    .filter(|source| admitted(&source.item))
                    .map(|_| slot)
            })
            .collect();
        let old = slots.iter().map(|&slot| region[slot].take()).collect();
        let mut scheduled = self.apply(old, order).into_iter();
        for slot in slots {
            region[slot] = scheduled.next();
        }
        region.into_iter().flatten().collect()
    }

    fn attach_sources(&mut self, old: &mut [Option<OriginalItem>], order: &[Partition]) {
        for partition in order {
            if partition.members.is_empty() {
                continue;
            }
            for &index in &partition.indices {
                let source = old[index].as_mut().expect("an inherent source");
                source.proof.attach_item(&mut source.item, &mut self.facts);
            }
        }
    }

    fn source_plan(
        &self,
        source: &syn::ItemImpl,
        destination_ports: &[(ResolvedOrigin, Ident)],
    ) -> RebindPlan {
        let source_ports = generic_ports(source, &self.facts);
        let aliases: BTreeMap<_, _> = source_ports
            .into_iter()
            .zip(destination_ports)
            .filter(|(source, target)| identity(source.0) != identity(target.0))
            .map(|(source, target)| (identity(source.0), target.clone()))
            .collect();
        let proofs: Vec<_> = source
            .items
            .iter()
            .map(|member| Snapshot::member(member, &self.facts))
            .collect();
        RebindPlan { aliases, proofs }
    }

    fn rebind_source(
        &mut self,
        source: &mut OriginalItem,
        mut plan: RebindPlan,
    ) -> Vec<Option<(syn::ImplItem, Snapshot)>> {
        let syn::Item::Impl(block) = &mut source.item else {
            unreachable!()
        };
        if !plan.aliases.is_empty() {
            for (member, proof) in block.items.iter_mut().zip(&mut plan.proofs) {
                Rebind {
                    records: proof.idents.iter_mut(),
                    macros: proof.macros.iter_mut(),
                    aliases: &plan.aliases,
                }
                .visit_impl_item_mut(member);
                for record in &mut proof.paths {
                    if let Some(resolution) = &mut record.resolution
                        && let Some(origin) = resolution.origin
                        && let Some((target, _)) = plan.aliases.get(&identity(origin))
                    {
                        resolution.origin = Some(ResolvedOrigin {
                            ns: origin.ns,
                            ..*target
                        });
                    }
                }
            }
        }
        normalize_owner(block, &mut self.facts);
        let members = core::mem::take(&mut block.items);
        source.proof = Snapshot::header(block, &self.facts);
        members.into_iter().zip(plan.proofs).map(Some).collect()
    }

    fn apply(
        &mut self,
        mut old: Vec<Option<OriginalItem>>,
        order: Vec<Partition>,
    ) -> Vec<OriginalItem> {
        self.attach_sources(&mut old, &order);
        let mut plans: Vec<Option<Vec<RebindPlan>>> = Vec::new();
        for partition in &order {
            if partition.members.is_empty() {
                plans.push(None);
                continue;
            }
            let destination_ports = {
                let item = &old[partition.indices[0]]
                    .as_ref()
                    .expect("an inherent destination")
                    .item;
                let syn::Item::Impl(block) = item else {
                    unreachable!()
                };
                generic_ports(block, &self.facts)
            };
            let mut source_plans = Vec::new();
            for &index in &partition.indices {
                let item = &old[index].as_ref().expect("an inherent source").item;
                let syn::Item::Impl(block) = item else {
                    unreachable!()
                };
                source_plans.push(self.source_plan(block, &destination_ports));
            }
            plans.push(Some(source_plans));
        }
        let mut result = Vec::with_capacity(order.len());
        for (partition, source_plans) in order.into_iter().zip(plans) {
            let destination = partition.indices[0];
            let Some(source_plans) = source_plans else {
                result.push(old[destination].take().expect("an unmoved item"));
                continue;
            };
            let mut member_slots = BTreeMap::new();
            for (index, plan) in partition.indices.iter().copied().zip(source_plans) {
                let source = old[index].as_mut().expect("an inherent source");
                member_slots.insert(index, self.rebind_source(source, plan));
            }
            let mut source = old[destination].take().expect("an inherent destination");
            let syn::Item::Impl(block) = &mut source.item else {
                unreachable!()
            };
            block.items.reserve(partition.members.len());
            for (index, member) in partition.members {
                let (item, proof) = member_slots.get_mut(&index).expect("a source block")[member]
                    .take()
                    .expect("an unmoved member");
                block.items.push(item);
                source.proof.append(proof);
            }
            result.push(source);
        }
        result
    }
}

impl VisitMut for Normalize<'_> {
    fn visit_file_mut(&mut self, file: &mut syn::File) {
        self.blocked |= crate::context::live_attr(&file.attrs);
        let refs: Vec<_> = file.items.iter().collect();
        let blocked = self.blocked;
        self.blocked |= self.scope_blocked(&refs);
        self.reorder(&mut file.items);
        self.blocked = blocked;
    }
    fn visit_item_mod_mut(&mut self, item: &mut syn::ItemMod) {
        let blocked = self.blocked;
        self.blocked |= crate::context::live_attr(&item.attrs);
        if let Some((_, items)) = &mut item.content {
            let refs: Vec<_> = items.iter().collect();
            self.blocked |= self.scope_blocked(&refs);
            self.reorder(items);
        }
        self.blocked = blocked;
    }
    fn visit_block_mut(&mut self, block: &mut syn::Block) {
        let blocked = self.blocked;
        let refs: Vec<_> = block
            .stmts
            .iter()
            .filter_map(|stmt| match stmt {
                syn::Stmt::Item(item) => Some(item),
                _ => None,
            })
            .collect();
        self.blocked |= self.scope_blocked(&refs);
        if !self.blocked {
            self.normalize_block(block);
        }
        self.blocked = blocked;
    }
}

impl Normalize<'_> {
    /// Reorder the item regions of an unblocked block, or walk it when it holds no items.
    fn normalize_block(&mut self, block: &mut syn::Block) {
        if !block
            .stmts
            .iter()
            .any(|stmt| matches!(stmt, syn::Stmt::Item(_)))
        {
            visit_mut::visit_block_mut(self, block);
            return;
        }
        let handles = self.helpers.take_block(block);
        let proofs: Vec<_> = block
            .stmts
            .iter()
            .map(|stmt| Snapshot::statement(stmt, &self.facts))
            .collect();
        for stmt in &block.stmts {
            Clear(&mut self.facts).visit_stmt(stmt);
        }
        let mut original: Vec<_> = core::mem::take(&mut block.stmts)
            .into_iter()
            .zip(proofs)
            .map(|(stmt, proof)| OriginalStmt { stmt, proof })
            .collect();
        self.order_regions(&mut original);
        block.stmts = self.restore(original);
        self.helpers.put_block(block, handles);
    }
}

struct Calls<'a> {
    facts: &'a mut ReferenceFacts,
    helpers: &'a crate::helpers::HelperSyntax,
    table: &'a crate::helpers::HelperTable,
}

impl Visit<'_> for Calls<'_> {
    fn visit_expr_call(&mut self, call: &syn::ExprCall) {
        if let syn::Expr::Path(path) = call.func.as_ref()
            && path.qself.is_none()
            && path.path.leading_colon.is_none()
            && path
                .path
                .segments
                .iter()
                .all(|segment| segment.arguments.is_none())
            && let Some(scope) = self.helpers.call_scope(&path.path)
            && self.table.scope_known(scope)
            && let Some(first) = path.path.segments.first()
        {
            let helper = if path.path.segments.len() == 1 {
                self.table
                    .scope(scope)
                    .and_then(|frame| {
                        frame.lookup(Ns::Value, &crate::scope::ident_name(&first.ident))
                    })
                    .and_then(|binding| binding.helper)
            } else {
                self.table.resolve_in_scope(scope, &path.path)
            };
            if let Some(helper) = helper
                && self.table.get(helper).is_some()
            {
                self.facts
                    .original_calls
                    .insert(core::ptr::from_ref(first), helper);
                self.facts
                    .original_call_scopes
                    .insert(core::ptr::from_ref(first), scope);
            }
        }
        visit::visit_expr_call(self, call);
    }
}

pub(crate) fn normalize(
    file: &mut syn::File,
    options: &crate::alpha::Options<'_>,
    helpers: &mut crate::helpers::HelperSyntax,
    table: &crate::helpers::HelperTable,
) {
    if options.observed || options.helper_blocked {
        return;
    }
    let mut facts = crate::alpha::analyze_file(
        file,
        crate::alpha::Options {
            seed: options.seed,
            generics: options.generics,
            self_canon: options.self_canon.clone(),
            prim_fallback: options.prim_fallback,
            observed: options.observed,
            helper_table: Some(table),
            helper_module: options.helper_module,
            helper_blocked: options.helper_blocked,
            helper_syntax: None,
        },
    );

    Calls {
        facts: &mut facts,
        helpers,
        table,
    }
    .visit_file(file);
    Normalize {
        facts,
        helpers,
        table: Some(table),
        blocked: false,
    }
    .visit_file_mut(file);
}
