//! Immutable total primitive helper proofs over original lexical namespaces.

use alloc::collections::{BTreeMap, BTreeSet, VecDeque};
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec;
use alloc::vec::Vec;
use syn::visit::{self, Visit};

use crate::alpha::Renamer;
use crate::context::{
    ModuleScope, UseTarget, blocked_modules, blocking_attr, blocking_meta, qualified_name,
    register_inline, resolve_use, scope_primitives_known, seed_frame, seed_items,
};
use crate::schedule::{
    Lbl, MAX_BLOCK_DEPTH, MAX_REGION_NODES, Port, TotalBinary, Value, total_binary,
};
use crate::scope::{
    Binding, BindingId, Frame, HelperId, Ns, Origin, PrimTy, ident_name, primitive_suffix,
};

const SATURATED_NODES: usize = MAX_REGION_NODES + 1;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct TableFacts {
    pub(crate) params: Vec<PrimTy>,
    pub(crate) result: PrimTy,
    pub(crate) nodes: usize,
    pub(crate) deps: Vec<HelperId>,
}

#[derive(Clone)]
struct Entry {
    canon: String,
    facts: Option<TableFacts>,
    exhausted: bool,
}

#[derive(Clone, Default)]
struct ModuleNames {
    values: BTreeMap<String, HelperId>,
    modules: BTreeMap<String, Vec<String>>,
}

/// A compact index of an original scope environment.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) struct ScopeId(usize);

#[derive(Clone, Default)]
pub(crate) struct HelperTable {
    entries: Vec<Entry>,
    canons: BTreeMap<String, HelperId>,
    modules: BTreeMap<Vec<String>, ModuleNames>,
    declarations: BTreeMap<*const syn::Block, HelperId>,
    block_scopes: BTreeMap<*const syn::Block, ScopeId>,
    frames: Vec<Environment>,
    module_canons: BTreeMap<String, Vec<String>>,
    inherited: Vec<bool>,
    calls: BTreeMap<*const syn::Path, ScopeId>,
}

impl HelperTable {
    pub(crate) fn get(&self, id: HelperId) -> Option<&TableFacts> {
        self.entries.get(id.0)?.facts.as_ref()
    }

    pub(crate) fn has_domain(&self, id: HelperId) -> bool {
        self.entries
            .get(id.0)
            .is_some_and(|entry| entry.facts.is_some() || entry.exhausted)
    }

    pub(crate) fn by_name(&self, module: &[String], name: &str) -> Option<HelperId> {
        self.modules
            .get(module)?
            .values
            .get(crate::scope::unraw(name))
            .copied()
    }

    pub(crate) fn by_canon(&self, canon: &str) -> Option<HelperId> {
        self.canons.get(canon).copied()
    }

    pub(crate) fn declaration(&self, body: &syn::Block) -> Option<HelperId> {
        self.declarations.get(&core::ptr::from_ref(body)).copied()
    }

    pub(crate) fn declarations(&self) -> &BTreeMap<*const syn::Block, HelperId> {
        &self.declarations
    }

    pub(crate) fn closure_nodes(&self, roots: &BTreeSet<HelperId>) -> usize {
        let mut seen = BTreeSet::new();
        let mut pending = Vec::new();
        let mut nodes = 0usize;
        for &root in roots {
            if !seen.insert(root) {
                continue;
            }
            pending.push(root);
            while let Some(id) = pending.pop() {
                let Some(facts) = self.get(id) else {
                    return SATURATED_NODES;
                };
                nodes = nodes.saturating_add(facts.nodes);
                if nodes > MAX_REGION_NODES {
                    return SATURATED_NODES;
                }
                for &dep in &facts.deps {
                    if seen.insert(dep) {
                        pending.push(dep);
                    }
                }
            }
        }
        nodes
    }

    pub(crate) fn resolve(&self, from: &[String], path: &syn::Path) -> Option<HelperId> {
        if path.leading_colon.is_some()
            || path.segments.is_empty()
            || path.segments.len() > MAX_BLOCK_DEPTH
            || path
                .segments
                .iter()
                .any(|seg| !matches!(seg.arguments, syn::PathArguments::None))
        {
            return None;
        }
        let names: Vec<String> = path
            .segments
            .iter()
            .map(|seg| ident_name(&seg.ident))
            .collect();
        self.resolve_names(from, &names)
    }

    fn resolve_names(&self, from: &[String], names: &[String]) -> Option<HelperId> {
        let (last, prefixes) = names.split_last()?;
        if prefixes.is_empty() {
            return self.by_name(from, last);
        }
        let mut module = from;
        for (index, part) in prefixes.iter().enumerate() {
            match part.as_str() {
                "crate" if index == 0 => module = &[],
                "self" if index == 0 => {}
                "super" if index == 0 || prefixes[..index].iter().all(|name| name == "super") => {
                    module = module.split_last()?.1;
                }
                _ => module = self.modules.get(module)?.modules.get(part)?,
            }
        }
        self.by_name(module, last)
    }

    pub(crate) fn scopes(&self) -> &BTreeMap<*const syn::Block, ScopeId> {
        &self.block_scopes
    }

    pub(crate) fn call_scopes(&self) -> &BTreeMap<*const syn::Path, ScopeId> {
        &self.calls
    }

    pub(crate) fn scope(&self, id: ScopeId) -> Option<&Frame> {
        self.frames.get(id.0).map(|environment| &environment.frame)
    }

    pub(crate) fn scope_known(&self, id: ScopeId) -> bool {
        self.frames[id.0].prim_fallback
    }

    pub(crate) fn inherited_canon(&self, id: HelperId) -> Option<&str> {
        let entry = self.entries.get(id.0)?;
        self.inherited
            .get(id.0)
            .is_some_and(|inherited| *inherited)
            .then_some(entry.canon.as_str())
    }

