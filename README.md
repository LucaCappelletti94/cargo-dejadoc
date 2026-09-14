# cargo-dejadoc

[![CI](https://github.com/LucaCappelletti94/cargo-dejadoc/actions/workflows/ci.yml/badge.svg)](https://github.com/LucaCappelletti94/cargo-dejadoc/actions/workflows/ci.yml)
[![codecov](https://codecov.io/github/LucaCappelletti94/cargo-dejadoc/graph/badge.svg)](https://codecov.io/github/LucaCappelletti94/cargo-dejadoc)
[![Quality Gate](https://sonarcloud.io/api/project_badges/measure?project=LucaCappelletti94_cargo-dejadoc&metric=alert_status)](https://sonarcloud.io/summary/new_project?id=lucacappelletti94-github_LucaCappelletti94_cargo-dejadoc)
[![crates.io](https://img.shields.io/crates/v/dejadoc.svg)](https://crates.io/crates/dejadoc)
[![docs.rs](https://docs.rs/dejadoc/badge.svg)](https://docs.rs/dejadoc)

Whoa, deja vu. A doctest went past us, and then another that looked just like it.

`cargo dejadoc` scans and canonicalizes every doctest your workspace's rustdoc would run, reporting any identified duplicates.

Run `cargo dejadoc` at the workspace root. It exits 0 when clean or with `--no-fail`, 1 when it finds duplicates, and 2 on scan or usage errors.
The flags under `cargo dejadoc --help` restrict the package, set the threshold and minimum token count, point at a config file, and switch the output to JSON.
Without flags, parameters come from `.dejadoc.toml` at the workspace root.

To allow a known copy, add the `dejadoc` token to the fence's info list, as in `rust,dejadoc`. rustdoc runs the doctest and ignores the unknown token, so nothing else about the site changes.

In CI, the composite action installs the crate and runs the gate.

```yaml
- uses: LucaCappelletti94/cargo-dejadoc@v1
  with:
    threshold: 2
```

The library builds the same report in memory.

```rust
let report = dejadoc::Dejadoc::default().run("tests/fixtures/dupws").unwrap();
assert_eq!(report.groups.len(), 2);
```
