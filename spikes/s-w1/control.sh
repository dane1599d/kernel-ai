#!/usr/bin/env bash
# Spike S-W1's quantize control (never merged): requantize unsloth's own F16
# of Qwen3-4B-Instruct-2507 with its imatrix and a pinned llama.cpp build, and
# compare with the Q4_K_M the OS pins (xtask/models.json). A byte match means
# an edited F16 can be requantized exactly as the shipped file was, so an edit
# and quantization drift are not confounded. Runs on a GitHub runner only.
#
# Usage: control.sh "LLAMA_TAG ..." WORK_DIR
set -euo pipefail

tags=${1:?llama.cpp release tags, such as "b6100 b6150"}
work=${2:?work directory}
repo=unsloth/Qwen3-4B-Instruct-2507-GGUF
rev=a06e946bb6b655725eafa393f4a9745d460374c9
f16=Qwen3-4B-Instruct-2507-F16.gguf
f16_sha=336228a2810f17724ecd6e9e2e40e8381eee81e886316fee311ade2899c31030
imatrix=imatrix_unsloth.gguf_file
imatrix_sha=9908ff45cd363230b05588f22150f62f1091459aa67ab4bd1376d23ef2d4781f
pinned_sha=3605803b982cb64aead44f6c1b2ae36e3acdb41d8e46c8a94c6533bc4c67e597
pinned_size=2497281120

here=$(cd "$(dirname "$0")" && pwd)
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

# S_W1_COMPARE=0 skips the pinned file and its tensor compare (a gate job,
# where the disk holds the F16 and the output only).
compare=${S_W1_COMPARE:-1}
pinned=Qwen3-4B-Instruct-2507-Q4_K_M.gguf
if [ "$compare" = 1 ]; then fetch "$pinned" "$pinned_sha"; fi

# Each tag in turn: the first whose output is the pinned file's bytes wins.
for tag in $tags; do
  zip="llama-$tag-bin-ubuntu-x64.zip"
  if ! curl -fsSL --retry 3 -o "$zip" "https://github.com/ggml-org/llama.cpp/releases/download/$tag/$zip"; then
    echo "s-w1: control: llama.cpp $tag has no ubuntu-x64 build"
    continue
  fi
  rm -rf llama && mkdir llama && unzip -q "$zip" -d llama
  quantize=$(find llama -name llama-quantize -type f | head -1)
  chmod +x "$quantize"
  if ! LD_LIBRARY_PATH="$(dirname "$quantize")" "$quantize" --imatrix "$imatrix" "$f16" out.gguf Q4_K_M "$(nproc)" > quantize.log 2>&1; then
    echo "s-w1: control: llama.cpp $tag failed: $(tail -1 quantize.log)"
    continue
  fi
  got=$(sha256sum out.gguf | cut -c1-64)
  echo "s-w1: control: llama.cpp $tag gives $got, $(stat -c %s out.gguf) bytes; pinned $pinned_sha, $pinned_size bytes"
  if [ "$got" = "$pinned_sha" ]; then
    echo "s-w1: control: byte match with llama.cpp $tag"
    exit 0
  fi
  if [ "$compare" = 1 ]; then
    python3 -I "$here/compare.py" "$pinned" out.gguf | grep -E "tensors byte-identical|key "
  else
    exit 0
  fi
done
echo "s-w1: control: no tag gives the pinned bytes"
