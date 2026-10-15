//! Shared alpha-renaming for whole files, lexical contexts, and seeded
//! views, with the dependency scheduler embedded in its block visits.

use alloc::borrow::Cow;
use alloc::boxed::Box;
use alloc::collections::BTreeMap;
use alloc::collections::BTreeSet;
use alloc::format;
use alloc::string::String;
use alloc::string::ToString;
use alloc::vec;
use alloc::vec::Vec;

use proc_macro2::{Ident, Span};
use quote::ToTokens;
use syn::parse::discouraged::Speculative as _;
use syn::visit::Visit;
use syn::visit_mut::VisitMut;

use crate::reference::{OwnerRoute, ReferenceFacts, ReferenceKind, Resolution, ResolvedOrigin};
use crate::schedule;
use crate::scope::{
    Binding, BindingId, Domain, FirstSeg, Frame, FreeKey, Inherited, Ns, Origin, PrimTy, Target,
    TargetSeg, unraw,
};

/// The normalization options of a file pass.
pub(crate) struct Options<'a> {
    /// The seed frame of the view's module, when one is indexed.
    pub(crate) seed: Option<&'a Frame>,
    /// The frame of the enclosing impl or trait's generics, above the seed
    /// and below the function's own binders.
    pub(crate) generics: Option<&'a Frame>,
    /// The canon the view's `Self` names, for a method or trait view.
    pub(crate) self_canon: Option<String>,
    /// Unresolved type names may still spell a primitive, when no glob
    /// import can shadow them.
    pub(crate) prim_fallback: bool,
    /// A live observer attribute on the declaration or an enclosing item.
    pub(crate) observed: bool,
    /// The source context's helper summary table, when one is indexed.
    pub(crate) helper_table: Option<&'a crate::helpers::HelperTable>,
    pub(crate) helper_module: &'a [String],
    pub(crate) helper_blocked: bool,
    pub(crate) helper_syntax: Option<crate::helpers::HelperSyntax>,
}

/// A resolved name reference, its identity only, the canon resolves on render.
#[derive(Clone, Copy)]
struct Resolved {
    domain: Domain,
    ns: Ns,
    id: BindingId,
    origin: Origin,
    prim: Option<PrimTy>,
    /// The declared position of a function parameter, zero otherwise.
    port: usize,
    target: Option<usize>,
    helper: Option<crate::scope::HelperId>,
}

#[derive(Clone, Copy)]
struct OpaqueBinding<'a> {
    domain: Domain,
    ns: Ns,
    binding: &'a Binding,
}

/// The namespace letter of a canonical name.
const fn ns_letter(ns: Ns) -> char {
    match ns {
        Ns::Value => 'v',
        Ns::Type => 't',
        Ns::Lifetime => 'l',
        Ns::Label => 'b',
        Ns::Macro => 'm',
    }
}

/// A block-local binder's canonical name in a context pass.
fn local_name(ns: Ns, number: usize) -> String {
    format!("_l{}{}", ns_letter(ns), number)
}

/// An inherited reference's first-reference canonical name.
fn inherited_name(ns: Ns, number: usize) -> String {
    format!("_i{}{}", ns_letter(ns), number)
}

/// Point `ident` at `canon`, keeping its span.
fn rename(ident: &mut Ident, canon: &str) {
    *ident = Ident::new(canon, ident.span());
}

/// Walk a written `use` prefix, resolving its first segment and copying the rest.
fn resolve_prefix<'a>(
    first_segment: &dyn Fn(&str) -> FirstSeg<'a>,
    rooted: bool,
    prefix: &[String],
) -> (bool, Vec<TargetSeg>) {
    let mut segments = Vec::with_capacity(prefix.len());
    let mut rooted = rooted;
    if let Some(first) = prefix.first() {
        match first_segment(first) {
            FirstSeg::Alias(target) => {
                rooted = target.rooted;
                segments.extend(target.segments.iter().cloned());
            }
            FirstSeg::Module(domain, id) => {
                segments.push(TargetSeg {
                    name: first.clone(),
                    origin: Some((domain, id, Ns::Type)),
                });
            }
            FirstSeg::Free => {
                segments.push(TargetSeg {
                    name: first.clone(),
                    origin: None,
                });
            }
        }
    }
    for segment in prefix.iter().skip(1) {
        segments.push(TargetSeg {
            name: segment.clone(),
            origin: None,
        });
    }
    (rooted, segments)
}

/// Resolve a nonempty `use` path while retaining opaque qualified suffixes.
pub(crate) fn resolve_target_path<'a>(
    resolve: &dyn Fn(&str) -> FirstSeg<'a>,
    rooted: bool,
    path: &[String],
) -> (bool, Vec<TargetSeg>) {
    if rooted {
        return (
            path.first()
                .is_some_and(|first| !matches!(resolve(first), FirstSeg::Free)),
            path.iter()
                .map(|name| TargetSeg {
                    name: name.clone(),
                    origin: None,
                })
                .collect(),
        );
    }
    let (leaf, prefix) = path.split_last().expect("a use target has a leaf");
    if prefix.is_empty() {
        return match resolve(leaf.as_str()) {
            FirstSeg::Alias(target) => (target.rooted, target.segments.clone()),
            FirstSeg::Module(domain, id) => (
                rooted,
                vec![TargetSeg {
                    name: leaf.clone(),
                    origin: Some((domain, id, Ns::Type)),
                }],
            ),
            FirstSeg::Free => (
                rooted,
                vec![TargetSeg {
                    name: leaf.clone(),
                    origin: None,
                }],
            ),
        };
    }
    let (rooted, mut segments) = resolve_prefix(resolve, rooted, prefix);
    segments.push(TargetSeg {
        name: leaf.clone(),
        origin: None,
    });
    (rooted, segments)
}

/// The empty module-frame table of a whole-file pass.
static EMPTY_MOD_FRAMES: BTreeMap<BindingId, Frame> = BTreeMap::new();

/// The canonicalizer, one mutable traversal per pass.
#[expect(
    clippy::struct_excessive_bools,
    reason = "Normalization mode, syntax positions and token opacity are independent"
)]
pub(crate) struct Renamer<'env> {
    /// The scope stack of the pass, the base frame first.
    frames: Vec<Frame>,
    /// The item frames of local modules, keyed by the module's binding id.
    mod_frames: BTreeMap<BindingId, Frame>,
    /// The written targets of the pass's own `use` aliases.
    targets: Vec<Target>,
    /// The next binding id to hand out, above any inherited id.
    next_id: usize,
    /// The positional counter for local canonical names.
    counter: usize,
    /// True when normalizing one candidate against an inherited environment.
    context: bool,
    /// The inherited environment of a candidate pass.
    inherited: Inherited<'env>,
    /// The seed frame of a seeded view, outermost inherited.
    seed: Option<&'env Frame>,
    /// The enclosing generics frame of a seeded view.
    generics: Option<&'env Frame>,
    /// The `Self` frame of a seeded method or trait view.
    self_frame: Option<Frame>,
    /// First-reference numbers of inherited binding ids.
    inherited_numbers: BTreeMap<BindingId, usize>,
    /// The local canon of every local binding id.
    local_canons: BTreeMap<BindingId, String>,
    /// The self type of the enclosing impl, expanded in type position.
    self_ty: Option<syn::Type>,
    /// The resolutions of the impl self type, restored onto its clones.
    self_ty_resolutions: Vec<TypeResolution>,
    /// The terminal owner the enclosing impl's `Self` names, analysis only.
    self_owner: Option<ResolvedOrigin>,
    /// A type position, where a path reads the type namespace only.
    in_type: bool,
    /// A bare `<T>` was just visited, so the next path names associated items of `T`.
    after_bare_qself: bool,
    /// Raw-involved identifiers in macro input keep their token spelling.
    opaque_tokens: bool,
    macro_supported: bool,
    /// A passing doctest: the scheduling analysis runs on its function blocks.
    compiles: bool,
    /// Unresolved type names may still spell a primitive, when no glob
    /// import can shadow them.
    prim_fallback: bool,
    /// A live observer attribute froze the declarations being visited.
    observed: bool,
    constants_observed: bool,
    constants_prepared: bool,
    regions_active: bool,
    /// The block nesting level of the traversal.
    depth: usize,
    /// The scheduler scope state of every function open in the traversal.
    pub(crate) scopes: Vec<schedule::ScopeState>,
    /// The scheduler facts of every analyzed block open in the traversal.
    block_facts: Vec<schedule::BlockFacts>,
    /// The resolutions collected for the macro arguments being rewritten.
    macro_origins: Option<Vec<Resolution>>,
    /// The reference provenance and block facts the pass recorded.
    pub(crate) facts: ReferenceFacts,
    pub(crate) helper_table: Cow<'env, crate::helpers::HelperTable>,
    helper_module: Vec<String>,
    helper_canons: BTreeMap<crate::scope::HelperId, FreeKey>,
    pub(crate) helper_syntax: crate::helpers::HelperSyntax,
    pub(crate) helper_blocked: bool,
    /// Original-syntax resolution without rewriting.
    analysis: bool,
    receiver: bool,
}

impl<'env> Renamer<'env> {
    fn base(context: bool, inherited: Inherited<'env>) -> Self {
        Self {
            frames: vec![Frame::default()],
            mod_frames: BTreeMap::new(),
            targets: Vec::new(),
            next_id: 0,
            counter: 0,
            context,
            inherited,
            seed: None,
            generics: None,
            self_frame: None,
            inherited_numbers: BTreeMap::new(),
            local_canons: BTreeMap::new(),
            self_ty: None,
            self_ty_resolutions: Vec::new(),
            self_owner: None,
            in_type: false,
            after_bare_qself: false,
            opaque_tokens: false,
            macro_supported: true,
            compiles: false,
            prim_fallback: false,
            observed: false,
            constants_observed: false,
            constants_prepared: false,
            regions_active: true,
            depth: 0,
            scopes: Vec::new(),
            block_facts: Vec::new(),
            macro_origins: None,
            facts: ReferenceFacts::default(),
            helper_table: Cow::Owned(crate::helpers::HelperTable::default()),
            helper_module: Vec::new(),
            helper_canons: BTreeMap::new(),
            helper_syntax: crate::helpers::HelperSyntax::default(),
            helper_blocked: false,
            analysis: false,
            receiver: false,
        }
    }

    /// The whole-file pass, no inherited environment.
    fn new() -> Self {
        Self::base(
            false,
            Inherited {
                frames: &[],
                mod_frames: &EMPTY_MOD_FRAMES,
                targets: &[],
            },
        )
    }

    /// One candidate pass against the inherited environment.
    pub(crate) fn contextual(inherited: Inherited<'env>) -> Self {
        Self::base(true, inherited)
    }

    /// A file pass with the optional seeded environment of a function view.
    fn seeded(options: Options<'env>, compiles: bool) -> Self {
        let mut renamer = Self::new();
        renamer.compiles = compiles;
        renamer.prim_fallback = options.prim_fallback;
        renamer.observed = options.observed;
        renamer.seed = options.seed;
        renamer.generics = options.generics;
        if let Some(table) = options.helper_table {
            renamer.helper_table = Cow::Borrowed(table);
        }
        renamer.helper_module = options.helper_module.to_vec();
        renamer.helper_blocked = options.helper_blocked;
        renamer.helper_syntax = options.helper_syntax.unwrap_or_default();
        if let Some(canon) = options.self_canon {
            let id = BindingId(
                renamer
                    .seed
                    .into_iter()
                    .chain(renamer.generics)
                    .map(Frame::next_id)
                    .max()
                    .unwrap_or(0),
            );
            let mut frame = Frame::default();
            frame.bind(
                Ns::Type,
                "Self",
                Binding {
                    id,
                    canon,
                    origin: Origin::SelfTy,
                    prim: None,
                    port: 0,
                    value_port: None,
                    target: None,
                    raw: false,
                    helper: None,
                },
            );
            renamer.self_frame = Some(frame);
        }
        renamer
    }

