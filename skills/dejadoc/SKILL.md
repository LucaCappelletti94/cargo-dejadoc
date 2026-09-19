---
name: dejadoc
description: Run cargo dejadoc and resolve duplicated Rust doctests by removing copies, linking the kept example, or allowing a copy with a rust,dejadoc fence. Use when a dejadoc check or review flags a duplicate, or the user asks to deduplicate doctests.
license: MIT
---

# dejadoc

`cargo dejadoc` hashes the canonical form of every doctest rustdoc would run and lists the sites sharing one. Exit `1` on any group, `2` on error.

## What counts as a copy

Cosmetic differences never separate two bodies, so editing them is not a fix.

- whitespace, comments, trailing commas, hidden `# ` lines
- local names, `fn shared() {}` and `fn helper() {}` are one doctest
- `use` order, literal spelling outside macros
- fence attributes such as `no_run`

Different paths, methods, string literals or macro arguments do separate bodies. `ignore` and non-Rust fences are never counted.

## Run

```
cargo dejadoc -v         # each group with its sites and code
cargo dejadoc --json     # {total, unique, groups: [{id, tokens, unparsed, sites: [{file, line, item, info, code}]}]}
```

`-p <member>`, `--all-targets`, `-t <sites>`, `--min-tokens <n>`, `--no-fail`, `--github` for workflow annotations.

## Resolve a group

1. Keep the site whose item the body calls. A crate level example beats a copy on an item.
2. At every other site, delete the block and link the kept item, `` See [`crate::parse`] ``, or rewrite it to call or assert something else.
3. Only when every copy must stay, open the kept copies with `rust,dejadoc`. rustdoc still runs them.
4. Rerun until exit `0`.

Trivial recurring bodies are better filtered than allowed. `.dejadoc.toml` at the workspace root takes `threshold` and `min-tokens`, compared against each group's `tokens`.

## Reviews

The GitHub action comments on each copy to remove with a removal suggestion and a link to the kept copy. Under `only-new` the kept copy is the one on the base commit. Accept the suggestion, or replace the added block with a link.

## Library

`dejadoc = { version = "0.2", default-features = false, features = ["std"] }`, then `dejadoc::Dejadoc::default().run(".")?` returns the same `Report`.
