//! Original module environments and borrowed function declarations.

use alloc::boxed::Box;
use alloc::collections::{BTreeMap, BTreeSet};
use alloc::string::String;
use alloc::string::ToString;
use alloc::vec;
use alloc::vec::Vec;

use quote::ToTokens;
use syn::visit::Visit;

use crate::alpha;
use crate::form;
use crate::scope::{Binding, BindingId, Frame, Ns, Origin, PrimTy, ident_name, unraw};

/// The original owner of a function.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Owner {
    /// A free function.
    Free,
    /// A method of an inherent impl.
    Inherent(String),
    /// A default body of a trait.
    Trait(String),
}

impl Owner {
    /// The resolved owner name or an injective encoding of its type.
    fn self_name(&self, seed: &Frame) -> Option<String> {
        let text = match self {
            Self::Free => return None,
            Self::Inherent(text) | Self::Trait(text) => text,
        };
        if let Some(binding) = seed.lookup(Ns::Type, unraw(text)) {
            return Some(binding.canon.clone());
        }
        Some(mangle_owner(text))
    }
}

/// A valid-identifier spelling of `text`, injective over its bytes.
fn mangle_owner(text: &str) -> String {
    const HEX: [char; 16] = [
        '0', '1', '2', '3', '4', '5', '6', '7', '8', '9', 'a', 'b', 'c', 'd', 'e', 'f',
    ];
    let mut out = String::from("__dejadoc_self");
    for byte in text.bytes() {
        out.push(HEX[usize::from(byte >> 4)]);
        out.push(HEX[usize::from(byte & 0x0f)]);
    }
    out
}

/// The private id of an indexed module, file-based or inline.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
struct ModuleId(usize);

/// One indexed module: its seed frame and its own glob uncertainty.
struct Module {
    seed: Frame,
    prim_fallback: bool,
    observed: bool,
    helper_blocked: bool,
}

/// One original declaration and its module environment.
struct Decl<'a> {
    module: ModuleId,
    /// The original declaration attributes.
    attrs: &'a [syn::Attribute],
    owner: Owner,
    prim_fallback: bool,
    observed: bool,
    helper_module: Vec<String>,
    helper_blocked: bool,
    /// The original enclosing impl or trait generics.
    generics: Option<*const syn::Generics>,
}

/// The source context, one entry per module file and inline module.
pub struct SourceContext<'a> {
    modules: Vec<Module>,
    decls: BTreeMap<(*const syn::Signature, *const syn::Block), Decl<'a>>,
    /// Generic frames indexed by their original declarations.
    gen_frames: BTreeMap<*const syn::Generics, Frame>,
    /// Complete helper summaries shared by every borrowed declaration.
    helpers: crate::helpers::HelperTable,
}

/// A function view selected from a source context.
pub struct FunctionView<'a> {
    ctx: &'a SourceContext<'a>,
    entry: &'a Decl<'a>,
    sig: &'a syn::Signature,
    block: &'a syn::Block,
}

impl FunctionView<'_> {
    /// The original declaration's canonical form in its module environment.
    #[must_use]
    pub fn canonicalize(&self) -> crate::CanonicalForm {
        let seed = self.ctx.seed(self.entry.module);
        let generics = self.entry.generics.map(|key| self.ctx.gen_frame(key));
        let self_canon = self.entry.owner.self_name(seed);
        let mut sig = self.sig.clone();
        if !matches!(self.entry.owner, Owner::Free) {
            sig.ident = syn::Ident::new("__dejadoc_method", sig.ident.span());
        }
        let mut item = syn::ItemFn {
            attrs: self
                .entry
                .attrs
                .iter()
                .filter(|attribute| live(&attribute.meta))
                .cloned()
                .collect(),
            vis: syn::Visibility::Inherited,
            modifiers: syn::FnModifiers::default(),
            sig,
            block: Box::new(self.block.clone()),
        };
        let helpers = transfer_helpers(&self.ctx.helpers, self.sig, self.block, &mut item);
        let file = syn::File {
            shebang: None,
            frontmatter: None,
            attrs: Vec::new(),
            items: vec![syn::Item::Fn(item)],
        };
        form::canonical_file_with_options(
            file,
            true,
            alpha::Options {
                seed: Some(seed),
                generics,
                prim_fallback: self.entry.prim_fallback,
                observed: self.entry.observed,
                self_canon,
                helper_table: Some(&self.ctx.helpers),
                helper_module: &self.entry.helper_module,
                helper_blocked: self.entry.helper_blocked,
                helper_syntax: Some(helpers),
            },
        )
    }
}

fn transfer_helpers(
    helpers: &crate::helpers::HelperTable,
    signature: &syn::Signature,
    body: &syn::Block,
    cloned: &mut syn::ItemFn,
) -> crate::helpers::HelperSyntax {
    crate::helpers::HelperSyntax::cloned_function(helpers, signature, body, cloned)
}

impl<'a> SourceContext<'a> {
    /// The shared seed of the module a declaration was indexed under.
    fn seed(&self, module: ModuleId) -> &Frame {
        &self.modules[module.0].seed
    }

    /// The shared frame of an impl or trait's generics.
    fn gen_frame(&self, key: *const syn::Generics) -> &Frame {
        &self.gen_frames[&key]
    }

