//! Original module environments and borrowed function declarations.

use alloc::boxed::Box;
use alloc::collections::BTreeMap;
use alloc::collections::BTreeSet;
use alloc::string::String;
use alloc::string::ToString;
use alloc::vec;
use alloc::vec::Vec;

use quote::ToTokens;

use crate::alpha;
use crate::form;
use crate::scope::{Binding, BindingId, Frame, Ns, Origin, ident_name, unraw};

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
        if let Some(name) = single_ident(text)
            && let Some(binding) = seed.lookup(Ns::Type, name)
        {
            return Some(binding.canon.clone());
        }
        Some(mangle_owner(text))
    }
}

/// The unraw text when it is one identifier, `r#` dropped.
fn single_ident(text: &str) -> Option<&str> {
    let name = unraw(text);
    (name.chars().all(|c| c.is_alphanumeric() || c == '_') && !name.is_empty()).then_some(name)
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

/// One indexed module: its seed frame.
struct Module {
    seed: Frame,
}

/// One original declaration and its module environment.
struct Decl<'a> {
    module: ModuleId,
    /// The original declaration attributes.
    attrs: &'a [syn::Attribute],
    owner: Owner,
    /// The original enclosing impl or trait generics.
    generics: Option<*const syn::Generics>,
}

/// The source context, one entry per module file and inline module.
pub struct SourceContext<'a> {
    modules: Vec<Module>,
    decls: BTreeMap<(*const syn::Signature, *const syn::Block), Decl<'a>>,
    /// Generic frames indexed by their original declarations.
    gen_frames: BTreeMap<*const syn::Generics, Frame>,
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
        let item = syn::ItemFn {
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
                self_canon,
            },
        )
    }
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
    /// inline modules under their own path and seed.
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
        let mut module_ids: BTreeMap<Vec<String>, ModuleId> = BTreeMap::new();
        let mut modules: Vec<Module> = Vec::new();
        for (path, scope) in &scopes {
            let seed = seed_frame(&scopes, path, scope);
            module_ids.insert(path.clone(), ModuleId(modules.len()));
            modules.push(Module { seed });
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
                &mut gen_frames,
            );
        }
        Self {
            modules,
            decls,
            gen_frames,
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

/// A module's items, inline content included.
#[derive(Clone, Copy)]
enum ModuleScope<'a> {
    File(&'a syn::File),
    Inline(&'a [syn::Item]),
}

impl<'a> ModuleScope<'a> {
    fn items(&self) -> &'a [syn::Item] {
        match self {
            Self::File(file) => &file.items,
            Self::Inline(items) => items,
        }
    }

    fn inline_mod(&self, name: &str) -> Option<ModuleScope<'a>> {
        for item in self.items() {
            if let syn::Item::Mod(mod_item) = item
                && mod_item.ident == name
                && let Some((_, inner)) = &mod_item.content
            {
                return Some(ModuleScope::Inline(inner));
            }
        }
        None
    }

    fn shadows_primitive_crate(&self, name: &str) -> bool {
        let raw = if name == "core" { "r#core" } else { "r#std" };
        let matches = |ident: &syn::Ident| ident == name || ident == raw;
        self.items().iter().any(|item| {
            type_item_ident(item).is_some_and(matches)
                || matches!(item, syn::Item::Use(import)
                    if imported_name_matches(&import.tree, matches))
        })
    }
}

/// Register every inline module body under its module path.
fn register_inline<'a>(
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

fn qualified_name(module: &[String], name: &str) -> String {
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

fn qualified_part(out: &mut String, part: &str) {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    out.push('_');
    for byte in part.bytes() {
        out.push(char::from(HEX[usize::from(byte >> 4)]));
        out.push(char::from(HEX[usize::from(byte & 15)]));
    }
}

/// One resolved `use` target.
enum UseTarget {
    /// An external path: `std`, `core`, or a leading `::`.
    External,
    /// A crate item the context resolved, its qualified name.
    Item(String),
    /// A name the context could not resolve.
    Unresolved,
}

/// The module map, declaration frame, and identity cursor for imported names.
struct UseBinder<'a> {
    scopes: &'a BTreeMap<Vec<String>, ModuleScope<'a>>,
    from: &'a [String],
    leading_colon: bool,
    frame: &'a mut Frame,
    visited: &'a mut BTreeSet<String>,
    next_id: &'a mut usize,
}

/// Bind every explicit local name in one import tree.
fn seed_use_bindings(
    scopes: &BTreeMap<Vec<String>, ModuleScope<'_>>,
    from: &[String],
    tree: &syn::UseTree,
    leading_colon: bool,
    frame: &mut Frame,
    next_id: &mut usize,
) {
    let mut visited = BTreeSet::new();
    use_tree_bindings(
        &mut UseBinder {
            scopes,
            from,
            leading_colon,
            frame,
            visited: &mut visited,
            next_id,
        },
        &[],
        tree,
    );
}

