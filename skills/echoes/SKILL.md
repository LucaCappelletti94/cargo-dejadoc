---
name: echoes
description: Run cargo echoes and resolve duplicated Rust doctests, functions and code blocks, in one workspace or across a GitHub organization. Use when a echoes check or review flags a duplicate, or the user asks to deduplicate Rust code.
license: MIT
---

# echoes

`cargo echoes` hashes the canonical form of every doctest rustdoc would run, and on request every function and code block, then lists the sites sharing one. Exit `1` on any group, `2` on an error or incomplete coverage, even with `--no-fail`.

## What counts as a copy

Cosmetic differences never separate two bodies, so editing them is not a fix.

- whitespace, comments, trailing commas, hidden `# ` lines
- local names, `fn shared() {}` and `fn helper() {}` are one doctest
- `use` order, literal spelling outside macros
- fence attributes such as `no_run`

Different paths, methods, string literals or macro arguments do separate bodies. `ignore` and non-Rust fences are never counted.

## Run

```
cargo echoes -v                                # each group with its sites and code
cargo echoes --functions --context-blocks      # also functions and code blocks
cargo echoes --organization <org> --functions  # every repository of a GitHub organization or user
cargo echoes --json                            # {groups: [{kind, id, tokens, remedy, sites}], context_groups, organization}
```

`-p <member>`, `--all-targets`, `-t <sites>`, `--min-tokens <n>`, `--fn-min-tokens <n>`, `--context-min-tokens <n>`, `--no-fail`, `--github` for workflow annotations.

## Resolve a doctest group

1. Keep the site whose item the body calls. A crate level example beats a copy on an item.
2. At every other site, delete the block and link the kept item, `` See [`crate::parse`] ``, or rewrite it to call or assert something else.
3. Only when every copy must stay, open the kept copies with `rust,echoes`. rustdoc still runs them.
4. Rerun until exit `0`.

Trivial recurring bodies are better filtered than allowed. `.echoes.toml` at the workspace root takes `threshold`, `min-tokens`, `fn-min-tokens` and `context-min-tokens`, compared against each group's `tokens`.

## Resolve a function group

The first site is the copy to keep, and the group's `remedy` says how to remove the others.

| Remedy | Fix |
| --- | --- |
| `delete` | Delete the other copies, or point their callers at the kept one |
| `merge-cfg` | Write one function covering both `cfg` cases |
| `helper-or-macro` | Keep each signature and move the body into one helper or macro |
| `generic-or-macro` | Share one body across the self types through a generic or a macro |

Keep a deliberate copy with `// echoes: allow` on the line right above the function or its first attribute.

## Resolve a context group

Context groups match function bodies, blocks, `match` arms and closures with identical canonical forms. They name no copy to keep, because the code around a fragment can change its meaning. Move the shared code into one helper every site calls. Never reorder or respell one copy to hide it from the check.

## Organizations

`--organization` reads every repository the `GH_TOKEN` or `GITHUB_TOKEN` credential can see, pinned to its default branch, and compares functions across crates and modules. Without a name it scans the current repository's owner. Cross-crate copies get `helper-or-macro` or `generic-or-macro`, so share them through a crate both depend on. Downloaded `.echoes.toml` files are ignored, so pass `--config`. Exit `2` with `echoes: incomplete organization analysis` lines means a repository was not analyzed.

## Reviews

The GitHub action comments on each copy to remove with a removal suggestion and a link to the kept copy. Under `only-new` the kept copy is the one on the base commit. Accept the suggestion, or replace the added block with a link. Context groups can be posted as neutral comments without a suggestion.

## Library

`echoes = { version = "0.7", default-features = false, features = ["std"] }`, then `echoes::Echoes::default().functions().run(".")?` returns the same `Report`, and `run_organization("org")` scans an organization.