    /// Index every module's top-level seed and every function it declares,
    /// inline modules under their own path, seed, and glob uncertainty.
    pub fn new<I>(files: I) -> Self
    where
        I: IntoIterator<Item = (&'a [String], &'a syn::File)>,
    {
        let files: Vec<(&'a [String], &'a syn::File)> = files.into_iter().collect();
        let mut scopes: BTreeMap<Vec<String>, ModuleScope<'a>> = BTreeMap::new();
        for (path, file) in &files {
            scopes
                .entry((*path).to_vec())
                .or_insert(ModuleScope::File(file));
        }
        for (path, file) in &files {
            register_inline(&mut scopes, path, &file.items);
        }
        let ancestry = module_ancestry(&scopes);
        let blocked: BTreeSet<Vec<String>> = ancestry
            .iter()
            .filter(|(_, (_, blocked))| *blocked)
            .map(|(path, _)| path.clone())
            .collect();
        let mut module_ids: BTreeMap<Vec<String>, ModuleId> = BTreeMap::new();
        let mut modules: Vec<Module> = Vec::new();
        for ((path, scope), &(observed, helper_blocked)) in scopes.iter().zip(ancestry.values()) {
            let prim_fallback = scope_primitives_known(scope.items());
            let seed = seed_frame(&scopes, path, scope, prim_fallback);
            module_ids.insert(path.clone(), ModuleId(modules.len()));
            modules.push(Module {
                seed,
                prim_fallback,
                observed,
                helper_blocked,
            });
        }
        let helpers = crate::helpers::module_table(&crate::helpers::Scopes::new(scopes, blocked));
        for module in &mut modules {
            for binding in module.seed.bindings_mut(Ns::Value).values_mut() {
                binding.helper = helpers.by_canon(&binding.canon);
            }
        }
        let mut decls: BTreeMap<(*const syn::Signature, *const syn::Block), Decl<'a>> =
            BTreeMap::new();
        let mut gen_frames: BTreeMap<*const syn::Generics, Frame> = BTreeMap::new();
        for (path, file) in &files {
            index_scope(
                &mut decls,
                &module_ids,
                &modules,
                path,
                &file.items,
                false,
                false,
                &mut gen_frames,
            );
        }
        Self {
            modules,
            decls,
            gen_frames,
            helpers,
        }
    }

    /// The view of the exact declaration its signature and block pointers
    /// index, `None` when the pair does not select one.
    #[must_use]
    pub fn function<'s>(
        &'s self,
        sig: &'s syn::Signature,
        block: &'s syn::Block,
    ) -> Option<FunctionView<'s>> {
        let key = (
            core::ptr::from_ref::<syn::Signature>(sig),
            core::ptr::from_ref::<syn::Block>(block),
        );
        let entry = self.decls.get(&key)?;
        Some(FunctionView {
            ctx: self,
            entry,
            sig,
            block,
        })
    }
}

/// Whether a module imports no wildcard names.
fn glob_free(items: &[syn::Item]) -> bool {
    !items
        .iter()
        .any(|item| matches!(item, syn::Item::Use(use_item) if has_glob(&use_item.tree)))
}

pub(crate) fn scope_primitives_known(items: &[syn::Item]) -> bool {
    let mut uncertainty = ScopeUncertainty(false);
    for item in items {
        uncertainty.visit_item(item);
        if uncertainty.0 {
            return false;
        }
    }
    glob_free(items)
}

struct ScopeUncertainty(bool);

impl<'ast> Visit<'ast> for ScopeUncertainty {
    fn visit_attribute(&mut self, attr: &'ast syn::Attribute) {
        self.0 |=
            live(&attr.meta) || attr.path().is_ident("cfg") || attr.path().is_ident("cfg_attr");
    }

    fn visit_item_mod(&mut self, item: &'ast syn::ItemMod) {
        for attr in &item.attrs {
            self.visit_attribute(attr);
        }
    }

    fn visit_item_macro(&mut self, item: &'ast syn::ItemMacro) {
        self.0 |= item.ident.is_none();
        syn::visit::visit_item_macro(self, item);
    }

    fn visit_item_extern_crate(&mut self, _: &'ast syn::ItemExternCrate) {
        self.0 = true;
    }

    fn visit_block(&mut self, _: &'ast syn::Block) {}
}

pub(crate) fn has_glob(tree: &syn::UseTree) -> bool {
    match tree {
        syn::UseTree::Glob(_) => true,
        syn::UseTree::Path(path) => has_glob(&path.tree),
        syn::UseTree::Group(group) => group.items.iter().any(has_glob),
        _ => false,
    }
}

/// A module's items, inline content included.
#[derive(Clone, Copy)]
pub(crate) enum ModuleScope<'a> {
    File(&'a syn::File),
    Inline(&'a [syn::Item]),
}

impl<'a> ModuleScope<'a> {
    pub(crate) fn items(&self) -> &'a [syn::Item] {
        match self {
            Self::File(file) => &file.items,
            Self::Inline(items) => items,
        }
    }

    fn shadows_primitive_crate(&self, name: &str) -> bool {
        let raw = if name == "core" { "r#core" } else { "r#std" };
        let matches = |ident: &syn::Ident| ident == name || ident == raw;
        self.items().iter().any(|item| {
            alpha::type_item_ident(item).is_some_and(matches)
                || matches!(item, syn::Item::Use(import)
                    if alpha::imported_name_matches(&import.tree, matches))
        })
    }
}

/// Register every inline module body under its module path.
pub(crate) fn register_inline<'a>(
    scopes: &mut BTreeMap<Vec<String>, ModuleScope<'a>>,
    path: &[String],
    items: &'a [syn::Item],
) {
    for item in items {
        if let syn::Item::Mod(mod_item) = item
            && let Some((_, inner)) = &mod_item.content
        {
            let mut child = path.to_vec();
            child.push(unraw(&mod_item.ident.to_string()).to_string());
            scopes
                .entry(child.clone())
                .or_insert(ModuleScope::Inline(inner));
            register_inline(scopes, &child, inner);
        }
    }
}

