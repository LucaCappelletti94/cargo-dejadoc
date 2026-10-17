# cargo-dejadoc

[![CI](https://github.com/LucaCappelletti94/cargo-dejadoc/actions/workflows/ci.yml/badge.svg)](https://github.com/LucaCappelletti94/cargo-dejadoc/actions/workflows/ci.yml)
[![codecov](https://codecov.io/github/LucaCappelletti94/cargo-dejadoc/graph/badge.svg)](https://codecov.io/github/LucaCappelletti94/cargo-dejadoc)
[![Quality Gate](https://sonarcloud.io/api/project_badges/measure?project=LucaCappelletti94_cargo-dejadoc&metric=alert_status)](https://sonarcloud.io/summary/new_project?id=lucacappelletti94-github_LucaCappelletti94_cargo-dejadoc)
[![crates.io](https://img.shields.io/crates/v/dejadoc.svg)](https://crates.io/crates/dejadoc)
[![docs.rs](https://docs.rs/dejadoc/badge.svg)](https://docs.rs/dejadoc)

Whoa, deja vu. A doctest went past us, and then another that looked just like it.

`cargo dejadoc` finds duplicated Rust doctests, and optionally duplicated functions and code blocks, across a workspace. The [wiki](https://github.com/LucaCappelletti94/cargo-dejadoc/wiki) explains each check, the options and the Action in depth.

![A pull request review by dejadoc, with an inline comment on a duplicated doctest and a suggestion that removes the copy](https://raw.githubusercontent.com/LucaCappelletti94/cargo-dejadoc/main/docs/review.png)

## Usage

```bash
cargo install dejadoc
cargo dejadoc --functions --context-blocks
```

| Check | Enable | Default minimum size | Set the size |
| --- | --- | --- | --- |
| Doctests | On by default | `0` tokens | `--min-tokens` |
| Functions | `--functions` | `30` weighted body tokens | `--fn-min-tokens` |
| Contexts | `--context-blocks` | `30` canonical weighted tokens | `--context-min-tokens` |

Options can also live in `.dejadoc.toml` at the workspace root, and `cargo dejadoc --help` lists them all. Keep an intentional copy with a `rust,dejadoc` fence or a `// dejadoc: allow` line above the function. The wiki lists [what counts as the same code](https://github.com/LucaCappelletti94/cargo-dejadoc/wiki/What-counts-as-the-same-code).

## GitHub Action

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

The [action](https://github.com/marketplace/actions/dejadoc) reviews the pull request and suggests removing each copy it introduces. [`action.yml`](https://github.com/LucaCappelletti94/cargo-dejadoc/blob/main/action.yml) lists every input, and [these two workflows](https://github.com/LucaCappelletti94/cargo-dejadoc/tree/main/examples/workflows) review pull requests from forks.

## Library

```rust
let report = dejadoc::Dejadoc::default()
    .functions()
    .context_blocks()
    .run(".")
    .unwrap();
println!("{}", dejadoc::human(&report, false));
```

Use `default-features = false, features = ["std"]` to drop the CLI. Coding agents get the same guidance from the [`dejadoc` skill](https://github.com/LucaCappelletti94/cargo-dejadoc/blob/main/skills/dejadoc/SKILL.md).

```bash
npx skills add LucaCappelletti94/cargo-dejadoc
```