    pub(crate) fn resolve_in_scope(&self, scope: ScopeId, path: &syn::Path) -> Option<HelperId> {
        let environment = self.frames.get(scope.0)?;
        let frame = &environment.frame;
        let names: Vec<String> = path
            .segments
            .iter()
            .map(|seg| ident_name(&seg.ident))
            .collect();
        if !matches!(names[0].as_str(), "crate" | "self" | "super") {
            let binding = frame.lookup(Ns::Type, &names[0])?;
            let module = self.module_canons.get(&binding.canon)?;
            return self.resolve_names(module, &names[1..]);
        }
        self.resolve_names(&environment.module, &names)
    }
}

pub(crate) struct Scopes<'a> {
    modules: BTreeMap<Vec<String>, ModuleScope<'a>>,
    blocked: BTreeSet<Vec<String>>,
    local_routes: BTreeMap<*const syn::ItemMod, Vec<String>>,
    local_paths: BTreeSet<Vec<String>>,
}

impl<'a> Scopes<'a> {
    pub(crate) fn new(
        mut modules: BTreeMap<Vec<String>, ModuleScope<'a>>,
        mut blocked: BTreeSet<Vec<String>>,
    ) -> Self {
        let mut index = LocalModuleIndex {
            base: &modules,
            modules: BTreeMap::new(),
            blocked: BTreeSet::new(),
            routes: BTreeMap::new(),
            module: Vec::new(),
            observed: false,
            in_block: false,
            lexical_depth: 0,
            expression_depth: 0,
            next: 0,
        };
        for (path, scope) in &modules {
            index.module.clone_from(path);
            index.observed = blocked.contains(path);
            for item in scope.items() {
                if !matches!(item, syn::Item::Mod(_)) {
                    index.visit_item(item);
                }
            }
        }
        let local_paths = index.modules.keys().cloned().collect();
        let local_routes = index.routes;
        let added = index.modules;
        let observed = index.blocked;
        modules.extend(added);
        blocked.extend(observed);
        Self {
            modules,
            blocked,
            local_routes,
            local_paths,
        }
    }

    pub(crate) fn from_file(file: &'a syn::File) -> Self {
        let mut modules = BTreeMap::new();
        modules.insert(Vec::new(), ModuleScope::File(file));
        register_inline(&mut modules, &[], &file.items);
        let blocked = blocked_modules(&modules);
        Self::new(modules, blocked)
    }
}

struct LocalModuleIndex<'a, 's> {
    base: &'s BTreeMap<Vec<String>, ModuleScope<'a>>,
    modules: BTreeMap<Vec<String>, ModuleScope<'a>>,
    routes: BTreeMap<*const syn::ItemMod, Vec<String>>,
    blocked: BTreeSet<Vec<String>>,
    module: Vec<String>,
    observed: bool,
    in_block: bool,
    lexical_depth: usize,
    expression_depth: usize,
    next: usize,
}

impl<'a> Visit<'a> for LocalModuleIndex<'a, '_> {
    fn visit_item(&mut self, item: &'a syn::Item) {
        let observed = self.observed;
        self.observed |= blocking_attr(item_attrs(item));
        visit::visit_item(self, item);
        self.observed = observed;
    }

    fn visit_item_mod(&mut self, item: &'a syn::ItemMod) {
        let Some((_, items)) = &item.content else {
            return;
        };
        if self.lexical_depth >= MAX_BLOCK_DEPTH {
            return;
        }
        let parent = self.in_block;
        let name = if parent {
            loop {
                let name = format!("__dejadoc_local_module_{}", self.next);
                self.next += 1;
                self.module.push(name.clone());
                let occupied =
                    self.base.contains_key(&self.module) || self.modules.contains_key(&self.module);
                self.module.pop();
                if !occupied {
                    break name;
                }
            }
        } else {
            ident_name(&item.ident)
        };
        self.module.push(name);
        if parent {
            self.routes
                .insert(core::ptr::from_ref(item), self.module.clone());
            self.modules
                .insert(self.module.clone(), ModuleScope::Inline(items));
            register_inline(&mut self.modules, &self.module, items);
        }
        if self.observed {
            self.blocked.insert(self.module.clone());
        }
        self.in_block = false;
        self.lexical_depth += 1;
        for item in items {
            self.visit_item(item);
        }
        self.lexical_depth -= 1;
        self.in_block = parent;
        self.module.pop();
    }

    fn visit_block(&mut self, block: &'a syn::Block) {
        if self.lexical_depth >= MAX_BLOCK_DEPTH {
            return;
        }
        let parent = core::mem::replace(&mut self.in_block, true);
        self.lexical_depth += 1;
        visit::visit_block(self, block);
        self.lexical_depth -= 1;
        self.in_block = parent;
    }

    fn visit_expr(&mut self, expr: &'a syn::Expr) {
        if self.expression_depth >= MAX_BLOCK_DEPTH {
            return;
        }
        let observed = self.observed;
        self.observed |= blocking_attr(crate::drift::expr_attrs(expr));
        self.expression_depth += 1;
        visit::visit_expr(self, expr);
        self.expression_depth -= 1;
        self.observed = observed;
    }
}

pub(crate) fn use_tree_known(tree: &syn::UseTree, depth: usize) -> bool {
    if depth >= MAX_BLOCK_DEPTH {
        return false;
    }
    match tree {
        syn::UseTree::Path(path) => use_tree_known(&path.tree, depth + 1),
        syn::UseTree::Group(group) => group
            .items
            .iter()
            .all(|tree| use_tree_known(tree, depth + 1)),
        syn::UseTree::Glob(_) => false,
        _ => true,
    }
}

#[derive(Clone)]
struct Environment {
    frame: Frame,
    module: Vec<String>,
    prim_fallback: bool,
}

struct Declaration<'a> {
    item: &'a syn::ItemFn,
    module: Vec<String>,
    environment: usize,
    blocked: bool,
    parent: Option<HelperId>,
}