fn module_ancestry(
    scopes: &BTreeMap<Vec<String>, ModuleScope<'_>>,
) -> BTreeMap<Vec<String>, (bool, bool)> {
    let mut direct: BTreeMap<Vec<String>, (bool, bool)> = BTreeMap::new();
    for (path, scope) in scopes {
        if let ModuleScope::File(file) = scope {
            let flags = direct.entry(path.clone()).or_default();
            flags.0 |= live_attr(&file.attrs);
            flags.1 |= blocking_attr(&file.attrs);
        }
        for item in scope.items() {
            if let syn::Item::Mod(module) = item {
                let mut child = path.clone();
                child.push(ident_name(&module.ident));
                let flags = direct.entry(child).or_default();
                flags.0 |= live_attr(&module.attrs);
                flags.1 |= blocking_attr(&module.attrs);
            }
        }
    }
    scopes
        .keys()
        .map(|path| {
            let mut flags = (false, false);
            for end in 0..=path.len() {
                if let Some(&(observed, blocked)) = direct.get(&path[..end]) {
                    flags.0 |= observed;
                    flags.1 |= blocked;
                }
            }
            (path.clone(), flags)
        })
        .collect()
}

pub(crate) fn blocked_modules(
    scopes: &BTreeMap<Vec<String>, ModuleScope<'_>>,
) -> BTreeSet<Vec<String>> {
    module_ancestry(scopes)
        .into_iter()
        .filter_map(|(path, (_, blocked))| blocked.then_some(path))
        .collect()
}

pub(crate) fn qualified_name(module: &[String], name: &str) -> String {
    let mut out = String::from("__dejadoc_inherited");
    for part in module
        .iter()
        .map(|part| unraw(part))
        .chain(core::iter::once(unraw(name)))
    {
        qualified_part(&mut out, part);
    }
    out
}

pub(crate) fn qualified_part(out: &mut String, part: &str) {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    out.push('_');
    for byte in part.bytes() {
        out.push(char::from(HEX[usize::from(byte >> 4)]));
        out.push(char::from(HEX[usize::from(byte & 15)]));
    }
}

#[derive(Clone)]
pub(crate) struct ImportedTarget {
    pub(crate) canon: String,
    pub(crate) prim: Option<PrimTy>,
    module: Option<Vec<String>>,
}

/// One resolved `use` target.
pub(crate) enum UseTarget {
    /// An external path.
    External,
    /// An indexed item's name and occupied namespaces.
    Item(ImportedTarget, [bool; 5]),
    /// A name the context could not resolve.
    Unresolved,
}

const IMPORT_NAMESPACES: [Ns; 3] = [Ns::Value, Ns::Type, Ns::Macro];

#[derive(Default)]
enum UnknownImport {
    #[default]
    None,
    Unique(ImportedTarget),
    Ambiguous,
}

#[derive(Default)]
struct ImportedName {
    resolved: [Option<ImportedTarget>; 5],
    unknown: UnknownImport,
}

struct ImportPath<'a> {
    module: &'a [String],
    name: &'a str,
    parent: Option<&'a Self>,
}

impl ImportPath<'_> {
    fn contains(&self, module: &[String], name: &str) -> bool {
        let mut active = Some(self);
        while let Some(query) = active {
            if query.module == module && query.name == name {
                return true;
            }
            active = query.parent;
        }
        false
    }
}

struct UseBinder<'a> {
    scopes: &'a BTreeMap<Vec<String>, ModuleScope<'a>>,
    from: &'a [String],
    leading_colon: bool,
    imports: &'a mut BTreeMap<String, ImportedName>,
    only_name: Option<&'a str>,
    depth: usize,
    active: Option<&'a ImportPath<'a>>,
}

fn import_slots(
    slots: &mut [Option<ImportedTarget>; 5],
    namespaces: [bool; 5],
    target: ImportedTarget,
) {
    let mut active = namespaces
        .into_iter()
        .enumerate()
        .filter_map(|(index, present)| present.then_some(index))
        .peekable();
    while let Some(index) = active.next() {
        if active.peek().is_some() {
            slots[index] = Some(target.clone());
        } else {
            slots[index] = Some(target);
            break;
        }
    }
}

fn bind_imports(
    frame: &mut Frame,
    from: &[String],
    imports: BTreeMap<String, ImportedName>,
    next_id: &mut usize,
) {
    for (alias, mut imported) in imports {
        let unknown = match imported.unknown {
            UnknownImport::None => None,
            UnknownImport::Unique(target) => Some(target),
            UnknownImport::Ambiguous => Some(ImportedTarget {
                canon: qualified_name(from, &alias),
                prim: None,
                module: None,
            }),
        };
        if let Some(target) = unknown {
            let mut unclaimed = [false; 5];
            for ns in IMPORT_NAMESPACES {
                unclaimed[ns as usize] =
                    imported.resolved[ns as usize].is_none() && frame.lookup(ns, &alias).is_none();
            }
            import_slots(&mut imported.resolved, unclaimed, target);
        }
        for ns in IMPORT_NAMESPACES {
            if let Some(target) = imported.resolved[ns as usize].take() {
                frame.bind(
                    ns,
                    &alias,
                    Binding {
                        id: BindingId(*next_id),
                        canon: target.canon,
                        origin: Origin::Use,
                        prim: if ns == Ns::Type { target.prim } else { None },
                        target: None,
                        raw: false,
                        port: 0,
                        value_port: None,
                        helper: None,
                    },
                );
                *next_id += 1;
            }
        }
    }
}

