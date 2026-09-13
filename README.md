# cargo-dejadoc

![CI](https://github.com/LucaCappelletti94/cargo-dejadoc/actions/workflows/ci.yml/badge.svg)

Whoa, deja vu. Find duplicated Rust doctests across a workspace.

`cargo dejadoc` scans every doctest your workspace's rustdoc would run,
canonicalizes each body through `syn`, and reports the groups that share a
canonical form. Comment and whitespace drift collapse; `#`-hidden lines are
dropped, as rustdoc does. Doctest attributes (`no_run`, editions, …) are
listed per site and never affect the grouping.

## Usage

```text
cargo dejadoc [OPTIONS]
  -p, --package <PACKAGE>  Restrict to one workspace member by name
      --all-targets        Scan bin and example targets in addition to lib targets
      --json               Machine-readable report
  -t, --threshold <N>      Report groups with ≥ N sites (default 2)
      --min-tokens <N>     Skip blocks with < N tokens (default 0)
      --config <PATH>      .dejadoc.toml location (default: workspace root)
      --no-fail            Exit 0 even when duplicates are found
  -v, --verbose            Print each group's code

exit: 0 clean or --no-fail, 1 duplicates found, 2 scan or usage error
```

## Allowing a site

Add the `dejadoc` token to the fence's info list. rustdoc runs the doctest
and ignores the unknown token, so nothing else about the site changes:

```text
/// ```rust,dejadoc
/// fn deliberate_copy() { }
/// ```
```

## Configuration

`.dejadoc.toml` at the workspace root, parameters only (CLI flags win):

```text
threshold = 2
min-tokens = 0
```

## Library

```rust
let report = dejadoc::run(
    std::path::Path::new("tests/fixtures/dupws"),
    &dejadoc::Options::default(),
)
.unwrap();
assert_eq!(report.groups.len(), 3);
```
