# cargo-dejadoc

[![CI](https://github.com/LucaCappelletti94/cargo-dejadoc/actions/workflows/ci.yml/badge.svg)](https://github.com/LucaCappelletti94/cargo-dejadoc/actions/workflows/ci.yml)
[![codecov](https://codecov.io/github/LucaCappelletti94/cargo-dejadoc/graph/badge.svg)](https://codecov.io/github/LucaCappelletti94/cargo-dejadoc)
[![Quality Gate](https://sonarcloud.io/api/project_badges/measure?project=LucaCappelletti94_cargo-dejadoc&metric=alert_status)](https://sonarcloud.io/summary/new_project?id=lucacappelletti94-github_LucaCappelletti94_cargo-dejadoc)
[![crates.io](https://img.shields.io/crates/v/dejadoc.svg)](https://crates.io/crates/dejadoc)
[![docs.rs](https://docs.rs/dejadoc/badge.svg)](https://docs.rs/dejadoc)

`cargo dejadoc` finds duplicated Rust doctests and optionally checks functions and lexical contexts across a workspace.

![A pull request review by dejadoc, with an inline comment on a duplicated doctest and a suggestion that removes the copy](https://raw.githubusercontent.com/LucaCappelletti94/cargo-dejadoc/main/docs/review.png)

## Checks

| Check | Enable | Default minimum size | Set the size |
| --- | --- | --- | --- |
| Doctests | On by default | `0` tokens | `--min-tokens` |
| Functions | `--functions` | `30` weighted body tokens | `--fn-min-tokens` |
| Contexts | `--context-blocks` | `30` canonical weighted tokens | `--context-min-tokens` |

Groups need at least `2` sites by default, configurable with `--threshold`. Size floors are inclusive and independent. For functions and contexts, a path, lifetime or multi-character operator counts as one token, while commas and grouping delimiters count as zero.

Defaults come from `.dejadoc.toml` at the workspace root, with command-line options taking precedence. Set `functions = true` or `context-blocks = true` to enable the optional checks, and `context-min-tokens = 30` to set the context floor. See `cargo dejadoc --help` for package selection, additional targets and `JSON` output.

Generated files are skipped when their first five lines contain `@generated` or a `// Code generated … DO NOT EDIT.` marker. Configure `generated-markers` for other generators or `scan-generated = true` to include their output.

Doctest and function groups identify a copy to keep, preferring one that references its own item, then file and line order. Use `rust,dejadoc` on a doctest fence or `// dejadoc: allow` above a function to keep an intentional copy.

Functions compare within their module across targets and `cfg` variants, keeping test markers, `should_panic` and `ignore` distinct. Only eligible free functions and inherent methods receive deletion suggestions. Library public API, exported symbols and differing `cfg` or self types require refactoring.

### Contexts

Whole function bodies, explicit blocks, complete match arms and closures are normalized independently using [`syn-canon`](https://docs.rs/syn-canon). Arm patterns and guards and closure inputs contribute to the canonical size.

These are approximate matches with unverified enclosing semantics. `context_groups` carries original half-open ranges with one-based byte columns, including BOM and shebang files. Context matches have no keeper or deletion advice and never feed Action reviews.

A smaller group is hidden only when one larger matching group contains every site.

```bash
cargo dejadoc --context-blocks --context-min-tokens 30
```

## GitHub Action

The [action](https://github.com/marketplace/actions/dejadoc) installs the crate and reports findings in a pull request review, job summary and annotations.

```yaml
on: pull_request
jobs:
  dejadoc:
    runs-on: ubuntu-latest
    permissions:
      contents: read
      pull-requests: write
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
      - uses: LucaCappelletti94/cargo-dejadoc@v1
        with:
          pr-number: ${{ github.event.pull_request.number }}
          functions: true
```

Review mode reports newly introduced duplicates by default. Removal suggestions appear on eligible copies, with links to the kept copy. Omit `pr-number` to fail the job on duplicates, or set `only-new: false` to include pre-existing copies.

Fork pull requests receive summaries and annotations because their token is read-only. For review comments on forks, use [`mode: prepare` and `mode: post`](https://github.com/LucaCappelletti94/cargo-dejadoc/blob/main/action.yml) in separate workflows following the [`workflow_run` security pattern](https://securitylab.github.com/resources/github-actions-preventing-pwn-requests/). The privileged posting job must never check out or execute pull-request code.

See [`action.yml`](https://github.com/LucaCappelletti94/cargo-dejadoc/blob/main/action.yml) for all inputs.

## Library

Use `dejadoc = { version = "0.5", default-features = false, features = ["std"] }` for in-memory reports without the CLI. Scanning compiles no code and includes all `cfg` variants in one run.

```rust
let report = dejadoc::Dejadoc::default().run("tests/fixtures/dupws").unwrap();
assert_eq!(report.groups.len(), 2);
let report = dejadoc::Dejadoc::default().functions().run("tests/fixtures/fnws").unwrap();
assert_eq!(report.groups.len(), 7);
let report = dejadoc::Dejadoc::default()
    .context_blocks()
    .context_min_tokens(0)
    .run("tests/fixtures/contextws")
    .unwrap();
let group = report.context_groups.iter()
    .find(|g| g.sites.iter().any(|s| s.item == "contextws::count_up"))
    .unwrap();
assert_eq!(group.sites.len(), 2);
```

Coding agents get the same guidance from the [`dejadoc` skill](https://github.com/LucaCappelletti94/cargo-dejadoc/blob/main/skills/dejadoc/SKILL.md), installed with the [skills CLI](https://github.com/vercel-labs/skills).

```bash
npx skills add LucaCappelletti94/cargo-dejadoc
```
