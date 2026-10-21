FROM gcr.io/oss-fuzz-base/base-builder-rust
COPY . $SRC/cargo-echoes
WORKDIR $SRC/cargo-echoes
COPY ./.clusterfuzzlite/build.sh $SRC/