struct Collector<'a, 's> {
    scopes: &'s Scopes<'a>,
    table: HelperTable,
    environments: Vec<Environment>,
    declarations: Vec<Declaration<'a>>,
    blocks: BTreeMap<*const syn::Block, usize>,
    module_canons: BTreeMap<String, Vec<String>>,
    module: Vec<String>,
    environment: usize,
    call_scopes: BTreeMap<*const syn::Path, ScopeId>,
    blocked: bool,
    lexical_depth: usize,
    expression_depth: usize,
    next_scope: usize,
    owner: Option<HelperId>,
}

impl<'a> Collector<'a, '_> {
    fn declare(
        &mut self,
        item: &'a syn::ItemFn,
        canon: String,
        environment: usize,
        inherited: bool,
    ) -> HelperId {
        let id = HelperId(self.declarations.len());
        self.declarations.push(Declaration {
            item,
            module: self.module.clone(),
            environment,
            blocked: self.blocked || blocking_attr(&item.attrs),
            parent: self.owner,
        });
        self.table.entries.push(Entry {
            canon: canon.clone(),
            facts: None,
            exhausted: false,
        });
        self.table.inherited.push(inherited);
        self.table.canons.insert(canon, id);
        self.table
            .declarations
            .insert(core::ptr::from_ref(item.block.as_ref()), id);
        id
    }

    fn observe(&mut self, observed: bool) {
        if observed {
            let mut owner = self.owner;
            while let Some(id) = owner {
                let declaration = &mut self.declarations[id.0];
                declaration.blocked = true;
                owner = declaration.parent;
            }
        }
    }

    fn exhaust(&mut self) {
        let mut owner = self.owner;
        while let Some(id) = owner {
            let declaration = &mut self.declarations[id.0];
            declaration.blocked = true;
            self.table.entries[id.0].exhausted = true;
            owner = declaration.parent;
        }
    }

    fn mask_generics(&mut self, generics: &syn::Generics) {
        if generics.params.is_empty() {
            return;
        }
        let outer = &self.environments[self.environment];
        let mut frame = seed_items(
            &self.scopes.modules,
            &self.module,
            &[],
            &outer.frame,
            outer.prim_fallback,
        );
        let mut next = frame.next_id();
        for param in &generics.params {
            let (ns, ident) = match param {
                syn::GenericParam::Type(param) => (Ns::Type, &param.ident),
                syn::GenericParam::Const(param) => (Ns::Value, &param.ident),
                syn::GenericParam::Lifetime(_) => continue,
            };
            frame.bind(
                ns,
                &ident.to_string(),
                Binding {
                    id: BindingId(next),
                    canon: format!("__dejadoc_helper_generic_{}", ident_name(ident)),
                    origin: Origin::Generic,
                    prim: None,
                    port: 0,
                    value_port: None,
                    target: None,
                    raw: false,
                    helper: None,
                },
            );
            next += 1;
        }
        self.environment = self.environments.len();
        self.environments.push(Environment {
            frame,
            module: self.module.clone(),
            prim_fallback: outer.prim_fallback,
        });
    }
}

impl<'a> Visit<'a> for Collector<'a, '_> {
    fn visit_attribute(&mut self, attr: &'a syn::Attribute) {
        self.observe(blocking_meta(&attr.meta));
    }

    fn visit_item(&mut self, item: &'a syn::Item) {
        let blocked = self.blocked;
        self.observe(blocking_attr(item_attrs(item)));
        self.blocked |= blocking_attr(item_attrs(item));
        if !matches!(item, syn::Item::Mod(_)) {
            visit::visit_item(self, item);
        }
        self.blocked = blocked;
    }

    fn visit_item_fn(&mut self, item: &'a syn::ItemFn) {
        let environment = self.environment;
        let blocked = self.blocked;
        let depth = (self.lexical_depth, self.expression_depth);
        let owner = self.owner;
        self.owner = self.table.declaration(&item.block);
        self.observe(blocking_attr(&item.attrs));
        self.visit_signature(&item.sig);
        self.blocked |= blocking_attr(&item.attrs);
        self.mask_generics(&item.sig.generics);
        self.lexical_depth = 0;
        self.expression_depth = 0;
        self.visit_block(&item.block);
        self.environment = environment;
        self.blocked = blocked;
        (self.lexical_depth, self.expression_depth) = depth;
        self.owner = owner;
    }

    fn visit_item_impl(&mut self, item: &'a syn::ItemImpl) {
        let environment = self.environment;
        self.mask_generics(&item.generics);
        visit::visit_item_impl(self, item);
        self.environment = environment;
    }

    fn visit_item_trait(&mut self, item: &'a syn::ItemTrait) {
        let environment = self.environment;
        self.mask_generics(&item.generics);
        visit::visit_item_trait(self, item);
        self.environment = environment;
    }

    fn visit_impl_item_fn(&mut self, item: &'a syn::ImplItemFn) {
        let environment = self.environment;
        let blocked = self.blocked;
        self.observe(blocking_attr(&item.attrs));
        self.visit_signature(&item.sig);
        self.blocked |= blocking_attr(&item.attrs);
        self.mask_generics(&item.sig.generics);
        self.visit_block(&item.block);
        self.environment = environment;
        self.blocked = blocked;
    }

    fn visit_trait_item_fn(&mut self, item: &'a syn::TraitItemFn) {
        let environment = self.environment;
        let blocked = self.blocked;
        self.observe(blocking_attr(&item.attrs));
        self.visit_signature(&item.sig);
        self.blocked |= blocking_attr(&item.attrs);
        self.mask_generics(&item.sig.generics);
        if let Some(body) = &item.default {
            self.visit_block(body);
        }
        self.environment = environment;
        self.blocked = blocked;
    }

