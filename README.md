# cargo-dejadoc

[![CI](https://github.com/LucaCappelletti94/cargo-dejadoc/actions/workflows/ci.yml/badge.svg)](https://github.com/LucaCappelletti94/cargo-dejadoc/actions/workflows/ci.yml)
[![codecov](https://codecov.io/github/LucaCappelletti94/cargo-dejadoc/graph/badge.svg)](https://codecov.io/github/LucaCappelletti94/cargo-dejadoc)
[![Quality Gate](https://sonarcloud.io/api/project_badges/measure?project=LucaCappelletti94_cargo-dejadoc&metric=alert_status)](https://sonarcloud.io/summary/new_project?id=lucacappelletti94-github_LucaCappelletti94_cargo-dejadoc)
[![crates.io](https://img.shields.io/crates/v/dejadoc.svg)](https://crates.io/crates/dejadoc)
[![docs.rs](https://docs.rs/dejadoc/badge.svg)](https://docs.rs/dejadoc)

Whoa, deja vu. A doctest went past us, and then another that looked just like it.

`cargo dejadoc` scans and canonicalizes every doctest your workspace's rustdoc would run, reporting any identified duplicates.

![A pull request review by dejadoc, with an inline comment on a duplicated doctest and a suggestion that removes the copy](https://raw.githubusercontent.com/LucaCappelletti94/cargo-dejadoc/main/docs/review.png)

The flags under `cargo dejadoc --help` restrict the package, set the threshold and minimum token count, point at a config file, and switch the output to JSON.
Without flags, parameters come from `.dejadoc.toml` at the workspace root.

To allow a known copy, add the `dejadoc` token to the fence's info list, as in `rust,dejadoc`. rustdoc runs the doctest and ignores the unknown token, so nothing else about the site changes.

In CI, the composite action installs the crate and runs the gate.

```yaml
- uses: LucaCappelletti94/cargo-dejadoc@v1
  with:
    threshold: 2
```

In a pull request, the action posts the findings as a review instead of failing the job.

```yaml
on: pull_request
jobs:
  dejadoc:
    runs-on: ubuntu-latest
    permissions:
      pull-requests: write
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
      - uses: LucaCappelletti94/cargo-dejadoc@v1
        with:
          pr-number: ${{ github.event.pull_request.number }}
```

Every duplicated doctest whose file is in the pull request diff gets an inline comment, the first copy marked as the one to keep and each later copy with a suggestion that deletes it, and the review requests changes. Sites outside the diff get no comment, and when no site is in the diff the review is skipped.

A pull request from a fork runs with a read-only `GITHUB_TOKEN`, so the action logs a warning, skips the review, and the job passes. To review fork pull requests, trigger on `pull_request_target` and check out the pull request head as below. That trigger runs the job with write access, so read [GitHub's guidance](https://docs.github.com/en/actions/reference/security/securely-using-pull_request_target) before opting the checkout in with `allow-unsafe-pr-checkout`.

```yaml
on: pull_request_target
jobs:
  dejadoc:
    runs-on: ubuntu-latest
    permissions:
      pull-requests: write
    steps:
      - uses: actions/checkout@v7
        with:
          ref: ${{ github.event.pull_request.head.sha }}
          allow-unsafe-pr-checkout: true
      - uses: dtolnay/rust-toolchain@stable
      - uses: LucaCappelletti94/cargo-dejadoc@v1
        with:
          pr-number: ${{ github.event.pull_request.number }}
```

On a private repository, the setting **Send write tokens to workflows from pull requests** under Settings > Actions > General does the same without changing the trigger.

The library builds the same report in memory.

```rust
let report = dejadoc::Dejadoc::default().run("tests/fixtures/dupws").unwrap();
assert_eq!(report.groups.len(), 2);
```