fn use_tree_bindings(binder: &mut UseBinder<'_>, prefix: &[String], tree: &syn::UseTree) {
    match tree {
        syn::UseTree::Path(path) => {
            let mut next = prefix.to_vec();
            next.push(unraw(&path.ident.to_string()).to_string());
            use_tree_bindings(binder, &next, &path.tree);
        }
        syn::UseTree::Name(name) => {
            let mut path = prefix.to_vec();
            path.push(unraw(&name.ident.to_string()).to_string());
            bind_use_name(binder, &path, &name.ident.to_string());
        }
        syn::UseTree::Rename(rename) => {
            let mut path = prefix.to_vec();
            path.push(unraw(&rename.ident.to_string()).to_string());
            bind_use_name(binder, &path, &rename.rename.to_string());
        }
        syn::UseTree::Group(group) => {
            for inner in &group.items {
                use_tree_bindings(binder, prefix, inner);
            }
        }
        syn::UseTree::Glob(_) => {}
    }
}

/// Bind one imported name, the path it came from and the alias it lands on.
fn bind_use_name(binder: &mut UseBinder<'_>, path: &[String], alias: &str) {
    if !binder.visited.insert(alias.to_string()) {
        return;
    }
    let canon = match resolve_use(binder.scopes, binder.from, path, binder.leading_colon) {
        UseTarget::External => qualified_name(&path[..path.len() - 1], &path[path.len() - 1]),
        UseTarget::Item(name) => name,
        UseTarget::Unresolved => qualified_name(binder.from, alias),
    };
    let binding = Binding {
        id: BindingId(*binder.next_id),
        canon,
        origin: Origin::Use,
        target: None,
        raw: false,
    };
    *binder.next_id += 1;
    binder.frame.bind(Ns::Value, alias, binding.clone());
    binder.frame.bind(Ns::Type, alias, binding.clone());
    binder.frame.bind(Ns::Macro, alias, binding);
}

/// Resolve a use path against the module map, `from` the importing module.
fn resolve_use(
    scopes: &BTreeMap<Vec<String>, ModuleScope<'_>>,
    from: &[String],
    path: &[String],
    leading_colon: bool,
) -> UseTarget {
    if path.is_empty() {
        return UseTarget::Unresolved;
    }
    let first = unraw(&path[0]);
    if leading_colon {
        return UseTarget::External;
    }
    let Some(local) = scopes.get(from) else {
        return UseTarget::Unresolved;
    };
    if matches!(first, "std" | "core") && !local.shadows_primitive_crate(first) {
        return UseTarget::External;
    }
    let (mut base, rest) = match first {
        "crate" => (Vec::new(), &path[1..]),
        "self" => (from.to_vec(), &path[1..]),
        "super" => {
            if from.is_empty() {
                return UseTarget::Unresolved;
            }
            let mut base = from.to_vec();
            base.pop();
            (base, &path[1..])
        }
        _ => (from.to_vec(), path),
    };
    let Some(mut scope) = scopes.get(&base).copied() else {
        return UseTarget::Unresolved;
    };
    for (index, name) in rest.iter().enumerate() {
        let name = unraw(name);
        if index + 1 == rest.len() {
            return resolve_item(scope.items(), &base, name);
        }
        let mut next_path = base.clone();
        next_path.push(name.to_string());
        let next = scopes
            .get(&next_path)
            .copied()
            .or_else(|| scope.inline_mod(name));
        base.push(name.to_string());
        scope = match next {
            Some(scope) => scope,
            None => return UseTarget::Unresolved,
        };
    }
    UseTarget::Unresolved
}

fn resolve_item(items: &[syn::Item], module: &[String], name: &str) -> UseTarget {
    let item = items
        .iter()
        .find(|item| match item {
            syn::Item::Fn(i) => ident_name(&i.sig.ident) == name,
            syn::Item::Const(i) => ident_name(&i.ident) == name,
            syn::Item::Static(i) => ident_name(&i.ident) == name,
            _ => false,
        })
        .or_else(|| {
            items
                .iter()
                .find(|item| type_item_ident(item).is_some_and(|ident| ident_name(ident) == name))
        });
    match item {
        Some(_) => UseTarget::Item(qualified_name(module, name)),
        None => UseTarget::Unresolved,
    }
}