    fn visit_expr(&mut self, expr: &'a syn::Expr) {
        if self.expression_depth >= MAX_BLOCK_DEPTH {
            self.exhaust();
            return;
        }
        let blocked = self.blocked;
        self.blocked |= blocking_attr(crate::drift::expr_attrs(expr));
        self.observe(blocking_attr(crate::drift::expr_attrs(expr)));
        self.expression_depth += 1;
        if let syn::Expr::Call(call) = expr
            && let syn::Expr::Path(path) = call.func.as_ref()
        {
            self.call_scopes
                .insert(core::ptr::from_ref(&path.path), ScopeId(self.environment));
        }
        visit::visit_expr(self, expr);
        self.expression_depth -= 1;
        self.blocked = blocked;
    }

    fn visit_block(&mut self, block: &'a syn::Block) {
        if self.lexical_depth >= MAX_BLOCK_DEPTH {
            self.exhaust();
            return;
        }
        let environment = self.environment;
        let items: Vec<&syn::Item> = block
            .stmts
            .iter()
            .filter_map(|stmt| match stmt {
                syn::Stmt::Item(item) => Some(item),
                _ => None,
            })
            .collect();
        if !items.is_empty() {
            let outer = &self.environments[environment];
            let known = outer.prim_fallback && items.iter().all(|item| {
                !blocking_attr(item_attrs(item))
                    && !matches!(item, syn::Item::Use(item) if !use_tree_known(&item.tree, 0))
                    && !matches!(item, syn::Item::ExternCrate(_) | syn::Item::Macro(_))
                    && !matches!(item, syn::Item::Mod(item) if !self.scopes.local_routes.contains_key(&core::ptr::from_ref(item)))
            });
            let mut frame = seed_items(
                &self.scopes.modules,
                &self.module,
                &items,
                &outer.frame,
                known,
            );
            self.environment = self.environments.len();
            let scope = self.next_scope;
            self.next_scope += 1;
            for item in &items {
                if let syn::Item::Fn(function) = item {
                    let name = ident_name(&function.sig.ident);
                    let canon = format!(
                        "__dejadoc_helper_local_{scope}_{}",
                        qualified_name(&self.module, &name)
                    );
                    let id = self.declare(function, canon.clone(), self.environment, false);
                    let binding = frame
                        .bindings_mut(Ns::Value)
                        .get_mut(Ns::Value.key(&name))
                        .expect("indexed helper value binding");
                    binding.canon = canon;
                    binding.helper = Some(id);
                } else if let syn::Item::Mod(item) = item
                    && let Some(route) = self.scopes.local_routes.get(&core::ptr::from_ref(item))
                {
                    let binding = frame
                        .bindings_mut(Ns::Type)
                        .get_mut(&ident_name(&item.ident))
                        .expect("indexed local module binding");
                    let (name, parent) = route.split_last().expect("a local module route");
                    binding.canon = qualified_name(parent, name);
                } else if let Some(ident) = crate::alpha::type_item_ident(item)
                    && let Some(binding) = frame
                        .bindings_mut(Ns::Type)
                        .get_mut(Ns::Type.key(&ident.to_string()))
                {
                    binding.canon = format!("__dejadoc_helper_type_{scope}_{}", ident_name(ident));
                }
            }
            sanitize_imports(
                &self.scopes.modules,
                &self.module,
                &items,
                &mut frame,
                &self.module_canons,
            );
            self.environments.push(Environment {
                frame,
                module: self.module.clone(),
                prim_fallback: known,
            });
        }
        self.blocks
            .insert(core::ptr::from_ref(block), self.environment);
        self.lexical_depth += 1;
        visit::visit_block(self, block);
        self.lexical_depth -= 1;
        self.environment = environment;
    }
}

pub(crate) fn walk_imports(
    tree: &syn::UseTree,
    prefix: &mut Vec<String>,
    depth: usize,
    callback: &mut impl FnMut(&[String], &str),
) {
    if depth >= MAX_BLOCK_DEPTH {
        return;
    }
    match tree {
        syn::UseTree::Path(path) => {
            prefix.push(ident_name(&path.ident));
            walk_imports(&path.tree, prefix, depth + 1, callback);
            prefix.pop();
        }
        syn::UseTree::Name(name) if name.ident == "self" => {
            if let Some(alias) = prefix.last() {
                callback(prefix, alias);
            }
        }
        syn::UseTree::Name(name) => {
            prefix.push(ident_name(&name.ident));
            callback(prefix, &name.ident.to_string());
            prefix.pop();
        }
        syn::UseTree::Rename(rename) => {
            prefix.push(ident_name(&rename.ident));
            callback(prefix, &rename.rename.to_string());
            prefix.pop();
        }
        syn::UseTree::Group(group) => {
            for tree in &group.items {
                walk_imports(tree, prefix, depth + 1, callback);
            }
        }
        syn::UseTree::Glob(_) => {}
    }
}

fn lexical_use(
    scopes: &BTreeMap<Vec<String>, ModuleScope<'_>>,
    module: &[String],
    frame: &Frame,
    module_canons: &BTreeMap<String, Vec<String>>,
    path: &[String],
    rooted: bool,
) -> UseTarget {
    if !rooted
        && let Some(first) = path.first()
        && path.len() > 1
        && !matches!(first.as_str(), "crate" | "self" | "super")
        && let Some(binding) = frame.lookup(Ns::Type, first)
    {
        let Some(route) = module_canons.get(&binding.canon) else {
            return UseTarget::Unresolved;
        };
        return resolve_use(scopes, route, &path[1..], false);
    }
    resolve_use(scopes, module, path, rooted)
}