    pub(crate) fn scratch_ref(frame: &Frame, prim_fallback: bool) -> Renamer<'_> {
        let mut renamer = Renamer::contextual(Inherited {
            frames: core::slice::from_ref(frame),
            mod_frames: &EMPTY_MOD_FRAMES,
            targets: &[],
        });
        renamer.prim_fallback = prim_fallback;
        renamer
    }

    /// Push a scope frame.
    fn push(&mut self) {
        self.frames.push(Frame::default());
    }

    /// Pop a scope frame.
    fn pop(&mut self) {
        self.frames.pop();
    }

    /// Bind `name` to `binding` in the current frame.
    fn top_bind(&mut self, ns: Ns, name: &str, binding: Binding) {
        self.frames
            .last_mut()
            .expect("a scope frame")
            .bind(ns, name, binding);
    }

    /// Allocate the next binding id.
    fn alloc_id(&mut self) -> BindingId {
        let id = BindingId(self.next_id);
        self.next_id += 1;
        id
    }

    pub(crate) fn helper_closure_sum(&self, roots: &BTreeSet<crate::scope::HelperId>) -> usize {
        self.helper_table.closure_nodes(roots)
    }

    pub(crate) fn helper_id(
        &self,
        env: &BTreeMap<String, schedule::Value>,
        path: &syn::Path,
    ) -> Option<crate::scope::HelperId> {
        if !self.helpers_allowed()
            || path.leading_colon.is_some()
            || path.segments.len() > schedule::MAX_BLOCK_DEPTH
            || path
                .segments
                .iter()
                .any(|segment| !segment.arguments.is_none())
        {
            return None;
        }
        if let Some((helper, scope)) = self.helper_syntax.call_proof(path)
            && self.helper_table.scope_known(scope)
        {
            return Some(helper);
        }
        let scope = self.helper_path_scope(path)?;
        if !self.helper_table.scope_known(scope) {
            return None;
        }
        let first = path.segments.first()?;
        let name = crate::scope::ident_name(&first.ident);
        if path.segments.len() == 1 {
            return env.get(&name).and_then(|value| value.helper);
        }
        self.helper_table.resolve_in_scope(scope, path)
    }

    fn helper_path_scope(&self, path: &syn::Path) -> Option<crate::helpers::ScopeId> {
        self.helper_syntax.call_scope(path)
    }

    fn helpers_allowed(&self) -> bool {
        !self.helper_blocked
            && !self.glob_uncertain()
            && !self.cfg_blocked()
            && !self.scopes.last().is_some_and(|scope| scope.frozen)
    }

    fn helper_canon(&self, helper: crate::scope::HelperId) -> Option<&str> {
        match self.helper_canons.get(&helper) {
            Some(FreeKey::Resolved(canon)) => Some(canon),
            _ => self
                .helper_syntax
                .proof_canon(helper)
                .or_else(|| self.helper_table.inherited_canon(helper)),
        }
    }

    pub(crate) fn helper_identity(&self, helper: crate::scope::HelperId) -> Option<FreeKey> {
        self.helper_canon(helper)
            .map(|canon| FreeKey::Resolved(canon.to_string()))
    }

    pub(crate) fn helper_declaration(&self, block: &syn::Block) -> Option<crate::scope::HelperId> {
        self.helper_syntax.declaration(block)
    }

    pub(crate) fn helper_block_scope(&self, block: &syn::Block) -> Option<crate::helpers::ScopeId> {
        self.helper_syntax.scope(block)
    }

    pub(crate) fn helper_import_env(
        &self,
        block: &syn::Block,
        env: &mut BTreeMap<String, schedule::Value>,
    ) {
        let Some(scope) = self.helper_block_scope(block) else {
            return;
        };
        let frame = self
            .helper_table
            .scope(scope)
            .expect("indexed helper scope");
        for stmt in &block.stmts {
            if let syn::Stmt::Item(item) = stmt {
                if let syn::Item::Use(import) = item {
                    if !crate::helpers::use_tree_known(&import.tree, 0) {
                        for value in env.values_mut() {
                            value.helper = None;
                        }
                        continue;
                    }
                    crate::helpers::walk_imports(
                        &import.tree,
                        &mut Vec::new(),
                        0,
                        &mut |_, alias| {
                            let Some(binding) = frame.lookup(Ns::Value, alias) else {
                                return;
                            };
                            if binding.origin != Origin::Use {
                                return;
                            }
                            let helper = binding
                                .helper
                                .or_else(|| self.helper_table.by_canon(&binding.canon));
                            let key = helper
                                .and_then(|helper| self.helper_identity(helper))
                                .unwrap_or_else(|| FreeKey::Unresolved(alias.to_string()));
                            env.insert(
                                alias.to_string(),
                                schedule::Value {
                                    port: schedule::Port::Free(key),
                                    prim: binding.prim,
                                    helper,
                                },
                            );
                        },
                    );
                } else if let Some((namespace, ident)) = item_binding(item)
                    && (namespace == Ns::Value
                        || matches!(item, syn::Item::Struct(item) if !matches!(item.fields, syn::Fields::Named(_))))
                {
                    let name = crate::scope::ident_name(ident);
                    let helper = match item {
                        syn::Item::Fn(function) => self.helper_declaration(&function.block),
                        _ => None,
                    };
                    let key = helper
                        .and_then(|helper| self.helper_identity(helper))
                        .unwrap_or_else(|| FreeKey::Unresolved(name.clone()));
                    env.insert(
                        name,
                        schedule::Value {
                            port: schedule::Port::Free(key),
                            prim: None,
                            helper,
                        },
                    );
                }
            }
        }
    }

    /// The next local canonical name, positional in the traversal.
    fn local_canon(&mut self, ns: Ns) -> String {
        let canon = if self.context {
            local_name(ns, self.counter)
        } else {
            format!("_canon_{}", self.counter)
        };
        self.counter += 1;
        canon
    }

    /// Assign the producer's canonical position before the permutation
    /// moves its declaration.
    fn local_canon_offset(&mut self, ns: Ns, offset: isize) -> String {
        let position = self
            .counter
            .checked_add_signed(offset)
            .expect("a producer permutation preserves its binding range");
        let canon = if self.context {
            local_name(ns, position)
        } else {
            format!("_canon_{position}")
        };
        self.counter += 1;
        canon
    }

    /// Bind `name` in the current frame and return the new binding.
    fn bind(&mut self, ns: Ns, name: &str, origin: Origin) -> Binding {
        self.bind_proven(ns, name, origin, None)
    }

    /// Bind `name` with its proven scalar type.
    fn bind_proven(&mut self, ns: Ns, name: &str, origin: Origin, prim: Option<PrimTy>) -> Binding {
        self.bind_full(ns, name, origin, prim, 0, None)
    }

    /// Bind `name` in the current frame, consuming the scheduler's plan of
    /// the original pattern when one was recorded.
    fn bind_full(
        &mut self,
        ns: Ns,
        name: &str,
        origin: Origin,
        prim: Option<PrimTy>,
        port: usize,
        plan: Option<schedule::BindingPlan>,
    ) -> Binding {
        let (prim, offset, value_port) = plan.map_or((prim, 0, None), |plan| {
            (plan.prim, plan.offset, Some(plan.port))
        });
        let value_port = if origin == Origin::Closure {
            Some(schedule::Port::Closure(self.counter))
        } else {
            value_port
        };
        let id = self.alloc_id();
        let canon = self.local_canon_offset(ns, offset);
        let binding = Binding {
            id,
            canon: canon.clone(),
            origin,
            prim,
            port,
            value_port,
            target: None,
            raw: false,
            helper: None,
        };
        self.local_canons.insert(id, canon);
        self.top_bind(ns, name, binding.clone());
        binding
    }

    /// Share one id and canon across the value and type namespaces.
    fn bind_value_and_type(
        &mut self,
        name: &str,
        target: Option<usize>,
        origin: Origin,
    ) -> Binding {
        let id = self.alloc_id();
        let canon = self.local_canon(Ns::Value);
        let binding = Binding {
            id,
            canon: canon.clone(),
            origin,
            prim: None,
            port: 0,
            value_port: None,
            target,
            raw: false,
            helper: None,
        };
        self.local_canons.insert(id, canon);
        self.top_bind(Ns::Value, name, binding.clone());
        self.top_bind(Ns::Type, name, binding.clone());
        if origin == Origin::Use {
            self.top_bind(Ns::Macro, name, binding.clone());
        }
        binding
    }

    /// Bind `ident` in the current frame and rename it.
    fn bind_ident(&mut self, ns: Ns, origin: Origin, ident: &mut Ident) {
        let name = ident.to_string();
        let binding = self.bind(ns, &name, origin);
        if self.analysis {
            self.record_ident(ident, ns, binding.id, true);
        } else {
            rename(ident, &binding.canon);
        }
    }

    /// Rename a prebound item declaration.
    fn rename_binder(&mut self, ns: Ns, ident: &mut Ident) -> BindingId {
        let name = ident.to_string();
        let binding = self
            .frames
            .last()
            .and_then(|frame| frame.lookup(ns, &name))
            .expect("prebound item binder");
        let id = binding.id;
        if self.analysis {
            self.record_ident(ident, ns, id, true);
        } else {
            rename(ident, &binding.canon);
        }
        id
    }

    /// The canonical name a resolved reference renders.
    fn render(&mut self, resolved: &Resolved) -> String {
        match resolved.domain {
            Domain::Local => self
                .local_canons
                .get(&resolved.id)
                .expect("a local binding")
                .clone(),
            Domain::Inherited => {
                if self.context {
                    inherited_name(resolved.ns, self.inherited_number(resolved.id))
                } else {
                    self.inherited_binding(resolved.id).canon.clone()
                }
            }
        }
    }

    /// The inherited binding selected by its identity.
    fn inherited_binding(&self, id: BindingId) -> &Binding {
        self.self_frame
            .iter()
            .chain(self.generics)
            .chain(self.seed)
            .chain(self.inherited.frames.iter())
            .flat_map(|frame| {
                [Ns::Value, Ns::Type, Ns::Lifetime, Ns::Label, Ns::Macro]
                    .into_iter()
                    .flat_map(move |ns| frame.bindings(ns).values())
            })
            .find(|binding| binding.id == id)
            .expect("an inherited binding resolved by lookup")
    }

    /// The first-reference number of an inherited binding id.
    fn inherited_number(&mut self, id: BindingId) -> usize {
        let table = &mut self.inherited_numbers;
        if let Some(number) = table.get(&id) {
            return *number;
        }
        let number = table.len();
        table.insert(id, number);
        number
    }

    fn resolved(domain: Domain, ns: Ns, binding: &Binding) -> Resolved {
        Resolved {
            domain,
            ns,
            id: binding.id,
            origin: binding.origin,
            prim: binding.prim,
            port: binding.port,
            target: binding.target,
            helper: binding.helper,
        }
    }

    /// Nearest-binding lookup across the local and inherited frames,
    /// namespace priority first. A raw name that resolves keeps its
    /// binding, an unresolved raw token stays opaque by default.
    fn lookup(&self, ns: &[Ns], name: &str) -> Option<Resolved> {
        for ns in ns {
            for frame in self.frames.iter().rev() {
                if let Some(binding) = frame.lookup(*ns, name) {
                    if self.opaque_tokens && (binding.raw || name.starts_with("r#")) {
                        return None;
                    }
                    return Some(Self::resolved(Domain::Local, *ns, binding));
                }
            }
            for frame in self
                .self_frame
                .iter()
                .chain(self.generics)
                .chain(self.seed)
                .chain(self.inherited.frames.iter())
            {
                if let Some(binding) = frame.lookup(*ns, name) {
                    if self.opaque_tokens && (binding.raw || name.starts_with("r#")) {
                        return None;
                    }
                    return Some(Self::resolved(Domain::Inherited, *ns, binding));
                }
            }
        }
        None
    }

    /// The provenance and proven scalar type of a retained reference.
    fn resolution(&self, ns: &[Ns], name: &str) -> Resolution {
        let Some(resolved) = self.lookup(ns, name) else {
            return Resolution {
                prim: (self.analysis && ns == [Ns::Type])
                    .then(|| self.name_prim(name))
                    .flatten(),
                ..Resolution::default()
            };
        };
        let origin = if let Some(index) = resolved.target {
            let target = self.target_of(resolved.domain, index);
            if target.rooted {
                return Resolution::default();
            }
            let Some((domain, id, ns)) = target.segments.first().and_then(|segment| segment.origin)
            else {
                return Resolution::default();
            };
            if domain == Domain::Inherited {
                return Resolution::default();
            }
            ResolvedOrigin { domain, ns, id }
        } else {
            ResolvedOrigin {
                domain: resolved.domain,
                ns: resolved.ns,
                id: resolved.id,
            }
        };
        Resolution {
            kind: match origin.domain {
                Domain::Local => ReferenceKind::Owned,
                Domain::Inherited => ReferenceKind::Inherited,
            },
            prim: resolved.prim,
            origin: Some(origin),
        }
    }

    fn record_ident(&mut self, ident: &Ident, ns: Ns, id: BindingId, declaration: bool) {
        let key = core::ptr::from_ref(ident);
        self.facts.idents.insert(
            key,
            ResolvedOrigin {
                domain: Domain::Local,
                ns,
                id,
            },
        );
        if declaration {
            self.facts.declarations.insert(key);
        }
    }

    /// Record one path's resolution, into the macro collector when nested.
    fn record_path(&mut self, path: &syn::Path, ns: &[Ns], root_ns: &[Ns]) {
        let resolution = path
            .segments
            .first()
            .filter(|_| path.leading_colon.is_none())
            .map(|head| self.head_resolution(ns, root_ns, path, head))
            .unwrap_or_default();
        if let Some(origins) = &mut self.macro_origins {
            origins.push(resolution);
        } else {
            self.facts
                .paths
                .insert(core::ptr::from_ref(path), resolution);
            if self.analysis {
                if let Some(first) = path.segments.first() {
                    let key = core::ptr::from_ref(first);
                    self.facts.original_paths.insert(key, resolution);
                    if resolution.origin.is_none()
                        && self.prim_fallback
                        && (resolution.prim.is_some()
                            || self.portable_first(root_ns, first)
                            || (first.ident == "self" && self.receiver)
                            || self.in_type)
                    {
                        self.facts.portable_paths.insert(key);
                    }
                }
                self.record_path_origins(ns, root_ns, path);
            }
        }
    }

    /// The path's head resolution: the enclosing impl's proven `Self`
    /// owner, a `self::` prefix reading its tail name in the current
    /// module, or the head name in the root namespace.
    fn head_resolution(
        &self,
        ns: &[Ns],
        root_ns: &[Ns],
        path: &syn::Path,
        head: &syn::PathSegment,
    ) -> Resolution {
        if head.ident == "Self"
            && let Some(owner) = self.self_owner
        {
            return Resolution {
                kind: match owner.domain {
                    Domain::Local => ReferenceKind::Owned,
                    Domain::Inherited => ReferenceKind::Inherited,
                },
                prim: None,
                origin: Some(owner),
            };
        }
        if head.ident == "self" && path.segments.len() > 1 {
            let tail = &path.segments[1];
            let head_ns = if path.segments.len() == 2 {
                ns
            } else {
                root_ns
            };
            return self.resolution(head_ns, &tail.ident.to_string());
        }
        self.resolution(root_ns, &head.ident.to_string())
    }

    /// Record one token's resolution, into the macro collector only.
    fn record_token(&mut self, ns: &[Ns], name: &str) {
        let resolution = self.resolution(ns, name);
        self.macro_origins
            .as_mut()
            .expect("a macro origin collector")
            .push(resolution);
    }

    /// Record the original ordered port of a parameter binding.
    fn record_parameter(&mut self, domain: Domain, id: BindingId, port: usize) {
        self.facts
            .original_parameters
            .insert((domain == Domain::Inherited, id), port);
    }

    /// Rename a bare reference through its nearest binder, if any.
    fn resolve(&mut self, ns: &[Ns], ident: &mut Ident) {
        if self.analysis {
            let name = ident.to_string();
            if let Some(resolved) = self.lookup(ns, &name)
                && resolved.origin == Origin::Parameter
            {
                self.record_parameter(resolved.domain, resolved.id, resolved.port);
            }
            if let Some(origin) = self.resolution(ns, &name).origin {
                self.facts.idents.insert(core::ptr::from_ref(ident), origin);
            }
            return;
        }
        if let Some(resolved) = self.lookup(ns, &ident.to_string())
            && resolved.target.is_none()
        {
            rename(ident, &self.render(&resolved));
        }
    }

    fn resolve_opaque(&mut self, ident: &mut Ident) -> bool {
        if self.analysis || !self.opaque_tokens || self.seed.is_none() {
            return false;
        }
        let name = ident.to_string();
        let candidates = [Ns::Value, Ns::Type, Ns::Lifetime, Ns::Label, Ns::Macro].map(|ns| {
            self.frames
                .iter()
                .rev()
                .find_map(|frame| frame.lookup(ns, &name))
                .map(|binding| OpaqueBinding {
                    domain: Domain::Local,
                    ns,
                    binding,
                })
                .or_else(|| {
                    self.self_frame
                        .iter()
                        .chain(self.generics)
                        .chain(self.seed)
                        .chain(self.inherited.frames.iter())
                        .find_map(|frame| frame.lookup(ns, &name))
                        .map(|binding| OpaqueBinding {
                            domain: Domain::Inherited,
                            ns,
                            binding,
                        })
                })
        });
        let Some(first) = candidates.iter().flatten().next().copied() else {
            return false;
        };
        if !candidates
            .iter()
            .flatten()
            .any(|candidate| candidate.domain == Domain::Inherited)
        {
            return false;
        }
        let raw = name.starts_with("r#")
            || candidates
                .iter()
                .flatten()
                .any(|candidate| candidate.binding.raw);
        if !raw
            && candidates.iter().flatten().all(|candidate| {
                candidate.domain == first.domain
                    && candidate.binding.id == first.binding.id
                    && candidate.binding.canon == first.binding.canon
            })
        {
            rename(ident, &first.binding.canon);
        } else {
            let mut canon = String::from("__dejadoc_macro_namespaces");
            if raw {
                crate::context::qualified_part(&mut canon, &name);
            }
            for candidate in candidates.into_iter().flatten() {
                canon.push('_');
                canon.push(ns_letter(candidate.ns));
                canon.push('_');
                let name = match candidate.domain {
                    Domain::Local => self
                        .local_canons
                        .get(&candidate.binding.id)
                        .expect("a local binding"),
                    Domain::Inherited => &candidate.binding.canon,
                };
                canon.push_str(name);
            }
            rename(ident, &canon);
        }
        self.macro_origins
            .as_mut()
            .expect("a macro origin collector")
            .push(Resolution {
                kind: ReferenceKind::Inherited,
                prim: None,
                origin: None,
            });
        true
    }

    /// The unproven type names a scope may still spell as primitives, no
    /// type binder captured the name.
    fn name_prim(&self, name: &str) -> Option<PrimTy> {
        if let Some(resolved) = self.lookup(&[Ns::Type], name) {
            return match resolved.origin {
                Origin::TypeAlias | Origin::Use => resolved.prim,
                _ => None,
            };
        }
        if self.prim_fallback {
            crate::scope::primitive_suffix(name)
        } else {
            None
        }
    }

    fn constants_allowed(&self) -> bool {
        self.compiles
            && !self.analysis
            && !self.observed
            && !self.constants_observed
            && self.macro_origins.is_none()
            && !self.scopes.iter().any(|scope| scope.frozen)
    }

    pub(crate) fn values_allowed(&self) -> bool {
        self.constants_allowed()
            && self.prim_fallback
            && !self.glob_uncertain()
            && !self.cfg_blocked()
    }

    /// Whether an open frame carries a glob import that may shadow a
    /// type name.
    pub(crate) fn glob_uncertain(&self) -> bool {
        self.frames.iter().any(|frame| frame.glob)
    }

    /// Whether the innermost function scope carries a `cfg` the source
    /// cannot settle.
    pub(crate) fn cfg_blocked(&self) -> bool {
        self.scopes.last().is_some_and(|scope| scope.cfg)
    }

    /// The facts of the most recently analyzed block, when any.
    pub(crate) fn last_block_facts(&self) -> Option<&schedule::BlockFacts> {
        self.block_facts.last()
    }

    /// The proven primitive type of a declared type, when the declaration
    /// establishes one through fully known local definitions.
    pub(crate) fn param_prim(&self, ty: &syn::Type) -> Option<PrimTy> {
        if !self.prim_fallback {
            return None;
        }
        match ty {
            syn::Type::Group(group) => self.param_prim(&group.elem),
            syn::Type::Path(path) if path.qself.is_none() => {
                if path
                    .path
                    .segments
                    .iter()
                    .any(|segment| !matches!(segment.arguments, syn::PathArguments::None))
                {
                    return None;
                }
                let names: Vec<String> = path
                    .path
                    .segments
                    .iter()
                    .map(|segment| crate::scope::ident_name(&segment.ident))
                    .collect();
                if let [first, middle, third] = names.as_slice()
                    && middle == "primitive"
                    && matches!(first.as_str(), "std" | "core")
                    && (path.path.leading_colon.is_some()
                        || self.lookup(&[Ns::Type], first).is_none())
                {
                    return crate::scope::primitive_suffix(third);
                }
                if names.len() == 1 {
                    return self.name_prim(&names[0]);
                }
                None
            }
            _ => None,
        }
    }

    /// Whether a local macro rule, use import, or glob import claims the
    /// name, so the call is not the standard macro it spells.
    pub(crate) fn macro_shadowed(&self, name: &str) -> bool {
        !self.prim_fallback
            || self.frames.iter().any(|frame| frame.glob)
            || self.lookup(&[Ns::Macro], name).is_some()
    }

    /// The block nesting level of the traversal.
    pub(crate) fn depth(&self) -> usize {
        self.depth
    }

    pub(crate) fn push_facts(&mut self, facts: schedule::BlockFacts) {
        self.block_facts.push(facts);
    }

    /// The identity of a type reference, for the scheduler's cast labels.
    pub(crate) fn type_identity(&self, name: &str) -> FreeKey {
        match self.lookup(&[Ns::Type], name) {
            Some(resolved) => FreeKey::Resolved(self.identity(&resolved)),
            None => FreeKey::Unresolved(crate::scope::unraw(name).to_string()),
        }
    }

    /// The canonical identity the resolver names a resolved binding by.
    fn identity(&self, resolved: &Resolved) -> String {
        match resolved.domain {
            Domain::Local => self
                .local_canons
                .get(&resolved.id)
                .expect("a local binding")
                .clone(),
            Domain::Inherited => self.inherited_binding(resolved.id).canon.clone(),
        }
    }

    /// Pass every visible value binding into the scheduler's value
    /// environment, a nearer name masking a farther one.
    pub(crate) fn frame_env(&self, env: &mut BTreeMap<String, schedule::Value>) {
        let frames = self
            .self_frame
            .iter()
            .chain(self.generics)
            .chain(self.seed)
            .chain(self.inherited.frames.iter());
        for frame in self.frames.iter().rev().chain(frames) {
            for (name, binding) in frame.bindings(Ns::Value) {
                let port = match binding.origin {
                    Origin::Parameter => schedule::Port::Param(binding.port),
                    _ => binding.value_port.as_ref().map_or_else(
                        || schedule::Port::Free(FreeKey::Resolved(binding.canon.clone())),
                        Clone::clone,
                    ),
                };
                env.entry(name.clone()).or_insert_with(|| schedule::Value {
                    port,
                    prim: binding.prim,
                    helper: binding.helper,
                });
            }
        }
    }

    /// Bind the local names a block use tree imports into the current
    /// frame, a glob marking the frame uncertain.
    fn prebind_use_tree(&mut self, tree: &syn::UseTree) {
        match tree {
            syn::UseTree::Path(path) => self.prebind_use_tree(&path.tree),
            syn::UseTree::Name(name) => {
                self.prebind_macro_name(crate::scope::unraw(&name.ident.to_string()));
            }
            syn::UseTree::Rename(rename) => {
                self.prebind_macro_name(crate::scope::unraw(&rename.rename.to_string()));
            }
            syn::UseTree::Group(group) => {
                for inner in &group.items {
                    self.prebind_use_tree(inner);
                }
            }
            syn::UseTree::Glob(_) => self.top_glob(),
        }
    }

    /// Bind one imported macro name into the current frame.
    fn prebind_macro_name(&mut self, name: &str) {
        let id = self.alloc_id();
        self.top_bind(
            Ns::Macro,
            name,
            Binding {
                id,
                canon: String::new(),
                origin: Origin::Use,
                prim: None,
                port: 0,
                value_port: None,
                target: None,
                raw: false,
                helper: None,
            },
        );
    }

    /// Mark the current frame as glob-scoped.
    fn top_glob(&mut self) {
        self.frames.last_mut().expect("a scope frame").glob = true;
    }

    /// Bind block imports into the caller's temporary pre-scan frame.
    fn prebind_block_uses(&mut self, block: &syn::Block) {
        let mut binder = BlockUseBinder { renamer: self };
        syn::visit::visit_block(&mut binder, block);
    }

    /// Visit the attribute list of an item or function.
    fn visit_attrs(&mut self, attrs: &mut [syn::Attribute]) {
        for attr in attrs {
            self.visit_attribute_mut(attr);
        }
    }

    /// Bind a loop or block label in the current frame.
    fn bind_label(&mut self, label: Option<&mut syn::Label>) {
        if let Some(label) = label {
            self.bind_ident(Ns::Label, Origin::Label, &mut label.name.ident);
        }
    }

    /// Resolve a label through the label namespace only, rustc keeps it
    /// distinct from lifetimes.
    fn rename_label(&mut self, lifetime: &mut syn::Lifetime) {
        self.resolve(&[Ns::Label], &mut lifetime.ident);
    }

    /// Drop a leading `::` when no frame in the type namespace names the
    /// path's first segment, `::std` and `std` then naming the same crate.
    fn unroot(&self, leading_colon: &mut Option<syn::Token![::]>, first: Option<&Ident>) {
        if self.analysis || leading_colon.is_none() {
            return;
        }
        let Some(first) = first else {
            return;
        };
        let name = first.to_string();
        let bound = self
            .frames
            .iter()
            .any(|frame| frame.lookup(Ns::Type, &name).is_some())
            || self
                .self_frame
                .as_ref()
                .is_some_and(|frame| frame.lookup(Ns::Type, &name).is_some())
            || self
                .generics
                .is_some_and(|frame| frame.lookup(Ns::Type, &name).is_some())
            || self
                .seed
                .is_some_and(|frame| frame.lookup(Ns::Type, &name).is_some())
            || self
                .inherited
                .frames
                .iter()
                .any(|frame| frame.lookup(Ns::Type, &name).is_some());
        if !bound {
            *leading_colon = None;
        }
    }

    /// The module frame of `id`, by domain.
    fn module_frame(&self, domain: Domain, id: BindingId) -> Option<&Frame> {
        match domain {
            Domain::Local => self.mod_frames.get(&id),
            Domain::Inherited => self.inherited.mod_frames.get(&id),
        }
    }

    /// The alias target of an index, by domain.
    fn target_of(&self, domain: Domain, index: usize) -> &Target {
        match domain {
            Domain::Local => self.targets.get(index).expect("a target index"),
            Domain::Inherited => self.inherited.targets.get(index).expect("a target index"),
        }
    }

    /// Resolve a path through imports and available module frames.
    fn resolve_path(&mut self, path: &mut syn::Path) {
        self.unroot(
            &mut path.leading_colon,
            path.segments.first().map(|s| &s.ident),
        );
        if path.leading_colon.is_none()
            && let Some(first) = path.segments.first_mut()
            && self.resolve_opaque(&mut first.ident)
        {
            return;
        }
        let ns: &[Ns] = if self.in_type {
            &[Ns::Type]
        } else {
            &[Ns::Value, Ns::Type]
        };
        let root_ns = if path.segments.len() > 1 {
            &[Ns::Type][..]
        } else {
            ns
        };
        self.record_path(path, ns, root_ns);
        if self.analysis || path.leading_colon.is_some() {
            return;
        }
        let Some(resolved) = path
            .segments
            .first()
            .and_then(|first| self.lookup(root_ns, &first.ident.to_string()))
        else {
            return;
        };
        if let Some(index) = resolved.target {
            let (rooted, start, mut rendered) = {
                let target = self.target_of(resolved.domain, index);
                (
                    target.rooted,
                    target
                        .segments
                        .last()
                        .and_then(|seg| seg.origin.map(|(domain, id, _)| (domain, id))),
                    self.render_target(target),
                )
            };
            rendered.last_mut().expect("an alias target").arguments = core::mem::replace(
                &mut path
                    .segments
                    .first_mut()
                    .expect("a first segment")
                    .arguments,
                syn::PathArguments::None,
            );
            self.chain_modules(start, ns, path.segments.iter_mut().skip(1));
            rendered.extend(core::mem::take(&mut path.segments).into_iter().skip(1));
            path.segments = rendered;
            path.leading_colon = rooted.then(Default::default);
        } else {
            let name = self.render(&resolved);
            rename(
                &mut path.segments.first_mut().expect("a first segment").ident,
                &name,
            );
            self.chain_modules(
                Some((resolved.domain, resolved.id)),
                ns,
                path.segments.iter_mut().skip(1),
            );
        }
    }

    /// The rendered segments of an alias target, a local module step renamed.
    fn render_target(
        &self,
        target: &Target,
    ) -> syn::punctuated::Punctuated<syn::PathSegment, syn::Token![::]> {
        target
            .segments
            .iter()
            .map(|seg| match seg.origin {
                Some((Domain::Local, id, _)) => syn::PathSegment {
                    ident: Ident::new(
                        self.local_canons.get(&id).expect("a local binding"),
                        Span::call_site(),
                    ),
                    arguments: syn::PathArguments::None,
                },
                // Inherited target spellings retain the imported association.
                _ => syn::PathSegment::from(if let Some(name) = seg.name.strip_prefix("r#") {
                    Ident::new_raw(name, Span::call_site())
                } else {
                    Ident::new(&seg.name, Span::call_site())
                }),
            })
            .collect()
    }

    /// Rename module members until the available module chain ends.
    fn chain_modules<'a>(
        &mut self,
        mut current: Option<(Domain, BindingId)>,
        ns: &[Ns],
        mut segments: impl ExactSizeIterator<Item = &'a mut syn::PathSegment>,
    ) {
        while let Some(segment) = segments.next() {
            let member_ns = if segments.len() == 0 { ns } else { &[Ns::Type] };
            let hit = current.and_then(|(domain, id)| {
                self.module_frame(domain, id)
                    .and_then(|frame| {
                        module_member(
                            frame,
                            member_ns,
                            &segment.ident.to_string(),
                            self.opaque_tokens,
                        )
                    })
                    .map(|(member, member_ns)| ((domain, member), member_ns))
            });
            match hit {
                Some(((Domain::Local, member), _)) => {
                    rename(
                        &mut segment.ident,
                        self.local_canons.get(&member).expect("a local binding"),
                    );
                    current = self
                        .module_frame(Domain::Local, member)
                        .map(|_| (Domain::Local, member));
                }
                Some(((Domain::Inherited, member), _)) => {
                    // An inherited member keeps its written name.
                    current = self
                        .module_frame(Domain::Inherited, member)
                        .map(|_| (Domain::Inherited, member));
                }
                None => current = None,
            }
        }
    }

    /// Attach the original binding each path segment names to that
    /// segment's identifier, the syntax untouched.
    fn record_path_origins(&mut self, ns: &[Ns], root_ns: &[Ns], path: &syn::Path) {
        for (position, origin) in self.path_segment_origins(ns, root_ns, path) {
            self.record_origin(&path.segments[position], origin);
        }
    }

    /// The original binding each path segment names, in segment order,
    /// through the module frames and the import target of the first
    /// name, the syntax untouched.
    fn path_segment_origins(
        &self,
        ns: &[Ns],
        root_ns: &[Ns],
        path: &syn::Path,
    ) -> Vec<(usize, ResolvedOrigin)> {
        if path.leading_colon.is_some() || path.segments.is_empty() {
            return Vec::new();
        }
        if path.segments.len() > 1 && path.segments[0].ident == "self" {
            // `self` names the current module; its tail reads as a bare
            // path in that module.
            let head_ns = if path.segments.len() == 2 {
                ns
            } else {
                root_ns
            };
            return self
                .path_segment_walk(ns, head_ns, &path.segments[1], path.segments.iter().skip(2))
                .into_iter()
                .map(|(position, origin)| (position + 1, origin))
                .collect();
        }
        self.path_segment_walk(ns, root_ns, &path.segments[0], path.segments.iter().skip(1))
    }

    /// The segment walk of `path_segment_origins` over a bare path.
    fn path_segment_walk<'a>(
        &self,
        ns: &[Ns],
        root_ns: &[Ns],
        first: &'a syn::PathSegment,
        rest: impl ExactSizeIterator<Item = &'a syn::PathSegment>,
    ) -> Vec<(usize, ResolvedOrigin)> {
        let mut origins: Vec<(usize, ResolvedOrigin)> = Vec::new();
        let rest_count = rest.len();
        let name = first.ident.to_string();
        let Some(resolved) = self.lookup(root_ns, &name) else {
            return origins;
        };
        let mut current: Option<(Domain, BindingId)> = None;
        let mut stepped = false;
        if let Some(index) = resolved.target {
            let target = self.target_of(resolved.domain, index);
            if !target.rooted {
                for (position, segment) in target.segments.iter().enumerate() {
                    let last = position + 1 == target.segments.len() && rest_count == 0;
                    match current {
                        None => {
                            let Some((domain, id, _)) = segment.origin else {
                                return origins;
                            };
                            if self.module_frame(domain, id).is_none() {
                                return origins;
                            }
                            current = Some((domain, id));
                        }
                        Some((domain, id)) => {
                            let frame = self
                                .module_frame(domain, id)
                                .expect("a stepped module frame persists through the walk");
                            let Some((member, member_ns)) = module_member(
                                frame,
                                if last { ns } else { &[Ns::Type] },
                                &segment.name,
                                self.opaque_tokens,
                            ) else {
                                return origins;
                            };
                            origins.push((
                                0,
                                ResolvedOrigin {
                                    domain,
                                    ns: member_ns,
                                    id: member,
                                },
                            ));
                            stepped = true;
                            if last {
                                return origins;
                            }
                            if self.module_frame(domain, member).is_none() {
                                return origins;
                            }
                            current = Some((domain, member));
                        }
                    }
                }
                if !stepped && let Some((domain, id)) = current {
                    origins.push((
                        0,
                        ResolvedOrigin {
                            domain,
                            ns: Ns::Type,
                            id,
                        },
                    ));
                }
            }
        } else {
            origins.push((
                0,
                ResolvedOrigin {
                    domain: resolved.domain,
                    ns: resolved.ns,
                    id: resolved.id,
                },
            ));
        }
        if rest_count == 0 {
            return origins;
        }
        let (domain, id) = if let Some(step) = current {
            step
        } else {
            if self.module_frame(resolved.domain, resolved.id).is_none() {
                return origins;
            }
            (resolved.domain, resolved.id)
        };
        self.path_tail_origins(ns, domain, id, rest, &mut origins);
        origins
    }

    fn path_tail_origins<'a>(
        &self,
        ns: &[Ns],
        domain: Domain,
        mut id: BindingId,
        rest: impl ExactSizeIterator<Item = &'a syn::PathSegment>,
        origins: &mut Vec<(usize, ResolvedOrigin)>,
    ) {
        let rest_count = rest.len();
        for (position, segment) in rest.enumerate() {
            let frame = self
                .module_frame(domain, id)
                .expect("a stepped tail module frame persists through the walk");
            let Some((member, member_ns)) = module_member(
                frame,
                if position + 1 == rest_count {
                    ns
                } else {
                    &[Ns::Type]
                },
                &segment.ident.to_string(),
                self.opaque_tokens,
            ) else {
                break;
            };
            origins.push((
                position + 1,
                ResolvedOrigin {
                    domain,
                    ns: member_ns,
                    id: member,
                },
            ));
            if self.module_frame(domain, member).is_none() {
                break;
            }
            id = member;
        }
    }

    /// Record one segment's original binding at its identifier.
    fn record_origin(&mut self, segment: &syn::PathSegment, origin: ResolvedOrigin) {
        self.facts
            .idents
            .insert(core::ptr::from_ref(&segment.ident), origin);
    }

    /// The one proof-backed original route of the self type's terminal
    /// owner, through the original module frames. The terminal is a type
    /// rather than a module, every prefix segment is a proven module step,
    /// and the first segment's alias, when one, expands to its target. An
    /// in-unit `crate` prefix is dropped for a whole-file pass.
    fn owner_route(&self, path: &syn::Path) -> Option<(ResolvedOrigin, OwnerRoute)> {
        if path.leading_colon.is_some() || path.segments.is_empty() {
            return None;
        }
        let root_ns: &[Ns] = &[Ns::Type];
        let mut rest = path.segments.iter();
        let first = rest.next()?;
        let head_name = first.ident.to_string();
        // An in-unit `crate` prefix reaches the original root, provable only
        // for a whole-file pass; `self` and `super` need the original module
        // path, which the pass does not track.
        let (first, rest) = if head_name == "crate" {
            if self.context || self.seed.is_some() {
                return None;
            }
            (rest.next()?, rest)
        } else if head_name == "self" || head_name == "super" {
            return None;
        } else {
            (first, rest)
        };
        let first_name = first.ident.to_string();
        let resolved = if head_name == "crate" {
            let binding = self.frames.first()?.lookup(Ns::Type, &first_name)?;
            Self::resolved(Domain::Local, Ns::Type, binding)
        } else {
            self.lookup(root_ns, &first_name)?
        };
        let mut segments: Vec<String> = Vec::new();
        let root;
        if let Some(index) = resolved.target {
            let target = self.target_of(resolved.domain, index);
            if target.rooted || target.segments.is_empty() {
                return None;
            }
            let (domain, id, _) = target.segments.first()?.origin?;
            root = (domain, id);
            segments.extend(target.segments.iter().map(|seg| seg.name.clone()));
        } else {
            root = (resolved.domain, resolved.id);
            segments.push(first_name);
        }
        segments.extend(rest.map(|segment| segment.ident.to_string()));
        let (domain, mut id) = root;
        let mut origins = vec![ResolvedOrigin {
            domain,
            ns: Ns::Type,
            id,
        }];
        for (position, name) in segments.iter().enumerate().skip(1) {
            let frame = self.module_frame(domain, id)?;
            let (member, member_ns) = module_member(frame, &[Ns::Type], name, false)?;
            let terminal = position + 1 == segments.len();
            let is_module = self.module_frame(domain, member).is_some();
            if terminal == is_module {
                return None;
            }
            origins.push(ResolvedOrigin {
                domain,
                ns: member_ns,
                id: member,
            });
            id = member;
        }
        let terminal = *origins.last()?;
        if self.module_frame(terminal.domain, terminal.id).is_some() {
            return None;
        }
        let first = Resolution {
            kind: if domain == Domain::Inherited {
                ReferenceKind::Inherited
            } else {
                ReferenceKind::Owned
            },
            prim: None,
            origin: origins.first().copied(),
        };
        Some((
            terminal,
            OwnerRoute {
                segments,
                origins,
                first,
            },
        ))
    }

    /// Whether the path's first name, an import target's first name
    /// through an alias, or its rooted spelling names a crate root the
    /// original frames do not shadow.
    fn portable_first(&self, ns: &[Ns], first: &syn::PathSegment) -> bool {
        let name = first.ident.to_string();
        match self.lookup(ns, &name) {
            None => self.crate_root(&name),
            Some(resolved) => match resolved.target {
                Some(index) => {
                    let target = self.target_of(resolved.domain, index);
                    target.rooted
                        || target.segments.first().is_some_and(|segment| {
                            segment.origin.is_none() && self.crate_root(&segment.name)
                        })
                }
                None => false,
            },
        }
    }

    /// The crate roots the portable proof admits, raw or plain, while
    /// no original type-namespace name shadows them.
    fn crate_root(&self, name: &str) -> bool {
        matches!(unraw(name), "core" | "alloc" | "std") && self.lookup(&[Ns::Type], name).is_none()
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
                    if !self.resolve_opaque(&mut ident) {
                        self.record_token(ns, &ident.to_string());
                        self.resolve(ns, &mut ident);
                    }
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
            self.macro_supported = false;
            return self.rename_tokens(tokens);
        };
        for (index, expr) in args.iter_mut().enumerate() {
            self.helper_syntax.clear_expr(expr);
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

    /// Store one written target and return its index for an alias binding.
    fn store_target(&mut self, rooted: bool, segments: Vec<TargetSeg>) -> usize {
        let index = self.targets.len();
        self.targets.push(Target { rooted, segments });
        index
    }

    /// Resolve one `use` path segment against the pass's frames.
    fn resolve_first_seg(&self, name: &str) -> FirstSeg<'_> {
        match self.lookup(&[Ns::Type], name) {
            Some(resolved) if resolved.target.is_some() => FirstSeg::Alias(
                self.target_of(resolved.domain, resolved.target.expect("a target index")),
            ),
            Some(resolved) => FirstSeg::Module(resolved.domain, resolved.id),
            None => FirstSeg::Free,
        }
    }

    /// Rewrite imported names as explicit aliases to preserve their targets.
    fn bind_use_tree(
        &mut self,
        rooted: bool,
        prefix: &[String],
        tree: syn::UseTree,
    ) -> syn::UseTree {
        match tree {
            syn::UseTree::Path(mut path) => {
                let mut prefix = prefix.to_vec();
                prefix.push(path.ident.to_string());
                path.tree = Box::new(self.bind_use_tree(rooted, &prefix, *path.tree));
                syn::UseTree::Path(path)
            }
            syn::UseTree::Name(name) if self.context && name.ident == "self" => {
                // `a::{self}` imports the module itself under its own name.
                if let Some(alias) = prefix.last() {
                    let (rooted, segments) =
                        resolve_target_path(&|name| self.resolve_first_seg(name), rooted, prefix);
                    let target = self.store_target(rooted, segments);
                    self.bind_value_and_type(alias, Some(target), Origin::Use);
                }
                syn::UseTree::Name(name)
            }
            syn::UseTree::Name(name) => {
                self.bind_use_leaf(rooted, prefix, &name.ident, &name.ident)
            }
            syn::UseTree::Rename(alias) => {
                self.bind_use_leaf(rooted, prefix, &alias.ident, &alias.rename)
            }
            syn::UseTree::Group(mut group) => {
                group.items = group
                    .items
                    .into_pairs()
                    .map(|pair| {
                        let (tree, comma) = pair.into_tuple();
                        syn::punctuated::Pair::new(self.bind_use_tree(rooted, prefix, tree), comma)
                    })
                    .collect();
                syn::UseTree::Group(group)
            }
            glob @ syn::UseTree::Glob(_) => {
                self.top_glob();
                glob
            }
        }
    }

    /// Bind one `use` leaf that imports `ident` under `alias`.
    fn bind_use_leaf(
        &mut self,
        rooted: bool,
        prefix: &[String],
        ident: &Ident,
        alias: &Ident,
    ) -> syn::UseTree {
        let mut helper_path = syn::Path {
            leading_colon: rooted.then(syn::token::PathSep::default),
            segments: prefix
                .iter()
                .map(|part| {
                    syn::PathSegment::from(if let Some(raw) = part.strip_prefix("r#") {
                        Ident::new_raw(raw, Span::call_site())
                    } else {
                        Ident::new(part, Span::call_site())
                    })
                })
                .collect(),
        };
        helper_path
            .segments
            .push(syn::PathSegment::from(ident.clone()));
        let helper = self.helper_table.resolve(&self.helper_module, &helper_path);
        let target = (self.context || self.analysis).then(|| {
            let mut path = prefix.to_vec();
            path.push(ident.to_string());
            let (rooted, segments) =
                resolve_target_path(&|name| self.resolve_first_seg(name), rooted, &path);
            self.store_target(rooted, segments)
        });
        let binding = self.bind_value_and_type(&alias.to_string(), target, Origin::Use);
        if let Some(helper) = helper {
            self.frames.last_mut().expect("a scope frame").set_helper(
                Ns::Value,
                &alias.to_string(),
                helper,
            );
        }
        syn::UseTree::Rename(syn::UseRename {
            rename: Ident::new(&binding.canon, alias.span()),
            ident: ident.clone(),
            as_token: <syn::Token![as]>::default(),
        })
    }

    /// Bind every name `pat` binds, then visit and rewrite it. The
    /// scheduler's plan of the original pattern picks the canonical
    /// name offset, the proven prim and the sealed value port, the block
    /// not yet permuted.
    fn bind_pat(&mut self, pat: &mut syn::Pat) {
        let mut names = Vec::new();
        pattern_names(pat, &mut names);
        let mut plan = self
            .block_facts
            .last()
            .and_then(|facts| facts.plan(pat))
            .map(|plan| schedule::BindingPlan {
                offset: plan.offset,
                prim: plan.prim,
                port: plan.port.clone(),
                active: plan.active,
            });
        for name in &names {
            self.bind_full(Ns::Value, name, Origin::Let, None, 0, plan.take());
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
        mut body: Option<&mut syn::Block>,
    ) {
        let receiver = core::mem::replace(
            &mut self.receiver,
            sig.inputs
                .iter()
                .any(|input| matches!(input, syn::FnArg::Receiver(_))),
        );
        let observed = self.observed;
        self.observed |= crate::context::live_attr(attrs);
        let scoped = if self.compiles
            && let Some(block) = body.as_deref_mut()
        {
            self.push();
            self.prebind_block_uses(block);
            let frozen = schedule::unknown_macro_reaches(self, block) || self.observed;
            self.pop();
            self.scopes.push(schedule::ScopeState {
                frozen,
                cfg: attrs.iter().any(|attr| {
                    let path = attr.path();
                    path.is_ident("cfg") || path.is_ident("cfg_attr")
                }),
                ..Default::default()
            });
            true
        } else {
            false
        };
        begin_generics(self, &mut sig.generics);
        self.visit_attrs(attrs);
        self.push();
        for (port, input) in sig.inputs.iter_mut().enumerate() {
            if let syn::FnArg::Typed(pat_type) = input {
                let prim = self.param_prim(&pat_type.ty);
                let mut names = Vec::new();
                pattern_names(&mut pat_type.pat, &mut names);
                for name in &names {
                    let binding =
                        self.bind_full(Ns::Value, name, Origin::Parameter, prim, port, None);
                    if self.analysis {
                        self.record_parameter(Domain::Local, binding.id, port);
                    }
                }
            }
        }
        visit_fn_inputs(self, sig.inputs.iter_mut());
        syn::visit_mut::visit_return_type_mut(self, &mut sig.output);
        if let Some(body) = body {
            self.visit_block_mut(body);
        }
        self.pop();
        self.pop();
        if scoped {
            self.scopes.pop().expect("a fn scope");
        }
        self.observed = observed;
        self.receiver = receiver;
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

    fn visit_path_arguments(&mut self, path: &mut syn::Path) {
        for segment in &mut path.segments {
            match &mut segment.arguments {
                syn::PathArguments::AngleBracketed(args) => {
                    for arg in &mut args.args {
                        if matches!(arg, syn::GenericArgument::Const(_)) {
                            let outer = core::mem::replace(&mut self.in_type, false);
                            syn::visit_mut::visit_generic_argument_mut(self, arg);
                            if self.constants_allowed()
                                && let syn::GenericArgument::Const(expr) = arg
                            {
                                crate::drift::fold_scalar_argument_block(expr);
                            }
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

    /// The reference provenance and transported marker edges, consumed after the visit.
    pub(crate) fn into_reference_facts(mut self) -> ReferenceFacts {
        let mut facts = self.facts;
        let (paths, macros) = self.helper_syntax.take_marker_transport();
        facts.marker_paths.extend(paths);
        facts.marker_macros.extend(macros);
        facts
    }
}

/// Macro arguments as a comma list or the `elem; count` of `vec!`.
pub(crate) enum MacroArgs {
    List(syn::punctuated::Punctuated<syn::Expr, syn::Token![,]>),
    Repeat(syn::punctuated::Punctuated<syn::Expr, syn::Token![;]>),
}

impl MacroArgs {
    pub(crate) fn iter_mut(&mut self) -> syn::punctuated::IterMut<'_, syn::Expr> {
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

/// `name` as its bound canonical name, else as written.
fn push_format_name(renamer: &mut Renamer<'_>, name: &str, out: &mut String) {
    match renamer.lookup(&[Ns::Value], name) {
        Some(resolved) if resolved.target.is_none() => {
            renamer.record_token(&[Ns::Value], name);
            out.push_str(&renamer.render(&resolved));
        }
        _ => {
            renamer
                .macro_origins
                .as_mut()
                .expect("a macro origin collector")
                .push(Resolution::default());
            out.push_str(name);
        }
    }
}

/// Rewrite one `{…}` placeholder body, its name and any `name$` width or
/// precision in the spec.
fn rewrite_format_arg(renamer: &mut Renamer<'_>, arg: &str, out: &mut String) {
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
fn rewrite_format_str(renamer: &mut Renamer<'_>, lit: &syn::LitStr) -> syn::LitStr {
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

/// Alpha-normalize a parsed file in place, returning the reference facts.
pub(crate) fn normalize_file(
    file: &mut syn::File,
    compiles: bool,
    options: Options<'_>,
) -> ReferenceFacts {
    let mut renamer = Renamer::seeded(options, compiles);
    renamer.visit_file_mut(file);
    renamer.into_reference_facts()
}

pub(crate) fn analyze_file(file: &mut syn::File, options: Options<'_>) -> ReferenceFacts {
    let mut renamer = Renamer::seeded(options, false);
    renamer.analysis = true;
    renamer.visit_file_mut(file);
    renamer.into_reference_facts()
}

/// True for an ident pattern that names a unit struct, variant, or
/// const rather than a binding, by the uppercase naming convention.
pub(crate) fn is_unit_path(id: &syn::PatIdent) -> bool {
    let name = id.ident.to_string();
    id.by_ref.is_none()
        && id.mutability.is_none()
        && id.subpat.is_none()
        && Ns::Value.key(&name).starts_with(|c: char| c.is_uppercase())
}

/// Call `f` on every identifier pattern of `pat`, in traversal order.
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
fn rewrite_pat_bindings(renamer: &mut Renamer<'_>, pat: &mut syn::Pat) {
    if !renamer.analysis {
        struct Explicit;
        impl VisitMut for Explicit {
            fn visit_field_pat_mut(&mut self, field: &mut syn::FieldPat) {
                field.colon_token.get_or_insert_with(Default::default);
                syn::visit_mut::visit_field_pat_mut(self, field);
            }
        }
        Explicit.visit_pat_mut(pat);
    }
    walk_pat_idents(pat, &mut |id| {
        let ns: &[Ns] = if is_unit_path(id) {
            &[Ns::Value, Ns::Type]
        } else {
            &[Ns::Value]
        };
        if renamer.analysis {
            if let Some(origin) = renamer.resolution(ns, &id.ident.to_string()).origin {
                let key = core::ptr::from_ref(&id.ident);
                renamer.facts.idents.insert(key, origin);
                if !is_unit_path(id) {
                    renamer.facts.declarations.insert(key);
                }
            }
        } else {
            renamer.resolve(ns, &mut id.ident);
        }
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

/// Binds the leaf macro names a block use tree imports into a temporary
/// frame, the source untouched.
struct BlockUseBinder<'a, 'e> {
    renamer: &'a mut Renamer<'e>,
}

impl Visit<'_> for BlockUseBinder<'_, '_> {
    fn visit_stmt(&mut self, stmt: &syn::Stmt) {
        if let syn::Stmt::Item(syn::Item::Use(use_item)) = stmt {
            self.renamer.prebind_use_tree(&use_item.tree);
        } else {
            syn::visit::visit_stmt(self, stmt);
        }
    }
}

/// The identifier of a type-namespace item, `use` and `macro_rules!` aside.
pub(crate) fn type_item_ident(item: &syn::Item) -> Option<&syn::Ident> {
    match item {
        syn::Item::Struct(item) => Some(&item.ident),
        syn::Item::Enum(item) => Some(&item.ident),
        syn::Item::Union(item) => Some(&item.ident),
        syn::Item::Type(item) => Some(&item.ident),
        syn::Item::Trait(item) => Some(&item.ident),
        syn::Item::TraitAlias(item) => Some(&item.ident),
        syn::Item::Mod(item) => Some(&item.ident),
        _ => None,
    }
}

/// Whether the use tree binds the name `matches` claims, a plain glob
/// binding no particular name.
pub(crate) fn imported_name_matches(
    tree: &syn::UseTree,
    mut matches: impl FnMut(&syn::Ident) -> bool,
) -> bool {
    fn walk(
        tree: &syn::UseTree,
        parent: Option<&syn::Ident>,
        matches: &mut impl FnMut(&syn::Ident) -> bool,
    ) -> bool {
        match tree {
            syn::UseTree::Path(path) => walk(&path.tree, Some(&path.ident), matches),
            syn::UseTree::Name(name) if name.ident == "self" => parent.is_some_and(matches),
            syn::UseTree::Name(name) => matches(&name.ident),
            syn::UseTree::Rename(rename) => matches(&rename.rename),
            syn::UseTree::Group(group) => {
                group.items.iter().any(|tree| walk(tree, parent, matches))
            }
            syn::UseTree::Glob(_) => false,
        }
    }
    walk(tree, None, &mut matches)
}

/// The proven primitive a type alias names, its target a single-segment
/// path resolved through the sibling items and the outer frames.
fn alias_prim(renamer: &Renamer, items: &[&syn::Item], name: &str) -> Option<PrimTy> {
    let mut visited = BTreeSet::new();
    alias_prim_inner(renamer, items, crate::scope::unraw(name), 0, &mut visited)
}

fn alias_prim_inner(
    renamer: &Renamer,
    items: &[&syn::Item],
    name: &str,
    hops: usize,
    visited: &mut BTreeSet<String>,
) -> Option<PrimTy> {
    if hops > 16 || !visited.insert(name.to_string()) {
        return None;
    }
    let alias = items.iter().find_map(|item| match item {
        syn::Item::Type(item) if crate::scope::ident_name(&item.ident) == name => Some(item),
        _ => None,
    })?;
    if alias.attrs.iter().any(|attr| {
        crate::context::live(&attr.meta)
            || attr.path().is_ident("cfg")
            || attr.path().is_ident("cfg_attr")
    }) {
        return None;
    }
    let ty = &alias.ty;
    let target = single_segment_type_name(ty)?;
    let target = crate::scope::unraw(&target);
    // A sibling of the target's name decides it before any outer binding.
    for item in items {
        match item {
            syn::Item::Use(use_item) => {
                if crate::context::has_glob(&use_item.tree) {
                    return None;
                }
                if imported_name_matches(&use_item.tree, |ident| {
                    crate::scope::ident_name(ident) == target
                }) {
                    return None;
                }
            }
            syn::Item::Type(alias) if crate::scope::ident_name(&alias.ident) == target => {
                return alias_prim_inner(renamer, items, target, hops + 1, visited);
            }
            other => {
                if let Some(ident) = type_item_ident(other)
                    && crate::scope::ident_name(ident) == target
                {
                    return None;
                }
            }
        }
    }
    renamer.name_prim(target)
}

/// The name and namespace of an item the prebind recognizes.
pub(crate) fn item_binding(item: &syn::Item) -> Option<(Ns, &Ident)> {
    Some(match item {
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
        _ => return None,
    })
}

/// Pre-bind item names so a use may precede its definition, skipping
/// `macro_rules!` and `use`, which are visible only after their line.
fn prebind(renamer: &mut Renamer<'_>, items: &[&syn::Item]) {
    for item in items {
        let Some((ns, ident)) = item_binding(item) else {
            continue;
        };
        let name = ident.to_string();
        let prim = matches!(item, syn::Item::Type(_))
            .then(|| alias_prim(renamer, items, &name))
            .flatten();
        let origin = if matches!(item, syn::Item::Type(_)) {
            Origin::TypeAlias
        } else {
            Origin::Item
        };
        let binding = renamer.bind_proven(ns, &name, origin, prim);
        if let syn::Item::Struct(item) = item
            && !matches!(item.fields, syn::Fields::Named(_))
        {
            renamer.top_bind(Ns::Value, &name, binding.clone());
        }
        if let syn::Item::Fn(function) = item
            && let Some(helper) = renamer.helper_declaration(&function.block)
        {
            renamer
                .frames
                .last_mut()
                .expect("a scope frame")
                .set_helper(Ns::Value, &name, helper);
            renamer
                .helper_canons
                .insert(helper, FreeKey::Resolved(binding.canon.clone()));
        }
        if let syn::Item::Mod(syn::ItemMod {
            content: Some((_, module_items)),
            ..
        }) = item
        {
            renamer.push();
            let previous = renamer.prim_fallback;
            renamer.prim_fallback = crate::context::scope_primitives_known(module_items);
            let items: Vec<&syn::Item> = module_items.iter().collect();
            prebind(renamer, &items);
            renamer.prim_fallback = previous;
            let frame = renamer.frames.pop().expect("a scope frame");
            renamer.mod_frames.insert(binding.id, frame);
        }
    }
}

/// The member of a module frame that `name` names, in namespace priority
/// order.
fn module_member(frame: &Frame, ns: &[Ns], name: &str, opaque: bool) -> Option<(BindingId, Ns)> {
    for ns in ns {
        if let Some(bind) = frame.lookup(*ns, name) {
            if opaque && (bind.raw || name.starts_with("r#")) {
                return None;
            }
            return Some((bind.id, *ns));
        }
    }
    None
}

/// Visit function parameters and rewrite their binder patterns.
fn visit_fn_inputs<'a>(
    renamer: &mut Renamer<'_>,
    inputs: impl Iterator<Item = &'a mut syn::FnArg>,
) {
    for input in inputs {
        syn::visit_mut::visit_fn_arg_mut(renamer, input);
        if let syn::FnArg::Typed(pat_type) = input {
            rewrite_pat_bindings(renamer, &mut pat_type.pat);
        }
    }
}

/// The identifier of a bare single-segment type, if any.
pub(crate) fn single_segment_type_name(ty: &syn::Type) -> Option<String> {
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
fn begin_generics(renamer: &mut Renamer<'_>, generics: &mut syn::Generics) {
    renamer.push();
    for param in &mut generics.params {
        match param {
            syn::GenericParam::Lifetime(lifetime) => {
                renamer.bind_ident(Ns::Lifetime, Origin::Lifetime, &mut lifetime.lifetime.ident);
            }
            syn::GenericParam::Type(ty_param) => {
                renamer.bind_ident(Ns::Type, Origin::Generic, &mut ty_param.ident);
            }
            syn::GenericParam::Const(r#const) => {
                // A bare reference in `Foo<N>` reads as a type argument.
                let binding =
                    renamer.bind_value_and_type(&r#const.ident.to_string(), None, Origin::Generic);
                if renamer.analysis {
                    renamer.record_ident(&r#const.ident, Ns::Type, binding.id, true);
                } else {
                    rename(&mut r#const.ident, &binding.canon);
                }
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
fn walk_let_cond(renamer: &mut Renamer<'_>, cond: &mut syn::Expr) {
    match cond {
        syn::Expr::Binary(binary) => {
            walk_let_cond(renamer, &mut binary.left);
            walk_let_cond(renamer, &mut binary.right);
        }
        syn::Expr::Let(let_expr) => bind_let(renamer, let_expr),
        other => syn::visit_mut::visit_expr_mut(renamer, other),
    }
}

/// Visit a `let` expression, binding its pattern names after its
/// initializer.
fn bind_let(renamer: &mut Renamer<'_>, let_expr: &mut syn::ExprLet) {
    syn::visit_mut::visit_expr_mut(renamer, &mut let_expr.expr);
    renamer.bind_pat(&mut let_expr.pat);
}

enum TypeResolution {
    Path(Resolution),
    Macro(Vec<Resolution>),
    Block(Option<crate::reference::BlockProof>),
}

fn type_resolutions(facts: &ReferenceFacts, ty: &syn::Type) -> Vec<TypeResolution> {
    struct Collect<'a> {
        facts: &'a ReferenceFacts,
        resolutions: Vec<TypeResolution>,
    }
    impl<'ast> Visit<'ast> for Collect<'_> {
        fn visit_path(&mut self, path: &'ast syn::Path) {
            self.resolutions.push(TypeResolution::Path(
                self.facts
                    .paths
                    .get(&core::ptr::from_ref(path))
                    .copied()
                    .unwrap_or_default(),
            ));
            syn::visit::visit_path(self, path);
        }

        fn visit_macro(&mut self, mac: &'ast syn::Macro) {
            self.resolutions.push(TypeResolution::Macro(
                self.facts
                    .macros
                    .get(&core::ptr::from_ref(mac))
                    .cloned()
                    .unwrap_or_default(),
            ));
            syn::visit::visit_macro(self, mac);
        }

        fn visit_block(&mut self, block: &'ast syn::Block) {
            self.resolutions.push(TypeResolution::Block(
                self.facts.blocks.get(&core::ptr::from_ref(block)).cloned(),
            ));
            syn::visit::visit_block(self, block);
        }
    }
    let mut collect = Collect {
        facts,
        resolutions: Vec::new(),
    };
    collect.visit_type(ty);
    collect.resolutions
}

fn restore_type(facts: &mut ReferenceFacts, ty: &syn::Type, resolutions: &[TypeResolution]) {
    struct Restore<'a> {
        facts: &'a mut ReferenceFacts,
        resolutions: core::slice::Iter<'a, TypeResolution>,
    }
    impl<'ast> Visit<'ast> for Restore<'_> {
        fn visit_path(&mut self, path: &'ast syn::Path) {
            let Some(TypeResolution::Path(resolution)) = self.resolutions.next() else {
                unreachable!("cloned types preserve path traversal");
            };
            self.facts
                .paths
                .insert(core::ptr::from_ref(path), *resolution);
            syn::visit::visit_path(self, path);
        }

        fn visit_macro(&mut self, mac: &'ast syn::Macro) {
            let Some(TypeResolution::Macro(resolutions)) = self.resolutions.next() else {
                unreachable!("cloned types preserve macro traversal");
            };
            self.facts
                .macros
                .insert(core::ptr::from_ref(mac), resolutions.clone());
            syn::visit::visit_macro(self, mac);
        }

        fn visit_block(&mut self, block: &'ast syn::Block) {
            let Some(TypeResolution::Block(proof)) = self.resolutions.next() else {
                unreachable!("cloned types preserve block traversal");
            };
            if let Some(proof) = proof {
                self.facts
                    .blocks
                    .insert(core::ptr::from_ref(block), proof.clone());
            }
            syn::visit::visit_block(self, block);
        }
    }
    let mut restore = Restore {
        facts,
        resolutions: resolutions.iter(),
    };
    restore.visit_type(ty);
}

impl VisitMut for Renamer<'_> {
    fn visit_file_mut(&mut self, file: &mut syn::File) {
        self.observed |= crate::context::live_attr(&file.attrs);
        self.constants_observed |= crate::context::live_attr(&file.attrs);
        self.prim_fallback &= crate::context::scope_primitives_known(&file.items);
        self.helper_blocked |= crate::context::blocking_attr(&file.attrs);
        let items: Vec<&syn::Item> = file.items.iter().collect();
        prebind(self, &items);
        syn::visit_mut::visit_file_mut(self, file);
    }

    fn visit_item_mut(&mut self, item: &mut syn::Item) {
        let observed = self.constants_observed;
        self.constants_observed |=
            crate::drift::item_attrs(item).is_some_and(|attrs| crate::context::live_attr(attrs));
        let prepared = core::mem::replace(&mut self.constants_prepared, false);
        let regions_active = core::mem::replace(&mut self.regions_active, true);
        let helper_blocked = self.helper_blocked;
        self.helper_blocked |= crate::drift::item_attrs(item)
            .is_some_and(|attrs| crate::context::blocking_attr(attrs));
        syn::visit_mut::visit_item_mut(self, item);
        self.constants_prepared = prepared;
        self.regions_active = regions_active;
        self.constants_observed = observed;
        self.helper_blocked = helper_blocked;
    }

    fn visit_expr_mut(&mut self, expr: &mut syn::Expr) {
        let observed = self.constants_observed;
        self.constants_observed |= !crate::drift::expr_attrs(expr).is_empty();
        let prepared = self.constants_prepared;
        if !prepared && self.constants_allowed() {
            crate::constants::normalize_expr(self, expr, None);
        }
        self.constants_prepared = true;
        syn::visit_mut::visit_expr_mut(self, expr);
        self.constants_prepared = prepared;
        self.constants_observed = observed;
    }

    fn visit_block_mut(&mut self, block: &mut syn::Block) {
        self.push();
        let before_constants = self.helper_syntax.snapshot_block(block);
        let items: Vec<&syn::Item> = block
            .stmts
            .iter()
            .filter_map(|stmt| match stmt {
                syn::Stmt::Item(item) => Some(item),
                _ => None,
            })
            .collect();
        prebind(self, &items);
        for item in &items {
            if let syn::Item::Use(import) = item {
                self.prebind_use_tree(&import.tree);
            }
        }
        self.depth += 1;
        if self.constants_allowed() {
            crate::constants::normalize_block(self, block);
        }
        self.helper_syntax.refresh_block(before_constants, block);
        let mut original_syntax = Some(self.helper_syntax.snapshot_block(block));
        let prepared = core::mem::replace(&mut self.constants_prepared, true);
        let analyzed = self.compiles
            && !self.analysis
            && self.regions_active
            && !self.scopes.is_empty()
            && self.macro_origins.is_none();
        if analyzed {
            schedule::analyze(self, block);
            let before_algebra = self.helper_syntax.snapshot_block(block);
            if self.constants_allowed() && !self.glob_uncertain() && !self.cfg_blocked() {
                let facts = self.last_block_facts().expect("an analyzed block");
                crate::algebra::normalize(self, block, facts);
            }
            self.helper_syntax.refresh_block(before_algebra, block);
            if let Some(original) = self
                .block_facts
                .last_mut()
                .expect("an analyzed block")
                .original_on_exhaustion(block, self.depth - 1)
            {
                let before_rollback = self.helper_syntax.snapshot_block(block);
                self.block_facts.pop();
                *block = original;
                self.helper_syntax
                    .extend(original_syntax.take().expect("original helper syntax"));
                self.helper_syntax.refresh_block(before_rollback, block);
                schedule::record_all_opaque(self, block);
            }
        }
        let complete = analyzed
            && self
                .last_block_facts()
                .is_some_and(schedule::BlockFacts::can_rewrite);
        let before_visit = self.helper_syntax.snapshot_block(block);
        let mut reachable = true;
        for stmt in &mut block.stmts {
            let boundary = matches!(
                schedule::stmt_kind(self, stmt),
                schedule::StmtKind::Boundary(true)
            );
            let regions_active =
                core::mem::replace(&mut self.regions_active, complete && reachable);
            self.visit_stmt_mut(stmt);
            self.regions_active = regions_active;
            reachable &= !boundary;
        }
        self.constants_prepared = prepared;
        self.depth -= 1;
        self.pop();
        self.helper_syntax.refresh_block(before_visit, block);
        if let Some(original_syntax) = original_syntax {
            self.helper_syntax.refresh_block(original_syntax, block);
        }
        if analyzed {
            let facts = self.block_facts.pop().expect("an analyzed block");
            facts.finish(block, &mut self.facts);
        }
    }

    fn visit_local_mut(&mut self, local: &mut syn::Local) {
        let observed = self.constants_observed;
        self.constants_observed |= !local.attrs.is_empty();
        // The initializer resolves under the outer bindings.
        if let Some(init) = &mut local.init {
            syn::visit_mut::visit_expr_mut(self, &mut init.expr);
            if let Some(diverge) = &mut init.diverge {
                syn::visit_mut::visit_expr_mut(self, &mut diverge.1);
            }
        }
        self.bind_pat(&mut local.pat);
        self.constants_observed = observed;
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
        let binding = self.rename_binder(Ns::Type, &mut item.ident);
        let id = self.alloc_id();
        let canon = self
            .local_canons
            .get(&binding)
            .cloned()
            .expect("a local binding");
        self.local_canons.insert(id, canon.clone());
        self.top_bind(
            Ns::Type,
            "Self",
            Binding {
                id,
                canon,
                origin: Origin::SelfTy,
                prim: None,
                port: 0,
                value_port: None,
                target: None,
                raw: false,
                helper: None,
            },
        );
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
        let outer_owner = self.self_owner.take();
        if self.analysis {
            // Prove the terminal owner through the original module frames,
            // so that `Self` keeps the owner's original identity and every
            // spelling shares one proof-backed route.
            if let syn::Type::Path(ty) = item.self_ty.as_ref()
                && ty.qself.is_none()
                && let Some((terminal, route)) = self.owner_route(&ty.path)
            {
                self.self_owner = Some(terminal);
                self.facts
                    .owner_routes
                    .insert((terminal.domain == Domain::Inherited, terminal.id), route);
            }
        } else if let Some(name) = single_segment_type_name(&item.self_ty)
            && let Some(resolved) = self.lookup(&[Ns::Type], &name)
        {
            let id = self.alloc_id();
            let canon = self.render(&resolved);
            self.local_canons.insert(id, canon.clone());
            self.top_bind(
                Ns::Type,
                "Self",
                Binding {
                    id,
                    canon,
                    origin: Origin::SelfTy,
                    prim: None,
                    port: 0,
                    value_port: None,
                    target: None,
                    raw: false,
                    helper: None,
                },
            );
        }
        let has_generics = item.generics.lt_token.is_some();
        if has_generics {
            begin_generics(self, &mut item.generics);
        }
        self.visit_attrs(&mut item.attrs);
        if let Some((path, _)) = &mut item.trait_ {
            // The trait path of `impl Trait for Type` is a bare path in type position.
            let outer = core::mem::replace(&mut self.in_type, true);
            self.visit_path_mut(path);
            self.in_type = outer;
        }
        syn::visit_mut::visit_type_mut(self, &mut item.self_ty);
        if self.analysis
            && let Some(origin) = self.self_owner
            && let syn::Type::Path(ty) = item.self_ty.as_ref()
            && let (Some(first), Some(last)) = (ty.path.segments.first(), ty.path.segments.last())
        {
            let resolution = Resolution {
                kind: if origin.domain == Domain::Inherited {
                    ReferenceKind::Inherited
                } else {
                    ReferenceKind::Owned
                },
                prim: None,
                origin: Some(origin),
            };
            self.facts
                .idents
                .insert(core::ptr::from_ref(&last.ident), origin);
            self.facts
                .paths
                .insert(core::ptr::from_ref(&ty.path), resolution);
            self.facts
                .original_paths
                .insert(core::ptr::from_ref(first), resolution);
        }
        let resolutions = type_resolutions(&self.facts, &item.self_ty);
        let outer_resolutions = core::mem::replace(&mut self.self_ty_resolutions, resolutions);
        let outer = self.self_ty.take();
        if !self.context {
            self.self_ty = Some(item.self_ty.as_ref().clone());
        }
        for impl_item in &mut item.items {
            syn::visit_mut::visit_impl_item_mut(self, impl_item);
        }
        self.self_ty = outer;
        self.self_ty_resolutions = outer_resolutions;
        self.self_owner = outer_owner;
        if has_generics {
            self.pop();
        }
        self.pop();
    }

    fn visit_item_const_mut(&mut self, item: &mut syn::ItemConst) {
        self.rename_binder(Ns::Value, &mut item.ident);
        if self.constants_allowed() {
            let expected = self.param_prim(&item.ty);
            crate::constants::normalize_expr(self, &mut item.expr, expected);
        }
        self.visit_attrs(&mut item.attrs);
        syn::visit_mut::visit_type_mut(self, &mut item.ty);
        syn::visit_mut::visit_expr_mut(self, &mut item.expr);
    }

    fn visit_item_static_mut(&mut self, item: &mut syn::ItemStatic) {
        self.rename_binder(Ns::Value, &mut item.ident);
        if self.constants_allowed() {
            let expected = self.param_prim(&item.ty);
            crate::constants::normalize_expr(self, &mut item.expr, expected);
        }
        self.visit_attrs(&mut item.attrs);
        syn::visit_mut::visit_type_mut(self, &mut item.ty);
        syn::visit_mut::visit_expr_mut(self, &mut item.expr);
    }

    fn visit_impl_item_const_mut(&mut self, item: &mut syn::ImplItemConst) {
        let observed = self.constants_observed;
        self.constants_observed |= crate::context::live_attr(&item.attrs);
        if self.constants_allowed() {
            let expected = self.param_prim(&item.ty);
            crate::constants::normalize_expr(self, &mut item.expr, expected);
        }
        syn::visit_mut::visit_impl_item_const_mut(self, item);
        self.constants_observed = observed;
    }

    fn visit_trait_item_const_mut(&mut self, item: &mut syn::TraitItemConst) {
        let observed = self.constants_observed;
        self.constants_observed |= crate::context::live_attr(&item.attrs);
        if self.constants_allowed()
            && let Some((_, expr)) = &mut item.default
        {
            let expected = self.param_prim(&item.ty);
            crate::constants::normalize_expr(self, expr, expected);
        }
        syn::visit_mut::visit_trait_item_const_mut(self, item);
        self.constants_observed = observed;
    }

    fn visit_const_param_mut(&mut self, param: &mut syn::ConstParam) {
        let observed = self.constants_observed;
        self.constants_observed |= crate::context::live_attr(&param.attrs);
        if self.constants_allowed()
            && let Some((_, expr)) = &mut param.default
        {
            let expected = self.param_prim(&param.ty);
            crate::constants::normalize_expr(self, expr, expected);
        }
        syn::visit_mut::visit_const_param_mut(self, param);
        self.constants_observed = observed;
    }

    fn visit_item_macro_mut(&mut self, item: &mut syn::ItemMacro) {
        if let Some(ident) = item.ident.as_mut() {
            self.bind_ident(Ns::Macro, Origin::MacroRule, ident);
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
        let rooted = item.leading_colon.is_some();
        let glob = syn::UseTree::Glob(syn::UseGlob {
            star_token: <syn::Token![*]>::default(),
        });
        if self.analysis {
            let _ = self.bind_use_tree(rooted, &[], item.tree.clone());
        } else {
            item.tree = self.bind_use_tree(rooted, &[], core::mem::replace(&mut item.tree, glob));
        }
    }

    fn visit_item_mod_mut(&mut self, item: &mut syn::ItemMod) {
        let previous_module = if let Some(route) = self
            .helper_syntax
            .module(item)
            .and_then(|route| self.helper_table.module_route(route))
        {
            let route = route.to_vec();
            Some(core::mem::replace(&mut self.helper_module, route))
        } else {
            self.helper_module
                .push(crate::scope::ident_name(&item.ident));
            None
        };
        let id = self.rename_binder(Ns::Type, &mut item.ident);
        self.visit_attrs(&mut item.attrs);
        if let Some((_, items)) = &mut item.content {
            let frame = self.mod_frames.remove(&id).unwrap_or_default();
            let primitive = self.prim_fallback;
            self.prim_fallback = crate::context::scope_primitives_known(items);
            let observed = self.observed;
            self.observed |= crate::context::live_attr(&item.attrs);
            self.frames.push(frame);
            for sub_item in items.iter_mut() {
                self.visit_item_mut(sub_item);
            }
            let frame = self.frames.pop().expect("a scope frame");
            self.prim_fallback = primitive;
            self.observed = observed;
            self.mod_frames.insert(id, frame);
        }
        if let Some(previous) = previous_module {
            self.helper_module = previous;
        } else {
            self.helper_module.pop();
        }
    }

    fn visit_expr_closure_mut(&mut self, closure: &mut syn::ExprClosure) {
        self.push();
        if let Some(bound) = &mut closure.lifetimes {
            self.visit_bound_lifetimes_mut(bound);
        }
        for input in &mut closure.inputs {
            let prim = match input {
                syn::Pat::Type(pat_type) => self.param_prim(&pat_type.ty),
                _ => None,
            };
            let mut names = Vec::new();
            pattern_names(input, &mut names);
            for name in &names {
                self.bind_full(Ns::Value, name, Origin::Closure, prim, 0, None);
            }
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
        if self.regions_active && self.values_allowed() && self.macro_origins.is_none() {
            crate::structure::literal_alternatives(arm);
        }
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
        if self.regions_active && self.values_allowed() && self.macro_origins.is_none() {
            crate::structure::named_fields(self, expr);
        }
        // A shorthand `S { x }` prints only the member, so a bound `x`
        // takes the explicit form before the rewrite.
        for field in &mut expr.fields {
            if !self.analysis
                && field.colon_token.is_none()
                && let syn::Member::Named(ident) = &field.member
                && self.lookup(&[Ns::Value], &ident.to_string()).is_some()
            {
                field.colon_token = Some(syn::token::Colon::default());
            }
        }
        let outer = core::mem::replace(&mut self.in_type, true);
        syn::visit_mut::visit_expr_struct_mut(self, expr);
        self.in_type = outer;
    }

    fn visit_expr_call_mut(&mut self, call: &mut syn::ExprCall) {
        let helper = if self.compiles
            && self.regions_active
            && self.macro_origins.is_none()
            && self.helpers_allowed()
        {
            match call.func.as_ref() {
                syn::Expr::Path(path)
                    if path.qself.is_none()
                        && self
                            .helper_path_scope(&path.path)
                            .is_none_or(|scope| self.helper_table.scope_known(scope)) =>
                {
                    let helper = if let Some(ident) = path.path.get_ident() {
                        self.lookup(&[Ns::Value], &ident.to_string())
                            .and_then(|binding| binding.helper)
                    } else {
                        self.helper_id(&BTreeMap::new(), &path.path)
                    };
                    helper.filter(|helper| {
                        self.helper_table.get(*helper).is_some()
                            && self.helper_canon(*helper).is_some()
                    })
                }
                _ => None,
            }
        } else {
            None
        };
        syn::visit_mut::visit_expr_call_mut(self, call);
        if let Some(helper) = helper
            && let syn::Expr::Path(path) = call.func.as_mut()
        {
            let transport = self.helper_syntax.take_path_transport(&path.path);
            while path.path.segments.len() > 1 {
                path.path.segments.pop();
            }
            path.path.segments.pop_punct();
            path.path.leading_colon = None;
            rename(
                &mut path.path.segments.first_mut().expect("a helper path").ident,
                self.helper_canon(helper)
                    .expect("an admitted helper identity"),
            );
            self.helper_syntax.put_path_transport(&path.path, transport);
            self.facts.paths.insert(
                core::ptr::from_ref(&path.path),
                Resolution {
                    kind: if self.helper_canons.contains_key(&helper) {
                        ReferenceKind::Owned
                    } else {
                        ReferenceKind::Inherited
                    },
                    prim: None,
                    origin: None,
                },
            );
        }
    }

    fn visit_expr_path_mut(&mut self, expr: &mut syn::ExprPath) {
        let outer = core::mem::replace(&mut self.in_type, false);
        syn::visit_mut::visit_expr_path_mut(self, expr);
        self.in_type = outer;
    }

    fn visit_pat_struct_mut(&mut self, pat: &mut syn::PatStruct) {
        let outer = core::mem::replace(&mut self.in_type, true);
        syn::visit_mut::visit_pat_struct_mut(self, pat);
        self.in_type = outer;
    }

    fn visit_pat_tuple_struct_mut(&mut self, pat: &mut syn::PatTupleStruct) {
        let outer = core::mem::replace(&mut self.in_type, false);
        syn::visit_mut::visit_pat_tuple_struct_mut(self, pat);
        self.in_type = outer;
    }

    fn visit_qself_mut(&mut self, qself: &mut syn::QSelf) {
        // syn visits every qualified path's `<T>` right before its path.
        syn::visit_mut::visit_qself_mut(self, qself);
        self.after_bare_qself = qself.position == 0;
    }

    fn visit_path_mut(&mut self, path: &mut syn::Path) {
        let handles = self.helper_syntax.take_path(path);
        // Only the first segment names a local binder, and a leading
        // colon always names an extern crate.
        if !core::mem::take(&mut self.after_bare_qself) {
            self.resolve_path(path);
        }
        self.visit_path_arguments(path);
        self.helper_syntax.put_path(path, handles);
    }

    fn visit_macro_mut(&mut self, mac: &mut syn::Macro) {
        self.unroot(
            &mut mac.path.leading_colon,
            mac.path.segments.first().map(|s| &s.ident),
        );
        let ns: &[Ns] = if mac.path.segments.len() > 1 {
            &[Ns::Type]
        } else {
            &[Ns::Macro]
        };
        self.record_path(&mac.path, ns, ns);
        let known = mac.path.segments.first().is_some_and(|first| {
            let head = crate::scope::ident_name(&first.ident);
            if crate::drift::format_operand(&mac.path).is_none() || self.macro_shadowed(&head) {
                return false;
            }
            if mac.path.segments.len() == 1 {
                return true;
            }
            let leaf =
                crate::scope::ident_name(&mac.path.segments.last().expect("a macro path").ident);
            mac.path.segments.len() == 2
                && self.portable_first(&[Ns::Type], first)
                && (head == "std"
                    || (head == "alloc" && leaf == "format")
                    || (head == "core" && leaf == "format_args"))
        });
        let parent = self.macro_origins.replace(Vec::new());
        let parent_supported = core::mem::replace(&mut self.macro_supported, true);
        if let Some(first) = mac.path.segments.first_mut() {
            self.resolve(ns, &mut first.ident);
        }
        let format_at = crate::drift::format_operand(&mac.path).map(|(at, _)| at);
        let outer = core::mem::replace(&mut self.opaque_tokens, !known);
        if self.analysis {
            let _ = self.rewrite_macro_tokens(mac.tokens.clone(), format_at);
        } else {
            mac.tokens = self.rewrite_macro_tokens(core::mem::take(&mut mac.tokens), format_at);
        }
        self.opaque_tokens = outer;
        let origins = self.macro_origins.take().expect("a macro origin collector");
        let supported = known && self.macro_supported;
        self.macro_supported = if parent.is_some() {
            parent_supported && supported
        } else {
            parent_supported
        };
        if self.analysis
            && supported
            && parent.is_none()
            && let Some(first) = mac.path.segments.first()
        {
            self.facts.known_macros.insert(core::ptr::from_ref(first));
        }
        if let Some(mut parent) = parent {
            parent.extend(origins);
            self.macro_origins = Some(parent);
        } else {
            if self.analysis
                && let Some(first) = mac.path.segments.first()
            {
                self.facts
                    .original_macros
                    .insert(core::ptr::from_ref(first), origins.clone());
            }
            self.facts.macros.insert(core::ptr::from_ref(mac), origins);
        }
    }

    fn visit_type_path_mut(&mut self, node: &mut syn::TypePath) {
        // Type positions read the type namespace only.
        let outer = core::mem::replace(&mut self.in_type, true);
        syn::visit_mut::visit_type_path_mut(self, node);
        self.in_type = outer;
    }

    fn visit_type_mut(&mut self, ty: &mut syn::Type) {
        if !self.analysis
            && let Some(self_ty) = &self.self_ty
            && let syn::Type::Path(path) = &*ty
            && path.qself.is_none()
            && path.path.is_ident("Self")
        {
            *ty = self_ty.clone();
            restore_type(&mut self.facts, ty, &self.self_ty_resolutions);
            return;
        }
        let observed = self.constants_observed;
        self.constants_observed |= !crate::drift::type_attrs(ty).is_empty();
        syn::visit_mut::visit_type_mut(self, ty);
        self.constants_observed = observed;
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
                self.bind_ident(Ns::Lifetime, Origin::Lifetime, &mut lifetime.lifetime.ident);
            }
        }
        syn::visit_mut::visit_bound_lifetimes_mut(self, node);
    }

    fn visit_type_fn_ptr_mut(&mut self, node: &mut syn::TypeFnPtr) {
        // A `for<'a>` binder lives in its own frame.
        self.push();
        syn::visit_mut::visit_type_fn_ptr_mut(self, node);
        self.pop();
    }

    fn visit_trait_bound_mut(&mut self, node: &mut syn::TraitBound) {
        // Same owner scoping for `for<'a>`, and the bound path is a type position.
        self.push();
        let outer = core::mem::replace(&mut self.in_type, true);
        let nominal = self.context
            && node
                .path
                .segments
                .first()
                .and_then(|first| self.lookup(&[Ns::Type], &first.ident.to_string()))
                .is_some_and(|resolved| {
                    resolved.domain == Domain::Inherited && resolved.target.is_none()
                });
        if nominal {
            if let Some(lifetimes) = &mut node.lifetimes {
                self.visit_bound_lifetimes_mut(lifetimes);
            }
            self.visit_path_arguments(&mut node.path);
        } else {
            syn::visit_mut::visit_trait_bound_mut(self, node);
        }
        self.in_type = outer;
        self.pop();
    }

    fn visit_named_arg_mut(&mut self, node: &mut syn::NamedArg) {
        // Parameter names in a function pointer type bind nothing.
        node.name = None;
        syn::visit_mut::visit_named_arg_mut(self, node);
    }

    fn visit_attribute_mut(&mut self, attribute: &mut syn::Attribute) {
        let prepared = core::mem::replace(&mut self.constants_prepared, true);
        let observed = core::mem::replace(&mut self.constants_observed, true);
        // Attribute paths name external items, but a `#[attr = expr]`
        // value may reference the local bindings.
        if let syn::Meta::NameValue(name_value) = &mut attribute.meta {
            syn::visit_mut::visit_expr_mut(self, &mut name_value.value);
        }
        self.constants_prepared = prepared;
        self.constants_observed = observed;
    }
}
