# syn-canon

[![CI](https://github.com/LucaCappelletti94/cargo-dejadoc/actions/workflows/ci.yml/badge.svg)](https://github.com/LucaCappelletti94/cargo-dejadoc/actions/workflows/ci.yml)
[![crates.io](https://img.shields.io/crates/v/syn-canon.svg)](https://crates.io/crates/syn-canon)
[![docs.rs](https://docs.rs/syn-canon/badge.svg)](https://docs.rs/syn-canon)

Canonical forms of [`syn`](https://docs.rs/syn) files. Formatting, supported style folds and local binder names share one comparison key.

```rust
let canonical = |src: &str| syn_canon::canonicalize(syn::parse_str(src).unwrap());

assert_eq!(
    canonical("fn f(x: u32) -> u32 { return (x + 1); }"),
    canonical("fn g(y: u32) -> u32 { y + 1 }"),
);
assert_ne!(
    canonical("fn f(x: u32) -> u32 { x + 1 }"),
    canonical("fn f(x: u64) -> u64 { x + 1 }"),
);
```

`no_std` with `alloc`. It powers [`cargo-dejadoc`](https://github.com/LucaCappelletti94/cargo-dejadoc), which finds duplicated doctests.

`canonicalize` and `canonicalize_failing` return `CanonicalForm`. Its key is opaque and cannot be parsed as Rust, with `Eq`, `Hash` and `Display` sharing that key. `leaf_tokens()` counts canonical leaves, while `body_units()` counts body paths, lifetimes and operators as one and excludes commas and delimiters.

`SourceContext` indexes borrowed module files and selects only their original signature/body pairs. `FunctionView` preserves qualified inherited references and owner generics while alpha-renaming declarations inside the view.

Comparison keys retain proven fixed-width scalar types through resolved aliases. Unknown macros, syntax observers and storage observations restrict dependency proofs. Proof metadata contributes no size units.

## Folds

Every binder becomes a positional name in visit order: `let` and closure patterns, function names and parameters, generics, lifetimes, labels, local items, `use` aliases and `macro_rules!` names. Item names of a file, block or inline module are bound before the list is visited, so a use may precede its definition. Free names, fields, methods and attribute paths stay, and so do string literals. `Self` in type position expands to the impl self type. Macro arguments that parse as an expression list or `elem; count` are renamed as expressions, otherwise token by token.

Value, type and macro bindings treat `r#name` and `name` as the same identifier, including shadowing and module members. Lifetimes and labels retain their spelling. Macro references involving a raw binding or reference retain their spelling because macros can inspect tokens.

Style folds, each applied only where it keeps the meaning:

- `use` items flatten to one sorted item per leaf path at the front of their list.
- `{ expr }` arm and closure bodies become `expr`, and no arm keeps its comma.
- `else { if … }` becomes `else if …`.
- A tail `return v` becomes `v`, also in the arms of a tail `match`, and a trailing unit `return` goes.
- Redundant parentheses around expressions, patterns and types go, and so do `-> ()` and empty statements.
- When the code compiles, `mut` goes from `mut self` and from a binding that is the whole pattern of a `let`, parameter, closure input, arm, `for` or `let` condition, the semicolon after a non-tail block statement goes, and so does the tail semicolon of a function body without a return type, a loop body and the then blocks of an `if` chain without a final `else`. A nested `mut` stays, since under a reference it resets the binding mode before edition 2024. `canonicalize_failing` keeps them all for code meant to fail, such as a `compile_fail` doctest.
- Inline generic bounds move into `where`, `&'_ T` becomes `&T`, `Foo<'_>` becomes `Foo` outside an impl header, and `&'static` becomes `&` in a `const` or `static` type.
- Type paths omit `::` before generic arguments, bound lists omit a trailing `+`, and simple trait bounds omit parentheses. Function-trait output bounds retain grouping parentheses.
- Function-pointer and function-trait types omit a unit return, `self: &Self` becomes `&self` with named lifetimes preserved, and `self: Self` becomes `self`. These syntax folds also apply to `canonicalize_failing`.
- Empty-body closures omit `-> ()`. Nonempty closure return annotations stay.
- Unary minus goes from zero literals with an explicit signed integer suffix, such as `-0i8`. Unsuffixed integers, unsigned integers and floating-point zero retain the minus.
- When the code compiles, an empty, unlabeled, attribute-free `else {}` goes.
- When the code compiles, a function pointer with one lifetime binder used only by its single direct reference input elides that binder and reference lifetime. Bounds, shared output lifetimes, nested binders and opaque syntax stay.
- When the code compiles, a `const` block containing only a scalar literal becomes that literal, including array lengths and const-generic arguments. Scalar const-generic arguments omit redundant braces, while strings and nonliteral bodies stay.
- When the code compiles, private, attribute-free `usize` integer constants used only as direct array lengths are substituted and removed together. Other uses, shadowing, raw spellings, opaque references, imports and unresolved macros or modules keep the declaration.
- Empty angle-bracketed path and method arguments go. Nonempty expression and method turbofish arguments stay, and `use<>` capture bounds stay.
- Generic declaration and argument lists and closure parameter lists omit trailing commas without changing element order or tuple arity.
- `pub(in crate)`, `pub(in self)` and `pub(in super)` omit `in`.
- When the code compiles, an omitted `extern` ABI becomes `"C"`, `return ()` becomes `return`, leading pattern pipes go, and a non-uppercase `x @ _` becomes `x` with its binding mode preserved. `canonicalize_failing` keeps these spellings.
- These syntax folds preserve procedural macro inputs, including fields, arguments and nested syntax. Attribute payloads remain opaque, including reference lifetimes and macro delimiters.
- Unparsed `Verbatim` nodes retain generic-list punctuation and pattern-binding spelling.
- Identifier arguments of the std formatting macros move into the format string, `println!("{}", x)` becoming `println!("{x}")`.
- Macro calls take parentheses, and every statement macro but the tail one takes a semicolon.
- `derive` lists of std derives merge into one sorted list.
- `doc`, lint level and `rustfmt::` attributes go, except in the input of a proc macro derive, which reads them. A `doc` whose value is a macro call runs at compile time and stays.
- Literals take one spelling, `0x10` and `16` alike, and trailing commas go, except in the input of a macro or attribute that may match on them. The std macros that ignore one, `vec!` and `println!` among them, still lose it.

## Context blocks

`contexts` emits independently normalized function bodies, explicit blocks, complete match arms and complete closures with syntax-tree spans and containing items. Methods, trait defaults and nested functions are included, and explicit blocks in required trait and foreign signatures retain their function names. Arm patterns/guards and closure inputs remain in the form, and unused preceding declarations do not affect numbering.

`Context::span` retains the supplied syntax tree's coordinates, so `syn::parse_file` callers must account for removed BOM and shebang bytes when indexing their original text.

```rust
let forms = |src: &str| {
    let file = syn::parse_str(src).unwrap();
    let mut form = String::new();
    syn_canon::contexts(&file, &mut |_context, canonical| {
        form = canonical.into_key();
    });
    form
};
assert_eq!(
    forms("fn f(x: u32) -> u32 { x + x }"),
    forms("fn g(y: u32) -> u32 { y + y }"),
);
```

The `ContextKind` names the boundary. Candidate-local binders receive fresh declaration-order numbers from `0`, with namespace tags, and inherited captures receive first-reference numbers from `0`. Unused inherited declarations stay outside the form, and explicit import aliases retain their available target associations.

Namespaces remain distinct, while a declaration shared by value and type positions retains one origin number. Inherited nominal trait-bound paths retain their available spelling or import association, preserving inference-sensitive bound order. Trait `Self` bindings end at the trait boundary.

Raw imported segments retain their spelling. An absolute path keeps `::` when a candidate-local or inherited type binding shadows its root, and absolute import targets bypass lexical aliases.

Equal canonical forms are approximate duplication. The normalization cannot see what the compiler resolves, so two equal forms may still differ in a receiver type, an inferred type or lifetime, an unresolved import, a macro's generated code, an enclosing `cfg`, or the target of a `return`, `break` or `continue`. A context form claims no extraction and no deletion.

## Stability

`CanonicalForm` keys and hashes are comparable within one major version. New folds require a major version.