fn sanitize_imports(
    scopes: &BTreeMap<Vec<String>, ModuleScope<'_>>,
    module: &[String],
    items: &[&syn::Item],
    frame: &mut Frame,
    module_canons: &BTreeMap<String, Vec<String>>,
) {
    for item in items {
        let syn::Item::Use(item) = item else { continue };
        walk_imports(&item.tree, &mut Vec::new(), 0, &mut |path, alias| {
            let target = lexical_use(
                scopes,
                module,
                frame,
                module_canons,
                path,
                item.leading_colon.is_some(),
            );
            for ns in [Ns::Value, Ns::Type] {
                let Some(binding) = frame.bindings_mut(ns).get_mut(ns.key(alias)) else {
                    continue;
                };
                if binding.origin != Origin::Use {
                    continue;
                }
                match &target {
                    UseTarget::Item(target, namespaces) => {
                        let occupied = namespaces[ns as usize];
                        if occupied {
                            binding.canon.clone_from(&target.canon);
                            if ns == Ns::Type {
                                binding.prim = target.prim;
                            }
                        } else {
                            binding.canon = String::from("__dejadoc_helper_unresolved_import");
                            binding.prim = None;
                        }
                    }
                    UseTarget::External => {
                        binding.canon = String::from("__dejadoc_helper_external_import");
                    }
                    UseTarget::Unresolved => {
                        binding.canon = String::from("__dejadoc_helper_unresolved_import");
                        binding.prim = None;
                    }
                }
            }
        });
    }
}

fn item_attrs(item: &syn::Item) -> &[syn::Attribute] {
    match item {
        syn::Item::Const(item) => &item.attrs,
        syn::Item::Enum(item) => &item.attrs,
        syn::Item::ExternCrate(item) => &item.attrs,
        syn::Item::Fn(item) => &item.attrs,
        syn::Item::ForeignMod(item) => &item.attrs,
        syn::Item::Impl(item) => &item.attrs,
        syn::Item::Macro(item) => &item.attrs,
        syn::Item::Mod(item) => &item.attrs,
        syn::Item::Static(item) => &item.attrs,
        syn::Item::Struct(item) => &item.attrs,
        syn::Item::Trait(item) => &item.attrs,
        syn::Item::TraitAlias(item) => &item.attrs,
        syn::Item::Type(item) => &item.attrs,
        syn::Item::Union(item) => &item.attrs,
        syn::Item::Use(item) => &item.attrs,
        _ => &[],
    }
}

struct Signature {
    params: Vec<PrimTy>,
    result: PrimTy,
    values: BTreeMap<String, PrimTy>,
}

fn source_primitive(
    scopes: &Scopes<'_>,
    module_canons: &BTreeMap<String, Vec<String>>,
    environment: &Environment,
    renamer: &Renamer<'_>,
    mut ty: &syn::Type,
) -> Option<PrimTy> {
    if !environment.prim_fallback {
        return None;
    }
    for _ in 0..MAX_BLOCK_DEPTH {
        ty = match ty {
            syn::Type::Group(group) => &group.elem,
            syn::Type::Paren(paren) => &paren.elem,
            _ => {
                if let Some(prim) = renamer.param_prim(ty) {
                    return Some(prim);
                }
                let syn::Type::Path(path) = ty else {
                    return None;
                };
                if path.qself.is_some()
                    || path.path.segments.len() < 2
                    || path.path.segments.len() > MAX_BLOCK_DEPTH
                    || path
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
                    .map(|segment| ident_name(&segment.ident))
                    .collect();
                let UseTarget::Item(target, namespaces) = lexical_use(
                    &scopes.modules,
                    &environment.module,
                    &environment.frame,
                    module_canons,
                    &names,
                    path.path.leading_colon.is_some(),
                ) else {
                    return None;
                };
                return namespaces[Ns::Type as usize]
                    .then_some(target.prim)
                    .flatten();
            }
        };
    }
    None
}

fn signature(
    renamer: &Renamer<'_>,
    environment: &Environment,
    scopes: &Scopes<'_>,
    module_canons: &BTreeMap<String, Vec<String>>,
    sig: &syn::Signature,
) -> Option<Signature> {
    if sig.asyncness.is_some()
        || !matches!(sig.safety, syn::Safety::Default)
        || sig.abi.is_some()
        || sig.variadic.is_some()
        || !sig.generics.params.is_empty()
        || sig.generics.where_clause.is_some()
    {
        return None;
    }
    let mut params = Vec::with_capacity(sig.inputs.len());
    let mut values = BTreeMap::new();
    for arg in &sig.inputs {
        let syn::FnArg::Typed(arg) = arg else {
            return None;
        };
        let pattern = simple_pattern(&arg.pat)?;
        let prim = source_primitive(scopes, module_canons, environment, renamer, &arg.ty)?;
        params.push(prim);
        values.insert(ident_name(&pattern.ident), prim);
    }
    let syn::ReturnType::Type(_, result) = &sig.output else {
        return None;
    };
    Some(Signature {
        params,
        values,
        result: source_primitive(scopes, module_canons, environment, renamer, result)?,
    })
}

fn simple_pattern(pat: &syn::Pat) -> Option<&syn::PatIdent> {
    let syn::Pat::Ident(ident) = pat else {
        return None;
    };
    (ident.by_ref.is_none() && ident.mutability.is_none() && ident.subpat.is_none())
        .then_some(ident)
}

#[derive(Clone, Copy)]
struct Proven {
    prim: Option<PrimTy>,
    bits: Option<u128>,
}

impl Proven {
    fn scalar(prim: PrimTy) -> Self {
        Self {
            prim: Some(prim),
            bits: None,
        }
    }

    fn label(self) -> Lbl {
        match (self.prim, self.bits) {
            (Some(PrimTy::Bool), Some(bits)) => Lbl::Bool(bits != 0),
            (_, Some(bits)) => Lbl::Int(bits, self.prim),
            _ => Lbl::Ref(Value {
                port: Port::Param(0),
                prim: self.prim,
                helper: None,
            }),
        }
    }

