//! Shared alpha-renaming for whole files, lexical contexts, and seeded views.

use alloc::boxed::Box;
use alloc::collections::BTreeMap;
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

use crate::reference::{ReferenceFacts, Resolution};
use crate::scope::{
    Binding, BindingId, Domain, FirstSeg, Frame, Inherited, Ns, Origin, Target, TargetSeg,
    new_ident,
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
}

/// A resolved name reference, its identity only, the canon resolves on render.
#[derive(Clone, Copy)]
struct Resolved {
    domain: Domain,
    ns: Ns,
    id: BindingId,
    target: Option<usize>,
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
    *ident = new_ident(canon, ident.span());
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

/// Resolve a `use` root while retaining opaque qualified suffixes.
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
    let Some((leaf, prefix)) = path.split_last() else {
        return (rooted, Vec::new());
    };
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
    /// A type position, where a path reads the type namespace only.
    in_type: bool,
    /// A bare `<T>` was just visited, so the next path names associated items of `T`.
    after_bare_qself: bool,
    /// Raw-involved identifiers in macro input keep their token spelling.
    opaque_tokens: bool,
    /// The resolutions collected for the macro arguments being rewritten.
    macro_origins: Option<Vec<Resolution>>,
    /// The reference provenance the pass recorded.
    facts: ReferenceFacts,
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
            in_type: false,
            after_bare_qself: false,
            opaque_tokens: false,
            macro_origins: None,
            facts: ReferenceFacts::default(),
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
        let next_id = inherited_max_id(&inherited);
        let mut renamer = Self::base(true, inherited);
        renamer.next_id = next_id;
        renamer
    }

    /// A file pass with the optional seeded environment of a function view.
    fn seeded(
        seed: Option<&'env Frame>,
        generics: Option<&'env Frame>,
        self_canon: Option<String>,
    ) -> Self {
        let mut renamer = Self::new();
        if let Some(frame) = seed {
            renamer.seed = Some(frame);
            renamer.next_id = renamer.next_id.max(frame.next_id());
        }
        if let Some(frame) = generics {
            renamer.generics = Some(frame);
            renamer.next_id = renamer.next_id.max(frame.next_id());
        }
        if let Some(canon) = self_canon {
            let id = BindingId(renamer.next_id);
            renamer.next_id += 1;
            let mut frame = Frame::default();
            frame.bind(
                Ns::Type,
                "Self",
                Binding {
                    id,
                    canon,
                    origin: Origin::SelfTy,
                    target: None,
                    raw: false,
                },
            );
            renamer.self_frame = Some(frame);
        }
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

    /// Bind `name` in the current frame and return the new binding.
    fn bind(&mut self, ns: Ns, name: &str, origin: Origin) -> Binding {
        let id = self.alloc_id();
        let canon = self.local_canon(ns);
        let binding = Binding {
            id,
            canon: canon.clone(),
            origin,
            target: None,
            raw: false,
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
            target,
            raw: false,
        };
        self.local_canons.insert(id, canon);
        self.top_bind(Ns::Value, name, binding.clone());
        self.top_bind(Ns::Type, name, binding.clone());
        binding
    }

    /// Bind `ident` in the current frame and rename it.
    fn bind_ident(&mut self, ns: Ns, origin: Origin, ident: &mut Ident) {
        let name = ident.to_string();
        let binding = self.bind(ns, &name, origin);
        rename(ident, &binding.canon);
    }

    /// Rename `ident` to the canon it already holds in the current frame,
    /// else bind a new one.
    fn rename_binder(&mut self, ns: Ns, origin: Origin, ident: &mut Ident) -> BindingId {
        let name = ident.to_string();
        if let Some(binding) = self.frames.last().and_then(|frame| frame.lookup(ns, &name)) {
            rename(ident, &binding.canon);
            binding.id
        } else {
            let binding = self.bind(ns, &name, origin);
            rename(ident, &binding.canon);
            binding.id
        }
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
                    self.inherited_canon(resolved.id)
                }
            }
        }
    }

    /// The seed canonical name of an inherited binding id, scanned across
    /// the inherited frames.
    fn inherited_canon(&self, id: BindingId) -> String {
        for frame in self
            .self_frame
            .iter()
            .chain(self.generics)
            .chain(self.seed)
            .chain(self.inherited.frames.iter())
        {
            for ns in [Ns::Value, Ns::Type, Ns::Lifetime, Ns::Label, Ns::Macro] {
                if let Some(binding) = frame.bindings(ns).values().find(|binding| binding.id == id)
                {
                    return binding.canon.clone();
                }
            }
        }
        unreachable!("an inherited binding resolved by lookup")
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
            target: binding.target,
        }
    }

    /// Nearest-binding lookup across the local and inherited frames,
    /// namespace priority first.
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

    /// The provenance of the reference retained by normalization.
    fn resolution(&self, ns: &[Ns], name: &str) -> Resolution {
        let Some(resolved) = self.lookup(ns, name) else {
            return Resolution::Unresolved;
        };
        let domain = if let Some(index) = resolved.target {
            let target = self.target_of(resolved.domain, index);
            if target.rooted {
                return Resolution::Unresolved;
            }
            let Some((domain, _, _)) = target.segments.first().and_then(|segment| segment.origin)
            else {
                return Resolution::Unresolved;
            };
            if domain == Domain::Inherited {
                return Resolution::Unresolved;
            }
            domain
        } else {
            resolved.domain
        };
        match domain {
            Domain::Local => Resolution::Owned,
            Domain::Inherited => Resolution::Inherited,
        }
    }

    /// Record one path's resolution, into the macro collector when nested.
    fn record_path(&mut self, path: &syn::Path, ns: &[Ns]) {
        let resolution = path
            .segments
            .first()
            .filter(|_| path.leading_colon.is_none())
            .map(|head| self.resolution(ns, &head.ident.to_string()))
            .unwrap_or_default();
        if let Some(origins) = &mut self.macro_origins {
            origins.push(resolution);
        } else {
            self.facts
                .paths
                .insert(core::ptr::from_ref(path), resolution);
        }
    }

    /// Record one token's resolution, into the macro collector only.
    fn record_token(&mut self, ns: &[Ns], name: &str) {
        if self.macro_origins.is_none() {
            return;
        }
        let resolution = self.resolution(ns, name);
        if let Some(origins) = &mut self.macro_origins {
            origins.push(resolution);
        }
    }

    /// Rename a bare reference through its nearest binder, if any.
    fn resolve(&mut self, ns: &[Ns], ident: &mut Ident) {
        if let Some(resolved) = self.lookup(ns, &ident.to_string())
            && resolved.target.is_none()
        {
            rename(ident, &self.render(&resolved));
        }
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
        if !leading_colon.is_some() {
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
        let ns: &[Ns] = if self.in_type {
            &[Ns::Type]
        } else {
            &[Ns::Value, Ns::Type]
        };
        self.record_path(path, ns);
        if path.leading_colon.is_some() {
            return;
        }
        let Some(resolved) = path
            .segments
            .first()
            .and_then(|first| self.lookup(ns, &first.ident.to_string()))
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
        segments: impl Iterator<Item = &'a mut syn::PathSegment>,
    ) {
        for segment in segments {
            let hit = current.and_then(|(domain, id)| {
                self.module_frame(domain, id)
                    .and_then(|frame| {
                        module_member(frame, ns, &segment.ident.to_string(), self.opaque_tokens)
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
                    self.record_token(ns, &ident.to_string());
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
            glob @ syn::UseTree::Glob(_) => glob,
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
        let target = self.context.then(|| {
            let mut path = prefix.to_vec();
            path.push(ident.to_string());
            let (rooted, segments) =
                resolve_target_path(&|name| self.resolve_first_seg(name), rooted, &path);
            self.store_target(rooted, segments)
        });
        let binding = self.bind_value_and_type(&alias.to_string(), target, Origin::Use);
        syn::UseTree::Rename(syn::UseRename {
            rename: Ident::new(&binding.canon, alias.span()),
            ident: ident.clone(),
            as_token: <syn::Token![as]>::default(),
        })
    }

    /// Bind every name `pat` binds, then visit and rewrite it.
    fn bind_pat(&mut self, pat: &mut syn::Pat) {
        let mut names = Vec::new();
        pattern_names(pat, &mut names);
        for name in &names {
            self.bind(Ns::Value, name, Origin::Let);
        }
        syn::visit_mut::visit_pat_mut(self, pat);
        rewrite_pat_bindings(self, pat);
    }

    /// A function's generics frame and parameter frame around its
    /// signature and body.
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
            self.bind(Ns::Value, &param, Origin::Parameter);
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
        self.rename_binder(Ns::Type, Origin::Item, ident);
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

    /// The reference provenance the pass recorded, consumed after the visit.
    pub(crate) fn into_reference_facts(self) -> ReferenceFacts {
        self.facts
    }
}

/// The largest binding id across the inherited frames, the local ids start
/// above it.
fn inherited_max_id(inherited: &Inherited<'_>) -> usize {
    inherited
        .frames
        .iter()
        .chain(inherited.mod_frames.values())
        .map(Frame::next_id)
        .max()
        .unwrap_or(0)
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
            if let Some(origins) = &mut renamer.macro_origins {
                origins.push(Resolution::Unresolved);
            }
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
pub(crate) fn normalize_file(file: &mut syn::File, options: Options<'_>) -> ReferenceFacts {
    let mut renamer = Renamer::seeded(options.seed, options.generics, options.self_canon);
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

/// Call `f` on every identifier pattern of `pat`, in traversal order. A
/// shorthand `S { x }` binds the member name and drops the member when
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
fn rewrite_pat_bindings(renamer: &mut Renamer<'_>, pat: &mut syn::Pat) {
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

/// The origin of an item the prebind recognizes.
fn item_origin(item: &syn::Item) -> Origin {
    match item {
        syn::Item::Type(_) => Origin::TypeAlias,
        _ => Origin::Item,
    }
}

/// Pre-bind item names so a use may precede its definition, skipping
/// `macro_rules!` and `use`, which are visible only after their line.
fn prebind<'a>(renamer: &mut Renamer<'_>, items: impl Iterator<Item = &'a syn::Item>) {
    for item in items {
        let Some((ns, ident)) = item_binding(item) else {
            continue;
        };
        let origin = item_origin(item);
        let binding = renamer.bind(ns, &ident.to_string(), origin);
        if let syn::Item::Mod(syn::ItemMod {
            content: Some((_, items)),
            ..
        }) = item
        {
            renamer.push();
            prebind(renamer, items.iter());
            let frame = renamer.frames.pop().expect("a scope frame");
            renamer.mod_frames.insert(binding.id, frame);
        }
    }
}

/// The member of a module frame that `name` names, in namespace priority
/// order.
fn module_member(
    frame: &Frame,
    ns: &[Ns],
    name: &str,
    opaque_tokens: bool,
) -> Option<(BindingId, Ns)> {
    for ns in ns {
        if let Some(bind) = frame.lookup(*ns, name) {
            return (!opaque_tokens || (!bind.raw && !name.starts_with("r#")))
                .then_some((bind.id, *ns));
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
                rename(&mut r#const.ident, &binding.canon);
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
    }
    let mut restore = Restore {
        facts,
        resolutions: resolutions.iter(),
    };
    restore.visit_type(ty);
}

impl VisitMut for Renamer<'_> {
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
        // The initializer resolves under the outer bindings.
        if let Some(init) = &mut local.init {
            syn::visit_mut::visit_expr_mut(self, &mut init.expr);
            if let Some(diverge) = &mut init.diverge {
                syn::visit_mut::visit_expr_mut(self, &mut diverge.1);
            }
        }
        self.bind_pat(&mut local.pat);
    }

    fn visit_item_fn_mut(&mut self, item: &mut syn::ItemFn) {
        self.rename_binder(Ns::Value, Origin::Item, &mut item.sig.ident);
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
        let binding = self.rename_binder(Ns::Type, Origin::Item, &mut item.ident);
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
                target: None,
                raw: false,
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
        if let Some(name) = single_segment_type_name(&item.self_ty)
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
                    target: None,
                    raw: false,
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
        if has_generics {
            self.pop();
        }
        self.pop();
    }

    fn visit_item_const_mut(&mut self, item: &mut syn::ItemConst) {
        self.rename_binder(Ns::Value, Origin::Item, &mut item.ident);
        self.visit_attrs(&mut item.attrs);
        syn::visit_mut::visit_type_mut(self, &mut item.ty);
        syn::visit_mut::visit_expr_mut(self, &mut item.expr);
    }

    fn visit_item_static_mut(&mut self, item: &mut syn::ItemStatic) {
        self.rename_binder(Ns::Value, Origin::Item, &mut item.ident);
        self.visit_attrs(&mut item.attrs);
        syn::visit_mut::visit_type_mut(self, &mut item.ty);
        syn::visit_mut::visit_expr_mut(self, &mut item.expr);
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
        item.tree = self.bind_use_tree(rooted, &[], core::mem::replace(&mut item.tree, glob));
    }

    fn visit_item_mod_mut(&mut self, item: &mut syn::ItemMod) {
        let id = self.rename_binder(Ns::Type, Origin::Item, &mut item.ident);
        self.visit_attrs(&mut item.attrs);
        if let Some((_, items)) = &mut item.content {
            let frame = self.mod_frames.remove(&id).unwrap_or_default();
            self.frames.push(frame);
            for sub_item in items.iter_mut() {
                syn::visit_mut::visit_item_mut(self, sub_item);
            }
            let frame = self.frames.pop().expect("a scope frame");
            self.mod_frames.insert(id, frame);
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
            self.bind(Ns::Value, name, Origin::Closure);
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

    fn visit_qself_mut(&mut self, qself: &mut syn::QSelf) {
        // syn visits every qualified path's `<T>` right before its path.
        syn::visit_mut::visit_qself_mut(self, qself);
        self.after_bare_qself = qself.position == 0;
    }

    fn visit_path_mut(&mut self, path: &mut syn::Path) {
        // Only the first segment names a local binder, and a leading
        // colon always names an extern crate.
        if !core::mem::take(&mut self.after_bare_qself) {
            self.resolve_path(path);
        }
        self.visit_path_arguments(path);
    }

    fn visit_macro_mut(&mut self, mac: &mut syn::Macro) {
        self.unroot(
            &mut mac.path.leading_colon,
            mac.path.segments.first().map(|s| &s.ident),
        );
        self.record_path(&mac.path, &[Ns::Macro]);
        let parent = self.macro_origins.replace(Vec::new());
        if let Some(first) = mac.path.segments.first_mut() {
            self.resolve(&[Ns::Macro], &mut first.ident);
        }
        let format_at = crate::drift::format_operand(&mac.path).map(|(at, _)| at);
        let outer = core::mem::replace(&mut self.opaque_tokens, true);
        mac.tokens = self.rewrite_macro_tokens(core::mem::take(&mut mac.tokens), format_at);
        self.opaque_tokens = outer;
        let origins = self.macro_origins.take().expect("a macro origin collector");
        if let Some(mut parent) = parent {
            parent.extend(origins);
            self.macro_origins = Some(parent);
        } else {
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
        if let Some(self_ty) = &self.self_ty
            && let syn::Type::Path(path) = &*ty
            && path.qself.is_none()
            && path.path.is_ident("Self")
        {
            *ty = self_ty.clone();
            restore_type(&mut self.facts, ty, &self.self_ty_resolutions);
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
        // Attribute paths name external items, but a `#[attr = expr]`
        // value may reference the local bindings.
        if let syn::Meta::NameValue(name_value) = &mut attribute.meta {
            syn::visit_mut::visit_expr_mut(self, &mut name_value.value);
        }
    }
}
