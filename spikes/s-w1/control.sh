#!/usr/bin/env bash
# Spike S-W1's quantize control (never merged): requantize unsloth's own F16
# of Qwen3-4B-Instruct-2507 with its imatrix and a pinned llama.cpp build, and
# compare with the Q4_K_M the OS pins (xtask/models.json). A byte match means
# an edited F16 can be requantized exactly as the shipped file was, so an edit
# and quantization drift are not confounded. Runs on a GitHub runner only.
#
# Usage: control.sh LLAMA_TAG WORK_DIR
set -euo pipefail

tag=${1:?llama.cpp release tag, such as b6100}
work=${2:?work directory}
repo=unsloth/Qwen3-4B-Instruct-2507-GGUF
rev=a06e946bb6b655725eafa393f4a9745d460374c9
f16=Qwen3-4B-Instruct-2507-F16.gguf
f16_sha=336228a2810f17724ecd6e9e2e40e8381eee81e886316fee311ade2899c31030
imatrix=imatrix_unsloth.gguf_file
imatrix_sha=9908ff45cd363230b05588f22150f62f1091459aa67ab4bd1376d23ef2d4781f
pinned_sha=3605803b982cb64aead44f6c1b2ae36e3acdb41d8e46c8a94c6533bc4c67e597
pinned_size=2497281120

mkdir -p "$work"
cd "$work"

fetch() { # file, sha256
  if [ ! -f "$1" ] || ! echo "$2  $1" | sha256sum -c --status; then
    curl -fsSL --retry 3 -o "$1" "https://huggingface.co/$repo/resolve/$rev/$1"
  fi
  echo "$2  $1" | sha256sum -c
}
fetch "$f16" "$f16_sha"
fetch "$imatrix" "$imatrix_sha"

zip="llama-$tag-bin-ubuntu-x64.zip"
curl -fsSL --retry 3 -o "$zip" "https://github.com/ggml-org/llama.cpp/releases/download/$tag/$zip"
echo "s-w1: llama.cpp $tag, $(sha256sum "$zip" | cut -c1-64)"
rm -rf llama && mkdir llama && unzip -q "$zip" -d llama
quantize=$(find llama -name llama-quantize -type f | head -1)
chmod +x "$quantize"
export LD_LIBRARY_PATH="$(dirname "$quantize"):${LD_LIBRARY_PATH:-}"

"$quantize" --imatrix "$imatrix" "$f16" out.gguf Q4_K_M "$(nproc)" > quantize.log 2>&1 || {
  tail -30 quantize.log
  exit 1
}
grep -E "model size|quant size|imatrix|llama_model_quantize_impl: model" quantize.log | head -8 || true

got=$(sha256sum out.gguf | cut -c1-64)
size=$(stat -c %s out.gguf)
echo "s-w1: control: llama.cpp $tag gives $got, $size bytes; pinned $pinned_sha, $pinned_size bytes"
if [ "$got" = "$pinned_sha" ]; then
  echo "s-w1: control: byte match"
else
  echo "s-w1: control: no byte match"
  echo "$got" > out.sha256
fi