    fn matches(self, expected: PrimTy) -> bool {
        self.prim == Some(expected)
            || (self.prim.is_none()
                && self.bits.is_some()
                && matches!(expected, PrimTy::Int { .. }))
    }
}

struct BodyFacts {
    nodes: usize,
    deps: Vec<HelperId>,
}

struct Proof<'a, 'env> {
    table: &'a HelperTable,
    scopes: &'a Scopes<'a>,
    environments: &'a [Environment],
    renamers: &'a [Renamer<'env>],
    blocks: &'a BTreeMap<*const syn::Block, usize>,
    module_canons: &'a BTreeMap<String, Vec<String>>,
    module: &'a [String],
    signatures: &'a [Option<Signature>],
    deps: BTreeSet<HelperId>,
    values: Vec<BTreeMap<String, PrimTy>>,
}

impl Proof<'_, '_> {
    fn value(&self, name: &str) -> Option<PrimTy> {
        self.values
            .iter()
            .rev()
            .find_map(|frame| frame.get(name).copied())
    }

    fn block(
        &mut self,
        block: &syn::Block,
        expected: Option<PrimTy>,
        tail: bool,
    ) -> Option<Proven> {
        self.values.push(BTreeMap::new());
        let result = self.block_inner(block, expected, tail);
        self.values.pop();
        result
    }

    fn block_inner(
        &mut self,
        block: &syn::Block,
        expected: Option<PrimTy>,
        tail: bool,
    ) -> Option<Proven> {
        let environment = *self.blocks.get(&core::ptr::from_ref(block))?;
        for (position, stmt) in block.stmts.iter().enumerate() {
            let last = position + 1 == block.stmts.len();
            match stmt {
                syn::Stmt::Local(local) => {
                    let init = local.init.as_ref()?;
                    if init.diverge.is_some() {
                        return None;
                    }
                    let (pattern, annotation) = match &local.pat {
                        syn::Pat::Type(typed) => (typed.pat.as_ref(), Some(typed.ty.as_ref())),
                        pattern => (pattern, None),
                    };
                    let name = ident_name(&simple_pattern(pattern)?.ident);
                    let annotation = match annotation {
                        Some(ty) => Some(source_primitive(
                            self.scopes,
                            self.module_canons,
                            &self.environments[environment],
                            &self.renamers[environment],
                            ty,
                        )?),
                        None => None,
                    };
                    let proven = self.expr(&init.expr, environment, annotation, false)?;
                    let prim = proven.prim.or(annotation)?;
                    self.values.last_mut()?.insert(name, prim);
                }
                syn::Stmt::Expr(expr, semi) => {
                    if last && (semi.is_none() || matches!(expr, syn::Expr::Return(_))) {
                        return self.expr(expr, environment, expected, tail);
                    }
                    self.expr(expr, environment, None, false)?;
                }
                syn::Stmt::Item(syn::Item::Fn(_) | syn::Item::Type(_) | syn::Item::Use(_)) => {}
                _ => return None,
            }
        }
        None
    }

    fn expr(
        &mut self,
        expr: &syn::Expr,
        environment: usize,
        expected: Option<PrimTy>,
        tail: bool,
    ) -> Option<Proven> {
        if let syn::Expr::Paren(paren) = expr {
            return self.expr(&paren.expr, environment, expected, tail);
        }
        if let syn::Expr::Group(group) = expr {
            return self.expr(&group.expr, environment, expected, tail);
        }
        let result = self.expr_inner(expr, environment, expected, tail);
        result.filter(|proven| expected.is_none_or(|expected| proven.matches(expected)))
    }

    fn expr_inner(
        &mut self,
        expr: &syn::Expr,
        environment: usize,
        expected: Option<PrimTy>,
        tail: bool,
    ) -> Option<Proven> {
        match expr {
            syn::Expr::Lit(lit) => literal(&lit.lit, expected),
            syn::Expr::Path(path) if path.qself.is_none() && path.path.get_ident().is_some() => {
                self.value(&ident_name(path.path.get_ident()?))
                    .map(Proven::scalar)
            }
            syn::Expr::Block(block) if block.label.is_none() => {
                self.block(&block.block, expected, tail)
            }
            syn::Expr::If(branch) => {
                self.expr(&branch.cond, environment, Some(PrimTy::Bool), false)?;
                let yes = self.block(&branch.then_branch, expected, tail)?;
                let (_, otherwise) = branch.else_branch.as_ref()?;
                let no = self.expr(otherwise, environment, expected.or(yes.prim), tail)?;
                let prim = yes.prim.or(no.prim)?;
                (yes.matches(prim) && no.matches(prim)).then_some(Proven::scalar(prim))
            }
            syn::Expr::Call(call) => {
                let syn::Expr::Path(callee) = call.func.as_ref() else {
                    return None;
                };
                if callee.qself.is_some() {
                    return None;
                }
                if let Some(ident) = callee.path.get_ident()
                    && self.value(&ident_name(ident)).is_some()
                {
                    return None;
                }
                let id = self.resolve(environment, &callee.path)?;
                let signatures = self.signatures;
                let signature = signatures.get(id.0)?.as_ref()?;
                if signature.params.len() != call.args.len() {
                    return None;
                }
                for (arg, &param) in call.args.iter().zip(&signature.params) {
                    self.expr(arg, environment, Some(param), false)?;
                }
                self.deps.insert(id);
                Some(Proven::scalar(signature.result))
            }
            syn::Expr::Unary(unary) if matches!(unary.op, syn::UnOp::Not(_)) => {
                let value = self.expr(&unary.expr, environment, expected, false)?;
                Some(Proven::scalar(value.prim?))
            }
            syn::Expr::Binary(binary) => {
                if matches!(
                    binary.op,
                    syn::BinOp::Add(_)
                        | syn::BinOp::Sub(_)
                        | syn::BinOp::Mul(_)
                        | syn::BinOp::Div(_)
                        | syn::BinOp::Rem(_)
                ) {
                    return folded(&self.renamers[environment], expr, expected);
                }
                let bitwise = crate::schedule::BitwiseOp::from_bin_op(&binary.op).is_some();
                let logical = matches!(binary.op, syn::BinOp::And(_) | syn::BinOp::Or(_));
                let shift = matches!(binary.op, syn::BinOp::Shl(_) | syn::BinOp::Shr(_));
                let left_expected = if logical {
                    Some(PrimTy::Bool)
                } else if bitwise || shift {
                    expected
                } else {
                    None
                };
                let left = self.expr(&binary.left, environment, left_expected, false)?;
                let right_expected = if shift {
                    None
                } else {
                    left.prim.or(left_expected)
                };
                let right = self.expr(&binary.right, environment, right_expected, false)?;
                let prim = match total_binary(&binary.op, &left.label(), &right.label())? {
                    TotalBinary::Flat(_, prim) | TotalBinary::Ordered { prim, .. } => prim,
                };
                if left.bits.is_some()
                    && right.bits.is_some()
                    && let Some(literal) = folded(&self.renamers[environment], expr, Some(prim))
                {
                    return Some(literal);
                }
                Some(Proven::scalar(prim))
            }
            syn::Expr::Unary(unary) if matches!(unary.op, syn::UnOp::Neg(_)) => {
                folded(&self.renamers[environment], expr, expected)
            }
            syn::Expr::Const(_) => folded(&self.renamers[environment], expr, expected),
            syn::Expr::Return(ret) if tail => {
                self.expr(ret.expr.as_ref()?, environment, expected, false)
            }
            _ => None,
        }
    }

