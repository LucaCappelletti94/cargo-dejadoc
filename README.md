# cargo-dejadoc

![CI](https://github.com/LucaCappelletti94/cargo-dejadoc/actions/workflows/ci.yml/badge.svg)
![codecov](https://codecov.io/github/LucaCappelletti94/cargo-dejadoc/graph/badge.svg)
![Quality Gate](https://sonarcloud.io/api/project_badges/measure?project=LucaCappelletti94_cargo-dejadoc&metric=alert_status)

Deja vu. Find duplicated Rust doctests across a workspace.

`cargo dejadoc` scans every doctest your workspace's rustdoc would run,
canonicalizes each body through `syn`, and reports the groups that share a
canonical form. Comment and whitespace drift collapse. `#`-hidden lines are
dropped, as rustdoc does. Doctest attributes such as `no_run` are listed
per site and never affect the grouping.

## Usage

```text
Usage: cargo-dejadoc [OPTIONS]

Options:
  -p, --package <PACKAGE>  Restrict to one workspace member by name
      --all-targets        Scan bin and example targets in addition to lib targets
      --json               Machine-readable output
  -t, --threshold <N>      Report groups with at least this many sites (default 2)
      --min-tokens <N>     Skip blocks with fewer tokens than this (default 0)
      --config <PATH>      Explicit `.dejadoc.toml` location
      --no-fail            Exit 0 even when duplicates are found
  -v, --verbose            Print each group's code
  -h, --help               Print help
  -V, --version            Print version
```

Exit codes. 0 clean or `--no-fail`, 1 duplicates found, 2 scan or usage error.

## Allowing a site

Add the `dejadoc` token to the fence's info list. rustdoc runs the doctest
and ignores the unknown token, so nothing else about the site changes.

```text
/// ```rust,dejadoc
/// fn deliberate_copy() { }
/// ```
```

## Configuration

Parameters live in `.dejadoc.toml` at the workspace root. CLI flags win.

```text
threshold = 2
min-tokens = 0
```

## Library

```rust
let report = dejadoc::Dejadoc::default().run("tests/fixtures/dupws").unwrap();
assert_eq!(report.groups.len(), 3);
```

`run_targets` scans caller-resolved targets in memory and is the `no_std`
entry point.

The builder `Dejadoc` is `#![no_std]` and `alloc`-only. The `std` feature
(defaulted) additionally enables `run`, `.config`, `Error`, `exit_code`,
and the `cargo-dejadoc` binary.
