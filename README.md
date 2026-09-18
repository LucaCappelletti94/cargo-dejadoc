# cargo-dejadoc

[![CI](https://github.com/LucaCappelletti94/cargo-dejadoc/actions/workflows/ci.yml/badge.svg)](https://github.com/LucaCappelletti94/cargo-dejadoc/actions/workflows/ci.yml)
[![codecov](https://codecov.io/github/LucaCappelletti94/cargo-dejadoc/graph/badge.svg)](https://codecov.io/github/LucaCappelletti94/cargo-dejadoc)
[![Quality Gate](https://sonarcloud.io/api/project_badges/measure?project=LucaCappelletti94_cargo-dejadoc&metric=alert_status)](https://sonarcloud.io/summary/new_project?id=lucacappelletti94-github_LucaCappelletti94_cargo-dejadoc)
[![crates.io](https://img.shields.io/crates/v/dejadoc.svg)](https://crates.io/crates/dejadoc)
[![docs.rs](https://docs.rs/dejadoc/badge.svg)](https://docs.rs/dejadoc)

Whoa, deja vu. A doctest went past us, and then another that looked just like it.

`cargo dejadoc` scans and canonicalizes every doctest your workspace's rustdoc would run, reporting any duplicates. Use it in your GitHub CI!

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

Review mode reports only the duplicates your pull request introduces, comparing each site against a scan of the base commit. Comments land on the copies to remove, a kept copy never gets one. Each comment links the copy that survives and GitHub renders those lines right in the comment, long ones collapsed behind a show link. The link points at the base commit for pre-existing copies and at the first added copy for groups the pull request created, and the removal suggestion deletes the copy together with an empty line left behind. Sites GitHub cannot anchor get permalinks in the review body. Set `only-new: false` to also report duplicates that predate the pull request.

Every run also writes the report to the job summary and one annotation per copy to remove, which GitHub anchors to the line in the diff. Annotations are workflow output rather than API calls, so they need no token and a fork pull request shows the findings whatever the token allows. The `--github` flag prints them from the command line too, next to the usual report.

Forks run with a read-only `GITHUB_TOKEN`, so the review itself cannot be posted, the annotations and the summary carry the findings and the job passes. The removal suggestion is the part a fork loses, since only a review comment can offer one. To post reviews on forks as well, keep the `pull_request` trigger and let a second workflow post from the base branch, as [GitHub's guidance](https://docs.github.com/en/actions/reference/security/securely-using-pull_request_target) describes, or on a private repository enable **Send write tokens to workflows from pull requests** under Settings > Actions > General.

`pull_request_target` also grants write access, and the action still accepts it, but the recipe means checking out the pull request head in a privileged job, so it is not the path this README recommends.

The library builds the same report in memory.

```rust
let report = dejadoc::Dejadoc::default().run("tests/fixtures/dupws").unwrap();
assert_eq!(report.groups.len(), 2);
```