    fn resolve(&self, environment: usize, path: &syn::Path) -> Option<HelperId> {
        if path.leading_colon.is_some()
            || path.segments.is_empty()
            || path.segments.len() > MAX_BLOCK_DEPTH
            || path
                .segments
                .iter()
                .any(|seg| !matches!(seg.arguments, syn::PathArguments::None))
        {
            return None;
        }
        let frame = &self.environments[environment].frame;
        if let Some(ident) = path.get_ident() {
            let binding = frame.lookup(Ns::Value, &ident.to_string())?;
            return match binding.origin {
                Origin::Item => binding.helper,
                Origin::Use => self.table.by_canon(&binding.canon),
                _ => None,
            };
        }
        let names: Vec<String> = path
            .segments
            .iter()
            .map(|seg| ident_name(&seg.ident))
            .collect();
        if !matches!(names[0].as_str(), "crate" | "self" | "super") {
            let binding = frame.lookup(Ns::Type, &names[0])?;
            let module = self.module_canons.get(&binding.canon)?;
            return self.table.resolve_names(module, &names[1..]);
        }
        self.table.resolve_names(self.module, &names)
    }
}

fn literal(lit: &syn::Lit, expected: Option<PrimTy>) -> Option<Proven> {
    match lit {
        syn::Lit::Bool(lit) => Some(Proven {
            prim: Some(PrimTy::Bool),
            bits: Some(u128::from(lit.value)),
        }),
        syn::Lit::Int(lit) => {
            let bits = lit.base10_parse().ok()?;
            let prim = if lit.suffix().is_empty() {
                expected.filter(|prim| matches!(prim, PrimTy::Int { .. }))
            } else {
                Some(primitive_suffix(lit.suffix())?)
            };
            Some(Proven {
                prim,
                bits: Some(bits),
            })
        }
        _ => None,
    }
}

fn folded(renamer: &Renamer<'_>, expr: &syn::Expr, expected: Option<PrimTy>) -> Option<Proven> {
    let mut normalized = expr.clone();
    crate::constants::normalize_expr(renamer, &mut normalized, expected);
    match &normalized {
        syn::Expr::Lit(lit) => literal(&lit.lit, expected),
        _ => crate::constants::signed_literal(&normalized).map(|(prim, bits)| Proven {
            prim: Some(prim),
            bits: Some(bits),
        }),
    }
}

struct OriginalScan {
    nodes: usize,
    exhausted: bool,
}

impl OriginalScan {
    fn node(&mut self) -> bool {
        if self.nodes >= MAX_REGION_NODES {
            self.exhausted = true;
            return false;
        }
        self.nodes += 1;
        true
    }
}

impl<'a> Visit<'a> for OriginalScan {
    fn visit_item(&mut self, item: &'a syn::Item) {
        if matches!(item, syn::Item::Fn(_)) {
            return;
        }
        if self.node() {
            visit::visit_item(self, item);
        }
    }

    fn visit_stmt(&mut self, stmt: &'a syn::Stmt) {
        if self.node() {
            visit::visit_stmt(self, stmt);
        }
    }