fn use_tree_bindings(binder: &mut UseBinder<'_>, prefix: &[String], tree: &syn::UseTree) {
    if binder.depth >= 64 {
        return;
    }
    binder.depth += 1;
    match tree {
        syn::UseTree::Path(path) => {
            let mut next = prefix.to_vec();
            next.push(ident_name(&path.ident));
            use_tree_bindings(binder, &next, &path.tree);
        }
        syn::UseTree::Name(name) if name.ident == "self" && !prefix.is_empty() => {
            bind_use_name(
                binder,
                prefix,
                prefix.last().expect("nonempty import prefix"),
            );
        }
        syn::UseTree::Name(name) => {
            let mut path = prefix.to_vec();
            path.push(ident_name(&name.ident));
            bind_use_name(binder, &path, &name.ident.to_string());
        }
        syn::UseTree::Rename(rename) => {
            let mut path = prefix.to_vec();
            if rename.ident != "self" || path.is_empty() {
                path.push(ident_name(&rename.ident));
            }
            bind_use_name(binder, &path, &rename.rename.to_string());
        }
        syn::UseTree::Group(group) => {
            for inner in &group.items {
                use_tree_bindings(binder, prefix, inner);
            }
        }
        syn::UseTree::Glob(_) => {}
    }
    binder.depth -= 1;
}

fn bind_use_name(binder: &mut UseBinder<'_>, path: &[String], alias: &str) {
    if binder.only_name.is_some_and(|name| unraw(alias) != name) || path.is_empty() {
        return;
    }
    let target = resolve_use_at(
        binder.scopes,
        binder.from,
        path,
        binder.leading_colon,
        binder.depth,
        binder.active,
    );
    let imported = if let Some(imported) = binder.imports.get_mut(alias) {
        imported
    } else {
        binder.imports.entry(alias.to_string()).or_default()
    };
    let target = match target {
        UseTarget::Item(target, namespaces) => {
            import_slots(&mut imported.resolved, namespaces, target);
            return;
        }
        UseTarget::External => ImportedTarget {
            canon: qualified_name(&path[..path.len() - 1], &path[path.len() - 1]),
            prim: external_prim(path).filter(|_| {
                scope_primitives_known(
                    binder
                        .scopes
                        .get(binder.from)
                        .expect("indexed importing module")
                        .items(),
                )
            }),
            module: None,
        },
        UseTarget::Unresolved => ImportedTarget {
            canon: qualified_name(binder.from, alias),
            prim: None,
            module: None,
        },
    };
    imported.unknown = match imported.unknown {
        UnknownImport::None => UnknownImport::Unique(target),
        UnknownImport::Unique(_) | UnknownImport::Ambiguous => UnknownImport::Ambiguous,
    };
}

/// Resolve a use path against the module map, `from` the importing module.
pub(crate) fn resolve_use(
    scopes: &BTreeMap<Vec<String>, ModuleScope<'_>>,
    from: &[String],
    path: &[String],
    leading_colon: bool,
) -> UseTarget {
    resolve_use_at(scopes, from, path, leading_colon, 0, None)
}

fn resolve_use_at(
    scopes: &BTreeMap<Vec<String>, ModuleScope<'_>>,
    from: &[String],
    path: &[String],
    leading_colon: bool,
    depth: usize,
    active: Option<&ImportPath<'_>>,
) -> UseTarget {
    if depth >= 64 || path.len() > 64 {
        return UseTarget::Unresolved;
    }
    let Some(first) = path.first().map(|name| unraw(name)) else {
        return UseTarget::Unresolved;
    };
    if leading_colon {
        return UseTarget::External;
    }
    let local = scopes.get(from).expect("indexed importing module");
    if matches!(first, "std" | "core") && !local.shadows_primitive_crate(first) {
        return UseTarget::External;
    }
    let (mut base, mut rest) = match first {
        "crate" => (Vec::new(), &path[1..]),
        "self" => (from.to_vec(), &path[1..]),
        _ => (from.to_vec(), path),
    };
    while rest.first().is_some_and(|name| unraw(name) == "super") {
        if base.pop().is_none() {
            return UseTarget::Unresolved;
        }
        rest = &rest[1..];
    }
    let Some(mut scope) = scopes.get(&base).copied() else {
        return UseTarget::Unresolved;
    };
    if rest.is_empty() {
        let Some(name) = base.last() else {
            return UseTarget::Unresolved;
        };
        return UseTarget::Item(
            ImportedTarget {
                canon: qualified_name(&base[..base.len() - 1], name),
                prim: None,
                module: Some(base.clone()),
            },
            [false, true, false, false, false],
        );
    }
    for (index, name) in rest.iter().enumerate() {
        let name = unraw(name);
        if index + 1 == rest.len() {
            return resolve_item(scopes, scope.items(), &base, name, depth + 1, active);
        }
        let mut next_path = base.clone();
        next_path.push(name.to_string());
        if !scopes.contains_key(&next_path) {
            let UseTarget::Item(target, namespaces) =
                resolve_item(scopes, scope.items(), &base, name, depth + 1, active)
            else {
                return UseTarget::Unresolved;
            };
            if !namespaces[1] {
                return UseTarget::Unresolved;
            }
            let Some(module) = target.module else {
                return UseTarget::Unresolved;
            };
            next_path = module;
        }
        let next = scopes
            .get(&next_path)
            .copied()
            .expect("indexed import target module");
        base = next_path;
        scope = next;
    }
    UseTarget::Unresolved
}

