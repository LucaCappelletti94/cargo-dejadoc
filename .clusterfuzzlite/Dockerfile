FROM gcr.io/oss-fuzz-base/base-builder-rust
COPY . $SRC/cargo-dejadoc
WORKDIR $SRC/cargo-dejadoc
COPY ./.clusterfuzzlite/build.sh $SRC/
