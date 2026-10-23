#!/bin/sh
# Build probe (spike S0, Linux only): clean `cargo build --release` of pdf-bakeoff per engine
# feature, each in a fresh target dir and a fresh empty HOME, so build-script downloads
# (tesseract-rs: Tesseract + Leptonica sources; ort: prebuilt ONNX Runtime) happen inside the
# timed build as on a first checkout. Crates are already in the local registry cache, so their
# download time is not included. Records wall time, binary size, the bytes build scripts
# downloaded into HOME, and the binary's dynamic libraries in spikes/out/build-probe.json.
# Model files are fetched by `fetch-ocr` at run time and are counted by the report.
set -eu
cd "$(dirname "$0")/.."
mkdir -p out
cargo_home=${CARGO_HOME:-$HOME/.cargo}
rustup_home=${RUSTUP_HOME:-$HOME/.rustup}

json="{"
sep=""
for name in baseline hayro oxide pdfium ocr ocrs tesseract onnx; do
    dir="$PWD/target/probe-$name"
    rm -rf "$dir"
    mkdir -p "$dir/home"
    feat=""
    [ "$name" = baseline ] || feat="--features $name"
    start=$(date +%s.%N)
    # shellcheck disable=SC2086
    env HOME="$dir/home" CARGO_HOME="$cargo_home" RUSTUP_HOME="$rustup_home" \
        CARGO_TARGET_DIR="$dir" cargo build --release --locked -p pdf-bakeoff $feat \
        >"out/build-probe-$name.log" 2>&1
    end=$(date +%s.%N)
    secs=$(awk "BEGIN { printf \"%.1f\", $end - $start }")
    bytes=$(stat -c %s "$dir/release/pdf-bakeoff")
    lib=0
    [ "$name" = pdfium ] && lib=$(stat -c %s vendor/pdfium/lib/libpdfium.so)
    fetched=$(du -sb "$dir/home" | cut -f1)
    libs=$(ldd "$dir/release/pdf-bakeoff" | awk '{ printf "%s%s", s, $1; s = " " }')
    echo "$name: ${secs} s, ${bytes} bytes, lib ${lib}, fetched ${fetched}, libs: $libs" >&2
    json="$json$sep\"$name\": {\"secs\": $secs, \"bytes\": $bytes, \"lib_bytes\": $lib, \"fetched_bytes\": $fetched, \"dynamic\": \"$libs\"}"
    sep=", "
    rm -rf "$dir"
done
json="$json, \"jobs\": $(nproc), \"cmake\": \"$(cmake --version | head -1)\", \"cxx\": \"$(c++ --version | head -1)\"}"
echo "$json" > out/build-probe.json
echo "$json"