fn resolve_item(
    scopes: &BTreeMap<Vec<String>, ModuleScope<'_>>,
    items: &[syn::Item],
    module: &[String],
    name: &str,
    depth: usize,
    active: Option<&ImportPath<'_>>,
) -> UseTarget {
    if depth >= 64 {
        return UseTarget::Unresolved;
    }
    let mut namespaces = [false; 5];
    for item in items {
        let value = match item {
            syn::Item::Fn(item) => Some(&item.sig.ident),
            syn::Item::Const(item) => Some(&item.ident),
            syn::Item::Static(item) => Some(&item.ident),
            _ => None,
        };
        if value.is_some_and(|ident| ident_name(ident) == name) {
            namespaces[Ns::Value as usize] = true;
        }
        if alpha::type_item_ident(item).is_some_and(|ident| ident_name(ident) == name) {
            namespaces[Ns::Type as usize] = true;
            if let syn::Item::Struct(item) = item
                && !matches!(item.fields, syn::Fields::Named(_))
            {
                namespaces[Ns::Value as usize] = true;
            }
        }
        if let syn::Item::Macro(item) = item
            && item
                .ident
                .as_ref()
                .is_some_and(|ident| ident_name(ident) == name)
        {
            namespaces[Ns::Macro as usize] = true;
        }
    }
    if namespaces.iter().any(|present| *present) {
        let prim = items.iter().find_map(|item| match item {
            syn::Item::Type(alias) if ident_name(&alias.ident) == name => {
                alias_chain_prim(alias, &items.iter(), scope_primitives_known(items), 0)
                    .or_else(|| resolved_alias_prim(scopes, module, alias, depth + 1, active))
            }
            _ => None,
        });
        UseTarget::Item(
            ImportedTarget {
                canon: qualified_name(module, name),
                prim,
                module: items.iter().find_map(|item| {
                    if let syn::Item::Mod(item) = item
                        && ident_name(&item.ident) == name
                    {
                        let mut path = module.to_vec();
                        path.push(name.to_string());
                        scopes.contains_key(&path).then_some(path)
                    } else {
                        None
                    }
                }),
            },
            namespaces,
        )
    } else {
        resolve_imported_item(scopes, items, module, name, depth, active)
    }
}

fn resolve_imported_item(
    scopes: &BTreeMap<Vec<String>, ModuleScope<'_>>,
    items: &[syn::Item],
    module: &[String],
    name: &str,
    depth: usize,
    active: Option<&ImportPath<'_>>,
) -> UseTarget {
    if active.is_some_and(|query| query.contains(module, name)) {
        return UseTarget::Unresolved;
    }
    let query = ImportPath {
        module,
        name,
        parent: active,
    };
    let mut namespaces = [false; 5];
    let mut imports = BTreeMap::new();
    for item in items {
        if let syn::Item::Use(import) = item {
            use_tree_bindings(
                &mut UseBinder {
                    scopes,
                    from: module,
                    leading_colon: import.leading_colon.is_some(),
                    imports: &mut imports,
                    only_name: Some(name),
                    depth: depth + 1,
                    active: Some(&query),
                },
                &[],
                &import.tree,
            );
        }
    }
    let Some(imported) = imports
        .into_iter()
        .find_map(|(alias, imported)| (unraw(&alias) == name).then_some(imported))
    else {
        return UseTarget::Unresolved;
    };
    let mut target: Option<ImportedTarget> = None;
    for (index, slot) in imported.resolved.into_iter().enumerate() {
        if let Some(resolved) = slot {
            if target
                .as_ref()
                .is_some_and(|current| current.canon != resolved.canon)
            {
                return UseTarget::Unresolved;
            }
            namespaces[index] = true;
            target = Some(resolved);
        }
    }
    if target.is_none()
        && let UnknownImport::Unique(external) = imported.unknown
        && external.prim.is_some()
    {
        namespaces[1] = true;
        target = Some(external);
    }
    target.map_or(UseTarget::Unresolved, |target| {
        UseTarget::Item(target, namespaces)
    })
}

/// The primitive an external `std::primitive` or `core::primitive` path
/// names, when it names one.
pub(crate) fn external_prim(path: &[String]) -> Option<PrimTy> {
    if let [first, middle, third] = path
        && middle == "primitive"
        && (first == "std" || first == "core")
    {
        return crate::scope::primitive_suffix(third);
    }
    None
}

/// The proven primitive a type alias names, its target resolved through
/// the sibling items, the chain bounded to sixteen hops.
fn alias_chain_prim<'a>(
    alias: &syn::ItemType,
    siblings: &(impl Iterator<Item = &'a syn::Item> + Clone),
    prim_fallback: bool,
    hops: usize,
) -> Option<PrimTy> {
    if hops > 16 || !prim_fallback || !alias.generics.params.is_empty() {
        return None;
    }
    let target = alpha::single_segment_type_name(&alias.ty)?;
    let target = unraw(&target);
    for item in siblings.clone() {
        match item {
            syn::Item::Type(other) if ident_name(&other.ident) == target => {
                return alias_chain_prim(other, siblings, prim_fallback, hops + 1);
            }
            syn::Item::Use(use_item) => {
                if alpha::imported_name_matches(&use_item.tree, |ident| ident_name(ident) == target)
                {
                    return None;
                }
            }
            other => {
                if let Some(ident) = alpha::type_item_ident(other)
                    && ident_name(ident) == target
                {
                    return None;
                }
            }
        }
    }
    crate::scope::primitive_suffix(target)
}

fn alias_type_path(mut ty: &syn::Type) -> Option<&syn::Path> {
    for _ in 0..64 {
        match ty {
            syn::Type::Paren(paren) => ty = &paren.elem,
            syn::Type::Group(group) => ty = &group.elem,
            syn::Type::Path(path)
                if path.qself.is_none()
                    && path
                        .path
                        .segments
                        .iter()
                        .all(|segment| segment.arguments.is_empty()) =>
            {
                return Some(&path.path);
            }
            _ => return None,
        }
    }
    None
}

fn resolved_alias_prim(
    scopes: &BTreeMap<Vec<String>, ModuleScope<'_>>,
    module: &[String],
    alias: &syn::ItemType,
    depth: usize,
    active: Option<&ImportPath<'_>>,
) -> Option<PrimTy> {
    if depth >= 64 || !alias.generics.params.is_empty() || blocking_attr(&alias.attrs) {
        return None;
    }
    let scope = scopes.get(module)?;
    if !scope_primitives_known(scope.items()) {
        return None;
    }
    let path = alias_type_path(&alias.ty)?;
    let names: Vec<String> = path
        .segments
        .iter()
        .map(|segment| ident_name(&segment.ident))
        .collect();
    match resolve_use_at(
        scopes,
        module,
        &names,
        path.leading_colon.is_some(),
        depth + 1,
        active,
    ) {
        UseTarget::Item(target, namespaces) if namespaces[1] => target.prim,
        UseTarget::External => external_prim(&names),
        UseTarget::Unresolved if names.len() == 1 && !scope.shadows_primitive_crate(&names[0]) => {
            crate::scope::primitive_suffix(&names[0])
        }
        _ => None,
    }
}

