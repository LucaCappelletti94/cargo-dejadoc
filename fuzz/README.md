# Fuzz targets

ClusterFuzzLite runs these on every pull request, daily in batch, and on every push to `main`. Each target runs dejadoc on the 1 GiB stack `group` uses.

- `canonicalize` hashes a doctest body twice through `group` and expects one group.
- `doc` writes NUL-separated parts as `///`, raw, escaped and `/** */` doc attributes and extracts them.
- `source` parses a whole file with `syn` and extracts it.

```sh
cargo +nightly fuzz run canonicalize fuzz/corpus/canonicalize fuzz/seeds/canonicalize -- -dict=fuzz/dejadoc.dict
```

## Seed Corpus

`fuzz/seeds/<target>/` holds one input per file, grown from dejadoc's own test strings and fixtures and reduced with `-set_cover_merge=1`. `.clusterfuzzlite/build.sh` zips it into the starting corpus and fails a target without seeds.
