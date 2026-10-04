# syn-canon

[![CI](https://github.com/LucaCappelletti94/cargo-dejadoc/actions/workflows/ci.yml/badge.svg)](https://github.com/LucaCappelletti94/cargo-dejadoc/actions/workflows/ci.yml)
[![crates.io](https://img.shields.io/crates/v/syn-canon.svg)](https://crates.io/crates/syn-canon)
[![docs.rs](https://docs.rs/syn-canon/badge.svg)](https://docs.rs/syn-canon)

Canonical form of a [`syn`](https://docs.rs/syn) file. Two files that differ only in formatting, in style rustfmt or a reviewer would not call a difference, or in the names of their local binders print the same canonical tokens, so their hashes match.

```rust
let canonical = |src: &str| syn_canon::canonicalize(syn::parse_str(src).unwrap()).to_string();

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

## Folds

Every binder becomes a positional name in visit order: `let` and closure patterns, function names and parameters, generics, lifetimes, labels, local items, `use` aliases and `macro_rules!` names. Item names of a file, block or inline module are bound before the list is visited, so a use may precede its definition. Free names, fields, methods and attribute paths stay, and so do string literals. `Self` in type position expands to the impl self type. Macro arguments that parse as an expression list or `elem; count` are renamed as expressions, otherwise token by token.

Style folds, each applied only where it keeps the meaning:

- `use` items flatten to one sorted item per leaf path at the front of their list.
- `{ expr }` arm and closure bodies become `expr`, and no arm keeps its comma.
- `else { if … }` becomes `else if …`.
- A tail `return v` becomes `v`, also in the arms of a tail `match`, and a trailing unit `return` goes.
- Redundant parentheses around expressions, patterns and types go, and so do `-> ()` and empty statements.
- Inline generic bounds move into `where`, `&'_ T` becomes `&T`, `Foo<'_>` becomes `Foo` outside an impl header, and `&'static` becomes `&` in a `const` or `static` type.
- Identifier arguments of the std formatting macros move into the format string, `println!("{}", x)` becoming `println!("{x}")`.
- Macro calls take parentheses, and every statement macro but the tail one takes a semicolon.
- `derive` lists of std derives merge into one sorted list.
- `doc`, lint level and `rustfmt::` attributes go, except in the input of a proc macro derive, which reads them.
- Literals take one spelling, `0x10` and `16` alike, and trailing commas go, except in the input of a macro or attribute that may match on them. The std macros that ignore one, `vec!` and `println!` among them, still lose it.

## Stability

The canonical tokens are the contract. Any change to them, a new fold included, is a new major version, so hashes stored by one version stay comparable within it.