fn seed_alias_prim(
    scopes: &BTreeMap<Vec<String>, ModuleScope<'_>>,
    module: &[String],
    alias: &syn::ItemType,
    frame: &Frame,
    prim_fallback: bool,
) -> Option<PrimTy> {
    if !alias.generics.params.is_empty() || blocking_attr(&alias.attrs) {
        return None;
    }
    let path = alias_type_path(&alias.ty)?;
    if path.leading_colon.is_none() && path.segments.len() == 1 {
        let name = ident_name(&path.segments[0].ident);
        return match frame.lookup(Ns::Type, &name) {
            Some(binding) => binding.prim,
            None if prim_fallback => crate::scope::primitive_suffix(&name),
            None => None,
        };
    }
    resolved_alias_prim(scopes, module, alias, 0, None)
}

fn bind_item_ident(
    frame: &mut Frame,
    module: &[String],
    ns: Ns,
    ident: &syn::Ident,
    origin: Origin,
    prim: Option<PrimTy>,
    next_id: &mut usize,
) {
    let name = ident.to_string();
    let binding = Binding {
        id: BindingId(*next_id),
        canon: qualified_name(module, &name),
        origin,
        target: None,
        raw: false,
        prim,
        port: 0,
        value_port: None,
        helper: None,
    };
    frame.bind(ns, &name, binding);
    *next_id += 1;
}

/// The identity a module item seeds at its own spelling: the namespace it
/// occupies, its identifier, and the prim its alias target proves.
fn item_seed<'a>(
    item: &'a syn::Item,
    siblings: &(impl Iterator<Item = &'a syn::Item> + Clone),
    prim_fallback: bool,
) -> Option<(Ns, &'a syn::Ident, Origin, Option<PrimTy>)> {
    match item {
        syn::Item::Fn(i) => Some((Ns::Value, &i.sig.ident, Origin::Item, None)),
        syn::Item::Const(i) => Some((Ns::Value, &i.ident, Origin::Item, None)),
        syn::Item::Static(i) => Some((Ns::Value, &i.ident, Origin::Item, None)),
        syn::Item::Struct(i) => Some((Ns::Type, &i.ident, Origin::Item, None)),
        syn::Item::Enum(i) => Some((Ns::Type, &i.ident, Origin::Item, None)),
        syn::Item::Union(i) => Some((Ns::Type, &i.ident, Origin::Item, None)),
        syn::Item::Trait(i) => Some((Ns::Type, &i.ident, Origin::Item, None)),
        syn::Item::TraitAlias(i) => Some((Ns::Type, &i.ident, Origin::Item, None)),
        syn::Item::Mod(i) => Some((Ns::Type, &i.ident, Origin::Item, None)),
        syn::Item::Type(i) => Some((
            Ns::Type,
            &i.ident,
            Origin::TypeAlias,
            alias_chain_prim(i, siblings, prim_fallback, 0),
        )),
        _ => None,
    }
}

/// The seed frame of one module scope: its items at their identity
/// spelling, the prims its type aliases and use imports prove.
pub(crate) fn seed_frame(
    scopes: &BTreeMap<Vec<String>, ModuleScope<'_>>,
    path: &[String],
    scope: &ModuleScope<'_>,
    prim_fallback: bool,
) -> Frame {
    seed_scope(scopes, path, &scope.items().iter(), None, prim_fallback)
}

pub(crate) fn seed_items(
    scopes: &BTreeMap<Vec<String>, ModuleScope<'_>>,
    module: &[String],
    items: &[&syn::Item],
    outer: &Frame,
    prim_fallback: bool,
) -> Frame {
    seed_scope(
        scopes,
        module,
        &items.iter().copied(),
        Some(outer),
        prim_fallback,
    )
}

fn seed_scope<'a>(
    scopes: &BTreeMap<Vec<String>, ModuleScope<'_>>,
    path: &[String],
    items: &(impl Iterator<Item = &'a syn::Item> + Clone),
    outer: Option<&Frame>,
    prim_fallback: bool,
) -> Frame {
    let mut frame = outer.cloned().unwrap_or_default();
    let mut next_id = frame.next_id();
    let mut imports = BTreeMap::new();
    for item in items.clone() {
        if let Some((ns, ident, origin, prim)) =
            item_seed(item, items, prim_fallback && outer.is_none())
        {
            bind_item_ident(&mut frame, path, ns, ident, origin, prim, &mut next_id);
            if let syn::Item::Struct(item) = item
                && !matches!(item.fields, syn::Fields::Named(_))
            {
                bind_item_ident(
                    &mut frame,
                    path,
                    Ns::Value,
                    &item.ident,
                    Origin::Item,
                    None,
                    &mut next_id,
                );
            }
            continue;
        }
        match item {
            syn::Item::Use(use_item) => {
                use_tree_bindings(
                    &mut UseBinder {
                        scopes,
                        from: path,
                        leading_colon: use_item.leading_colon.is_some(),
                        imports: &mut imports,
                        only_name: None,
                        depth: 0,
                        active: None,
                    },
                    &[],
                    &use_item.tree,
                );
            }
            syn::Item::Macro(i) => {
                if let Some(ident) = &i.ident {
                    bind_item_ident(
                        &mut frame,
                        path,
                        Ns::Macro,
                        ident,
                        Origin::MacroRule,
                        None,
                        &mut next_id,
                    );
                }
            }
            _ => {}
        }
    }
    bind_imports(&mut frame, path, imports, &mut next_id);
    for _ in 0..16 {
        let mut changed = false;
        for item in items.clone() {
            if let syn::Item::Type(alias) = item {
                if outer.is_none()
                    && alias_type_path(&alias.ty).is_some_and(|path| path.segments.len() == 1)
                {
                    continue;
                }
                let name = ident_name(&alias.ident);
                let lexical_shadow = outer.is_some()
                    && alias_type_path(&alias.ty).is_some_and(|target| {
                        target.leading_colon.is_none()
                            && target.segments.len() > 1
                            && target.segments.first().is_some_and(|first| {
                                let first_name = ident_name(&first.ident);
                                !matches!(first_name.as_str(), "crate" | "self" | "super")
                                    && items.clone().any(|item| {
                                        alpha::type_item_ident(item)
                                            .is_some_and(|ident| ident_name(ident) == first_name)
                                    })
                            })
                    });
                let prim = if lexical_shadow {
                    None
                } else {
                    seed_alias_prim(scopes, path, alias, &frame, prim_fallback)
                };
                if let Some(binding) = frame.bindings_mut(Ns::Type).get_mut(&name)
                    && binding.origin == Origin::TypeAlias
                    && binding.prim != prim
                {
                    binding.prim = prim;
                    changed = true;
                }
            }
        }
        if !changed {
            break;
        }
    }
    frame
}

