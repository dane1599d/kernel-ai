#!/usr/bin/env bash
# S14-1's builds: native engine and runner, then the guest three ways:
# - target/wasm32-wasip2/release: scalar component (aot-scalar);
# - target/simd/wasm32-wasip2/release: simd128 component (aot, pulley);
# - target/wasm32-wasip1/release: scalar core module (wasmi, as apps run today).
set -euo pipefail
cd "$(dirname "$0")"
CARGO=${CARGO:-cargo}
STACK="-C link-arg=-zstack-size=8388608"
$CARGO build --release -p s14-engine -p s14-runner
$CARGO build --release -p s14-engine --target wasm32-wasip2
$CARGO build --release -p s14-engine --target wasm32-wasip1
RUSTFLAGS="$STACK -C target-feature=+simd128" \
  $CARGO build --release -p s14-engine --target wasm32-wasip2 --target-dir target/simd
ls -l target/wasm32-wasip2/release/s14-engine.wasm target/simd/wasm32-wasip2/release/s14-engine.wasm target/wasm32-wasip1/release/s14-engine.wasm