    fn visit_expr(&mut self, expr: &'a syn::Expr) {
        if self.node() {
            visit::visit_expr(self, expr);
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum State {
    Pending,
    Accepted,
    Rejected,
}

fn source_collector<'a, 's>(scopes: &'s Scopes<'a>) -> Collector<'a, 's> {
    let mut collector = Collector {
        scopes,
        table: HelperTable::default(),
        environments: Vec::new(),
        declarations: Vec::new(),
        blocks: BTreeMap::new(),
        module_canons: BTreeMap::new(),
        module: Vec::new(),
        environment: 0,
        call_scopes: BTreeMap::new(),
        blocked: false,
        lexical_depth: 0,
        expression_depth: 0,
        next_scope: 0,
        owner: None,
    };
    let mut module_environments = BTreeMap::new();
    for (path, scope) in &scopes.modules {
        let known = scope_primitives_known(scope.items())
            && scope.items().iter().all(
                |item| !matches!(item, syn::Item::Use(item) if !use_tree_known(&item.tree, 0)),
            );
        let frame = seed_frame(&scopes.modules, path, scope, known);
        let environment = collector.environments.len();
        collector.environments.push(Environment {
            frame,
            module: path.clone(),
            prim_fallback: known,
        });
        module_environments.insert(path.clone(), environment);
        if let Some((name, parent)) = path.split_last() {
            collector
                .module_canons
                .insert(qualified_name(parent, name), path.clone());
        }
        collector.module.clone_from(path);
        collector.environment = environment;
        collector.blocked = scopes.blocked.contains(path);
        for item in scope.items() {
            if let syn::Item::Fn(function) = item {
                collector.declare(
                    function,
                    qualified_name(path, &ident_name(&function.sig.ident)),
                    environment,
                    !scopes.local_paths.contains(path),
                );
            }
        }
    }
    for (path, &environment) in &module_environments {
        let frame = &mut collector.environments[environment].frame;
        let items: Vec<&syn::Item> = scopes.modules[path].items().iter().collect();
        sanitize_imports(
            &scopes.modules,
            path,
            &items,
            frame,
            &collector.module_canons,
        );
        let mut names = ModuleNames::default();
        for (name, binding) in frame.bindings_mut(Ns::Value) {
            if let Some(id) = collector.table.by_canon(&binding.canon) {
                binding.helper = Some(id);
                names.values.insert(name.clone(), id);
            }
        }
        for (name, binding) in frame.bindings(Ns::Type) {
            if let Some(target) = collector.module_canons.get(&binding.canon) {
                names.modules.insert(name.clone(), target.clone());
            }
        }
        collector.table.modules.insert(path.clone(), names);
    }
    for (path, scope) in &scopes.modules {
        collector.module.clone_from(path);
        collector.environment = module_environments[path];
        collector.blocked = scopes.blocked.contains(path);
        for item in scope.items() {
            collector.visit_item(item);
        }
    }
    collector
}

pub(crate) fn module_table(scopes: &Scopes<'_>) -> HelperTable {
    let mut collector = source_collector(scopes);
    let renamers: Vec<Renamer<'_>> = collector
        .environments
        .iter()
        .map(|environment| Renamer::scratch_ref(&environment.frame, environment.prim_fallback))
        .collect();
    let mut signatures: Vec<Option<Signature>> = collector
        .declarations
        .iter()
        .map(|declaration| {
            (!declaration.blocked)
                .then(|| {
                    signature(
                        &renamers[declaration.environment],
                        &collector.environments[declaration.environment],
                        scopes,
                        &collector.module_canons,
                        &declaration.item.sig,
                    )
                })
                .flatten()
        })
        .collect();
    let mut bodies = Vec::with_capacity(collector.declarations.len());
    let mut exhausted: Vec<bool> = collector
        .table
        .entries
        .iter()
        .map(|entry| entry.exhausted)
        .collect();
    for (index, declaration) in collector.declarations.iter().enumerate() {
        let values = signatures[index]
            .as_mut()
            .map(|signature| core::mem::take(&mut signature.values));
        let body = signatures[index].as_ref().and_then(|signature| {
            let mut original = OriginalScan {
                nodes: 1,
                exhausted: false,
            };
            original.visit_item_fn(declaration.item);
            if original.exhausted {
                exhausted[index] = original.exhausted;
                return None;
            }
            let mut proof = Proof {
                table: &collector.table,
                environments: &collector.environments,
                renamers: &renamers,
                scopes,
                blocks: &collector.blocks,
                module_canons: &collector.module_canons,
                module: &declaration.module,
                signatures: &signatures,
                deps: BTreeSet::new(),
                values: vec![values?],
            };
            proof.block(&declaration.item.block, Some(signature.result), true)?;
            Some(BodyFacts {
                nodes: original.nodes,
                deps: proof.deps.into_iter().collect(),
            })
        });
        bodies.push(body);
    }
    let states = dependency_states(&bodies, &mut exhausted);
    drop(renamers);
    for ((((entry, signature), body), state), exhausted) in collector
        .table
        .entries
        .iter_mut()
        .zip(signatures)
        .zip(bodies)
        .zip(states)
        .zip(exhausted)
    {
        entry.exhausted = exhausted;
        if state == State::Accepted
            && let (Some(signature), Some(body)) = (signature, body)
        {
            entry.facts = Some(TableFacts {
                params: signature.params,
                result: signature.result,
                nodes: body.nodes,
                deps: body.deps,
            });
        }
    }
    collector.table.block_scopes = collector
        .blocks
        .into_iter()
        .map(|(block, scope)| (block, ScopeId(scope)))
        .collect();
    collector.table.frames = collector.environments;
    collector.table.module_canons = collector.module_canons;
    collector.table.calls = collector.call_scopes;
    collector.table
}

fn dependency_states(bodies: &[Option<BodyFacts>], exhausted: &mut [bool]) -> Vec<State> {
    let mut dependents = vec![Vec::new(); bodies.len()];
    let mut remaining = vec![0usize; bodies.len()];
    let mut states = vec![State::Pending; bodies.len()];
    let mut ready = VecDeque::new();
    for (index, body) in bodies.iter().enumerate() {
        if let Some(body) = body {
            remaining[index] = body.deps.len();
            for dep in &body.deps {
                dependents[dep.0].push(index);
            }
            if body.deps.is_empty() {
                states[index] = State::Accepted;
                ready.push_back(index);
            }
        } else {
            states[index] = State::Rejected;
            ready.push_back(index);
        }
    }
    while let Some(index) = ready.pop_front() {
        for &dependent in &dependents[index] {
            if states[dependent] != State::Pending {
                continue;
            }
            if states[index] == State::Rejected {
                exhausted[dependent] |= exhausted[index];
                states[dependent] = State::Rejected;
                ready.push_back(dependent);
            } else {
                remaining[dependent] -= 1;
                if remaining[dependent] == 0 {
                    states[dependent] = State::Accepted;
                    ready.push_back(dependent);
                }
            }
        }
    }
    states
}
