# The OSS-Fuzz Rust builder, pinned by digest. build.sh installs a current
# nightly on top, since the image's own lags the crate's rust-version.
FROM gcr.io/oss-fuzz-base/base-builder-rust@sha256:433319ea1a6bbea6c1db1915a5d385021908cc62026d9f0efda2270ef6060de3

RUN apt-get update \
    && apt-get install -y --no-install-recommends zip \
    && rm -rf /var/lib/apt/lists/*

# The workflow hydrates the fixture corpus and writes fuzz/corpus/ before
# this build, so the seeds come in with the tree.
COPY . $SRC/mediaprobe
WORKDIR $SRC/mediaprobe
COPY .clusterfuzzlite/build.sh $SRC/