/// Bind one generic parameter in its namespaces, one id shared by every
/// namespace the parameter occupies.
fn bind_generic_param(
    frame: &mut Frame,
    next_id: &mut usize,
    ns: Ns,
    ident: &syn::Ident,
    origin: Origin,
    owner: &str,
) {
    let name = ident.to_string();
    let mut canon = String::from(owner);
    qualified_part(&mut canon, unraw(&name));
    let binding = Binding {
        id: BindingId(*next_id),
        canon,
        origin,
        target: None,
        raw: false,
        prim: None,
        port: 0,
        value_port: None,
        helper: None,
    };
    frame.bind(ns, &name, binding);
    *next_id += 1;
}

/// Shared namespace origins qualified by the enclosing generic definition.
fn generics_frame(
    module: ModuleId,
    modules: &[Module],
    generics: &syn::Generics,
    owner: &str,
) -> Frame {
    let mut frame = Frame::default();
    let mut next_id = modules[module.0].seed.next_id();
    for param in &generics.params {
        match param {
            syn::GenericParam::Type(ty_param) => {
                bind_generic_param(
                    &mut frame,
                    &mut next_id,
                    Ns::Type,
                    &ty_param.ident,
                    Origin::Generic,
                    owner,
                );
            }
            syn::GenericParam::Lifetime(lifetime) => {
                bind_generic_param(
                    &mut frame,
                    &mut next_id,
                    Ns::Lifetime,
                    &lifetime.lifetime.ident,
                    Origin::Lifetime,
                    owner,
                );
            }
            syn::GenericParam::Const(r#const) => {
                let name = r#const.ident.to_string();
                let mut canon = String::from(owner);
                qualified_part(&mut canon, unraw(&name));
                let binding = Binding {
                    id: BindingId(next_id),
                    canon,
                    origin: Origin::Generic,
                    target: None,
                    raw: false,
                    prim: None,
                    port: 0,
                    value_port: None,
                    helper: None,
                };
                frame.bind(Ns::Value, &name, binding.clone());
                frame.bind(Ns::Type, &name, binding);
                next_id += 1;
            }
        }
    }
    frame
}

