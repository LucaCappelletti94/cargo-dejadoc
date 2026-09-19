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

Coding agents get the same guidance from the `dejadoc` skill in `skills/`, installed with `npx skills add LucaCappelletti94/cargo-dejadoc`.

In CI, one workflow covers it. The action installs the crate, scans, and posts the findings as a pull request review, with every other input optional.

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
          # Review the pull request instead of failing the job. Drop it to gate a push.
          pr-number: ${{ github.event.pull_request.number }}
          # Report duplicates that predate the pull request too, default false.
          only-new: false
          # Report a group from this many copies, default 2.
          threshold: 2
          # Ignore blocks below this token count, default 0.
          min-tokens: 0
          # Scan bin and example targets as well as lib, default false.
          all-targets: true
          # Restrict to one workspace member, and scan from a subdirectory.
          package: mycrate
          cwd: .
          # Annotate each copy to remove, default true.
          annotations: true
          # Render the review in the job summary instead of posting it.
          dry-run: false
```

Review mode reports only the duplicates your pull request introduces, comparing each site against a scan of the base commit. Comments land on the copies to remove, a kept copy never gets one. Each comment links the copy that survives and GitHub renders those lines right in the comment, long ones collapsed behind a show link. The link points at the base commit for pre-existing copies and at the first added copy for groups the pull request created, and the removal suggestion deletes the copy together with an empty line left behind. Sites GitHub cannot anchor get permalinks in the review body.

Every run also writes the report to the job summary and one annotation per copy to remove, which GitHub anchors to the line in the diff. Annotations are workflow output rather than API calls, so they need no token and a fork pull request shows the findings whatever its token allows. The `--github` flag prints them from the command line too, beside the usual report.

A fork's `GITHUB_TOKEN` is read-only, so the review cannot be posted there and the job passes on the annotations and the summary alone. Only a review comment can carry the removal suggestion, so keeping that on forks takes two workflows, the unprivileged one preparing the review and a privileged one posting it without ever checking out the pull request head.

<details>
<summary>Two workflows, so forks keep the removal suggestion</summary>

```yaml
# .github/workflows/dejadoc.yml, no permissions, runs on the pull request.
on: pull_request
jobs:
  dejadoc:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
      - uses: LucaCappelletti94/cargo-dejadoc@v1
        with:
          pr-number: ${{ github.event.pull_request.number }}
          mode: prepare
          payload: dejadoc-payload
      - uses: actions/upload-artifact@v4
        with:
          name: dejadoc-payload
          path: dejadoc-payload
```

```yaml
# .github/workflows/dejadoc-post.yml, writes, never checks out the head.
on:
  workflow_run:
    workflows: [dejadoc]
    types: [completed]
jobs:
  post:
    runs-on: ubuntu-latest
    permissions:
      pull-requests: write
    steps:
      - uses: actions/download-artifact@v4
        with:
          name: dejadoc-payload
          path: dejadoc-payload
          run-id: ${{ github.event.workflow_run.id }}
          github-token: ${{ github.token }}
      - uses: LucaCappelletti94/cargo-dejadoc@v1
        with:
          mode: post
          payload: dejadoc-payload
```

The payload holds the pull request number, the head commit, and one rendered comment per copy with the lines it covers. The posting job reads only that, so it needs no checkout, no toolchain and no dejadoc install, and untrusted code never runs beside the write token. `pull_request_target` grants the same access in one job, and the action still accepts it, but its recipe checks out the pull request head in a privileged job, so it is not the path this README recommends.

</details>

The library builds the same report in memory, so a project's own task runner can gate on it with `dejadoc = { version = "0.2", default-features = false, features = ["std"] }`, which leaves the CLI and its `clap` dependency out. The scan compiles nothing, so a crate whose features are mutually exclusive needs one run rather than one per feature set.

```rust
let report = dejadoc::Dejadoc::default().run("tests/fixtures/dupws").unwrap();
assert_eq!(report.groups.len(), 2);
```
