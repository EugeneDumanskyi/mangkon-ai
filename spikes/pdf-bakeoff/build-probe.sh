#!/bin/sh
# Build probe (spike S0, Linux only in M0b): clean `cargo build --release` of pdf-bakeoff per
# engine feature, each in a fresh target dir. Records wall time and binary size against a
# no-engine baseline in spikes/out/build-probe.json. Crates are already in the local registry
# cache, so download time is not included. M0c repeats this on macOS and Windows.
set -eu
cd "$(dirname "$0")/.."
mkdir -p out

json="{"
sep=""
for name in baseline hayro oxide pdfium; do
    dir="target/probe-$name"
    rm -rf "$dir"
    feat=""
    [ "$name" = baseline ] || feat="--features $name"
    start=$(date +%s.%N)
    # shellcheck disable=SC2086
    CARGO_TARGET_DIR="$dir" cargo build --release --locked -p pdf-bakeoff $feat >/dev/null 2>&1
    end=$(date +%s.%N)
    secs=$(awk "BEGIN { printf \"%.1f\", $end - $start }")
    bytes=$(stat -c %s "$dir/release/pdf-bakeoff")
    lib=0
    [ "$name" = pdfium ] && lib=$(stat -c %s vendor/pdfium/lib/libpdfium.so)
    echo "$name: ${secs} s, ${bytes} bytes, lib ${lib} bytes" >&2
    json="$json$sep\"$name\": {\"secs\": $secs, \"bytes\": $bytes, \"lib_bytes\": $lib}"
    sep=", "
    rm -rf "$dir"
done
json="$json, \"jobs\": $(nproc)}"
echo "$json" > out/build-probe.json
echo "$json"