/// Index every function `items` declares, nested modules included.
#[expect(
    clippy::too_many_arguments,
    reason = "the index carries its original module and observer evidence"
)]
#[expect(
    clippy::too_many_lines,
    reason = "each item kind carries its original owner and ancestry"
)]
fn index_scope<'a>(
    decls: &mut BTreeMap<(*const syn::Signature, *const syn::Block), Decl<'a>>,
    module_ids: &BTreeMap<Vec<String>, ModuleId>,
    modules: &[Module],
    path: &[String],
    items: &'a [syn::Item],
    observed: bool,
    helper_blocked: bool,
    gen_frames: &mut BTreeMap<*const syn::Generics, Frame>,
) {
    let module = *module_ids.get(path).expect("a module id");
    let prim_fallback = modules[module.0].prim_fallback;
    let observed = observed || modules[module.0].observed;
    let helper_blocked = helper_blocked || modules[module.0].helper_blocked;
    for item in items {
        match item {
            syn::Item::Fn(f) => {
                index_fn(
                    decls,
                    FnDecl {
                        module,
                        prim_fallback,
                        owner: Owner::Free,
                        observed,
                        helper_module: path,
                        helper_blocked,
                        attrs: &f.attrs,
                        generics: None,
                    },
                    &f.sig,
                    &f.block,
                );
            }
            syn::Item::Mod(m) => {
                if let Some((_, inner)) = &m.content {
                    let mut child = path.to_vec();
                    child.push(unraw(&m.ident.to_string()).to_string());
                    index_scope(
                        decls,
                        module_ids,
                        modules,
                        &child,
                        inner,
                        observed || live_attr(&m.attrs),
                        helper_blocked || blocking_attr(&m.attrs),
                        gen_frames,
                    );
                }
            }
            syn::Item::Impl(i) => {
                let text = i.self_ty.to_token_stream().to_string();
                let owner = if i.trait_.is_some() {
                    Owner::Trait(text)
                } else {
                    Owner::Inherent(text)
                };
                let observed = observed || live_attr(&i.attrs);
                let key = &raw const i.generics;
                gen_frames
                    .entry(key)
                    .or_insert_with(|| impl_generics_frame(module, modules, path, i));
                for member in &i.items {
                    if let syn::ImplItem::Fn(f) = member {
                        index_fn(
                            decls,
                            FnDecl {
                                module,
                                prim_fallback,
                                owner: owner.clone(),
                                observed,
                                helper_module: path,
                                helper_blocked: helper_blocked || blocking_attr(&i.attrs),
                                attrs: &f.attrs,
                                generics: Some(key),
                            },
                            &f.sig,
                            &f.block,
                        );
                    }
                }
            }
            syn::Item::Trait(t) => {
                let text = t.ident.to_string();
                let observed = observed || live_attr(&t.attrs);
                let key = &raw const t.generics;
                gen_frames.entry(key).or_insert_with(|| {
                    let ident = &t.ident;
                    let generics = &t.generics;
                    let supertraits = &t.supertraits;
                    let predicates = &t.generics.where_clause;
                    let header = quote::quote!(trait #ident #generics : #supertraits #predicates);
                    let name = qualified_name(path, &header.to_string());
                    generics_frame(module, modules, &t.generics, &name)
                });
                for member in &t.items {
                    if let syn::TraitItem::Fn(f) = member
                        && let Some(block) = &f.default
                    {
                        index_fn(
                            decls,
                            FnDecl {
                                module,
                                prim_fallback,
                                owner: Owner::Trait(text.clone()),
                                observed,
                                attrs: &f.attrs,
                                helper_module: path,
                                helper_blocked: helper_blocked || blocking_attr(&t.attrs),
                                generics: Some(key),
                            },
                            &f.sig,
                            block,
                        );
                    }
                }
            }
            _ => {}
        }
    }
}

fn impl_generics_frame(
    module: ModuleId,
    modules: &[Module],
    path: &[String],
    item: &syn::ItemImpl,
) -> Frame {
    let generics = &item.generics;
    let self_ty = &item.self_ty;
    let trait_path = item.trait_.as_ref().map(|(path, _)| path);
    let predicates = &item.generics.where_clause;
    let header = quote::quote!(impl #generics #trait_path for #self_ty #predicates);
    let name = qualified_name(path, &header.to_string());
    generics_frame(module, modules, &item.generics, &name)
}

/// The module facts one indexed function is recorded with.
struct FnDecl<'a, 'path> {
    module: ModuleId,
    prim_fallback: bool,
    owner: Owner,
    observed: bool,
    helper_module: &'path [String],
    helper_blocked: bool,
    attrs: &'a [syn::Attribute],
    generics: Option<*const syn::Generics>,
}

fn index_fn<'a>(
    decls: &mut BTreeMap<(*const syn::Signature, *const syn::Block), Decl<'a>>,
    facts: FnDecl<'a, '_>,
    sig: &syn::Signature,
    block: &syn::Block,
) {
    let key = (
        core::ptr::from_ref::<syn::Signature>(sig),
        core::ptr::from_ref::<syn::Block>(block),
    );
    decls.insert(
        key,
        Decl {
            module: facts.module,
            attrs: facts.attrs,
            owner: facts.owner,
            prim_fallback: facts.prim_fallback,
            observed: facts.observed || live_attr(facts.attrs),
            helper_module: facts.helper_module.to_vec(),
            helper_blocked: facts.helper_blocked || blocking_attr(facts.attrs),
            generics: facts.generics,
        },
    );
}

/// Whether `meta` takes part in the comparison. `cfg`, the hints that
/// leave the body's meaning alone, the attributes that export a symbol,
/// and a `cfg_attr` wrapping only those do not.
pub(crate) fn live(meta: &syn::Meta) -> bool {
    let path = meta.path();
    if path.is_ident("cfg_attr") {
        return wrapped(meta, 1).is_none_or(|metas| metas.iter().any(live));
    }
    if path.is_ident("unsafe") {
        return wrapped(meta, 0).is_none_or(|metas| metas.iter().any(live));
    }
    // A `doc` computed by a macro runs at compile time and can fail.
    if path.is_ident("doc")
        && matches!(meta, syn::Meta::NameValue(nv) if matches!(nv.value, syn::Expr::Macro(_)))
    {
        return true;
    }
    let inert = [
        "cfg",
        "inline",
        "cold",
        "must_use",
        "track_caller",
        "doc",
        "allow",
        "warn",
        "deny",
        "forbid",
        "expect",
        "no_mangle",
        "export_name",
    ];
    !inert.iter().any(|name| path.is_ident(name))
        && path.segments.first().is_none_or(|s| s.ident != "rustfmt")
}

/// Whether any attribute in `attrs` takes part in the comparison.
pub(crate) fn live_attr(attrs: &[syn::Attribute]) -> bool {
    attrs.iter().any(|attr| live(&attr.meta))
}

/// Whether `meta` can change a helper's body or observe its caller.
pub(crate) fn blocking_meta(meta: &syn::Meta) -> bool {
    let path = meta.path();
    path.is_ident("cfg")
        || path.is_ident("cfg_attr")
        || path.is_ident("track_caller")
        || (path.is_ident("unsafe")
            && wrapped(meta, 0).is_none_or(|metas| metas.iter().any(blocking_meta)))
        || live(meta)
}

/// Whether any attribute in `attrs` can change a helper's denotation.
pub(crate) fn blocking_attr(attrs: &[syn::Attribute]) -> bool {
    attrs.iter().any(|attr| blocking_meta(&attr.meta))
}

/// The metas `meta` wraps after its first `skip` arguments, `None` when
/// they don't parse.
fn wrapped(meta: &syn::Meta, skip: usize) -> Option<Vec<syn::Meta>> {
    let syn::Meta::List(list) = meta else {
        return None;
    };
    let parser = syn::punctuated::Punctuated::<syn::Meta, syn::Token![,]>::parse_terminated;
    list.parse_args_with(parser)
        .ok()
        .map(|metas| metas.into_iter().skip(skip).collect())
}