/// The identifier of a type-namespace item, `use` and `macro_rules!` aside.
fn type_item_ident(item: &syn::Item) -> Option<&syn::Ident> {
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

/// Whether any local name the import tree binds satisfies `matches`.
fn imported_name_matches(
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

fn bind_item_ident(
    frame: &mut Frame,
    module: &[String],
    ns: Ns,
    ident: &syn::Ident,
    origin: Origin,
    next_id: &mut usize,
) {
    let name = ident.to_string();
    let binding = Binding {
        id: BindingId(*next_id),
        canon: qualified_name(module, &name),
        origin,
        target: None,
        raw: false,
    };
    frame.bind(ns, &name, binding);
    *next_id += 1;
}

/// The namespace, name and origin of a module item.
fn item_seed(item: &syn::Item) -> Option<(Ns, &syn::Ident, Origin)> {
    match item {
        syn::Item::Fn(i) => Some((Ns::Value, &i.sig.ident, Origin::Item)),
        syn::Item::Const(i) => Some((Ns::Value, &i.ident, Origin::Item)),
        syn::Item::Static(i) => Some((Ns::Value, &i.ident, Origin::Item)),
        syn::Item::Struct(i) => Some((Ns::Type, &i.ident, Origin::Item)),
        syn::Item::Enum(i) => Some((Ns::Type, &i.ident, Origin::Item)),
        syn::Item::Union(i) => Some((Ns::Type, &i.ident, Origin::Item)),
        syn::Item::Trait(i) => Some((Ns::Type, &i.ident, Origin::Item)),
        syn::Item::TraitAlias(i) => Some((Ns::Type, &i.ident, Origin::Item)),
        syn::Item::Mod(i) => Some((Ns::Type, &i.ident, Origin::Item)),
        syn::Item::Type(i) => Some((Ns::Type, &i.ident, Origin::TypeAlias)),
        _ => None,
    }
}

/// The seed frame of one module scope: its items at their qualified
/// identity spelling, its `use` imports resolved through the module map.
fn seed_frame(
    scopes: &BTreeMap<Vec<String>, ModuleScope<'_>>,
    path: &[String],
    scope: &ModuleScope<'_>,
) -> Frame {
    let mut frame = Frame::default();
    let mut next_id = 0usize;
    for item in scope.items() {
        if let Some((ns, ident, origin)) = item_seed(item) {
            bind_item_ident(&mut frame, path, ns, ident, origin, &mut next_id);
            continue;
        }
        match item {
            syn::Item::Use(use_item) => {
                seed_use_bindings(
                    scopes,
                    path,
                    &use_item.tree,
                    use_item.leading_colon.is_some(),
                    &mut frame,
                    &mut next_id,
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
                        &mut next_id,
                    );
                }
            }
            _ => {}
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
fn index_scope<'a>(
    decls: &mut BTreeMap<(*const syn::Signature, *const syn::Block), Decl<'a>>,
    module_ids: &BTreeMap<Vec<String>, ModuleId>,
    modules: &[Module],
    path: &[String],
    items: &'a [syn::Item],
    gen_frames: &mut BTreeMap<*const syn::Generics, Frame>,
) {
    let module = *module_ids.get(path).expect("a module id");
    for item in items {
        match item {
            syn::Item::Fn(f) => {
                index_fn(
                    decls,
                    FnDecl {
                        module,
                        owner: Owner::Free,
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
                    index_scope(decls, module_ids, modules, &child, inner, gen_frames);
                }
            }
            syn::Item::Impl(i) => {
                let text = type_text(&i.self_ty);
                let owner = if i.trait_.is_some() {
                    Owner::Trait(text)
                } else {
                    Owner::Inherent(text)
                };
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
                                owner: owner.clone(),
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
                                owner: Owner::Trait(text.clone()),
                                attrs: &f.attrs,
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
struct FnDecl<'a> {
    module: ModuleId,
    owner: Owner,
    attrs: &'a [syn::Attribute],
    generics: Option<*const syn::Generics>,
}

fn index_fn<'a>(
    decls: &mut BTreeMap<(*const syn::Signature, *const syn::Block), Decl<'a>>,
    facts: FnDecl<'a>,
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
            generics: facts.generics,
        },
    );
}

/// The whole type as written, `Foo<u8>` and `a::Foo` telling apart types
/// one last segment would merge.
fn type_text(ty: &syn::Type) -> String {
    let printed = ty.to_token_stream().to_string();
    let word = |c: Option<char>| c.is_some_and(|c| c.is_alphanumeric() || c == '_');
    let mut out = String::with_capacity(printed.len());
    let mut chars = printed.chars().peekable();
    while let Some(c) = chars.next() {
        if c != ' ' || (word(out.chars().last()) && word(chars.peek().copied())) {
            out.push(c);
        }
    }
    out
}

/// Whether `meta` takes part in the comparison. `cfg`, the hints that
/// leave the body's meaning alone, the attributes that export a symbol,
/// and a `cfg_attr` wrapping only those do not.
fn live(meta: &syn::Meta) -> bool {
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
