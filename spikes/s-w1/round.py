"""Spike S-W1 (never merged): one round of weight-space programming on Qwen3-4B,
on a GitHub runner. It loads Qwen's bf16 weights at the pinned revision on the
CPU, applies the round's edit if it has one (`edit(model)` in the round file;
no gradients, no training), and answers each app request in the requests file
(`cargo xtask app-gate --request NAME` as JSON lines) greedily, printing each
answer's length in tokens and where its tokens go. The round file may also
define `probe(model, tokenizer, ids)` to print what it measures.

Usage: python3 round.py REQUESTS.jsonl [ROUND.py]
"""

import hashlib
import json
import runpy
import sys
import time

import torch
from huggingface_hub import snapshot_download
from transformers import AutoModelForCausalLM, AutoTokenizer

REPO = "Qwen/Qwen3-4B-Instruct-2507"
REVISION = "cdbee75f17c01a7cc42f958dc650907174af0554"


def load():
    path = snapshot_download(REPO, revision=REVISION)
    for part in ("model-00001-of-00003.safetensors", "model-00002-of-00003.safetensors",
                 "model-00003-of-00003.safetensors"):
        digest = hashlib.sha256()
        with open(f"{path}/{part}", "rb") as f:
            for chunk in iter(lambda: f.read(1 << 24), b""):
                digest.update(chunk)
        print(f"s-w1: round: {part} {digest.hexdigest()}")
    tokenizer = AutoTokenizer.from_pretrained(path)
    model = AutoModelForCausalLM.from_pretrained(path, torch_dtype=torch.bfloat16)
    model.eval()
    return model, tokenizer


def answer(model, tokenizer, request):
    ids = tokenizer.apply_chat_template(
        request["messages"], add_generation_prompt=True, return_tensors="pt",
        enable_thinking=False)
    started = time.time()
    with torch.no_grad():
        out = model.generate(ids, max_new_tokens=request["max_tokens"], do_sample=False)
    new = out[0, ids.shape[1]:]
    return ids, new, time.time() - started


def shape(tokenizer, new):
    """Where an answer's tokens go: each line of the script's code."""
    text = tokenizer.decode(new, skip_special_tokens=True)
    try:
        code = json.loads(text)["script"]
    except (ValueError, KeyError, TypeError):
        return text, []
    return text, [(len(tokenizer(line)["input_ids"]), line) for line in code.split("\n")]


def main():
    requests = [json.loads(line) for line in open(sys.argv[1], encoding="utf-8") if line.strip()]
    hooks = runpy.run_path(sys.argv[2]) if len(sys.argv) > 2 else {}
    model, tokenizer = load()
    if "edit" in hooks:
        touched = hooks["edit"](model)
        print(f"s-w1: round: edit applied: {touched}")
    for request in requests:
        ids, new, secs = answer(model, tokenizer, request)
        text, lines = shape(tokenizer, new)
        name = request.get("name", "?")
        print(f"s-w1: round: {name}: {len(new)} answer tokens in {secs:.0f} s, {len(lines)} code lines")
        for tokens, line in sorted(lines, reverse=True)[:8]:
            print(f"s-w1: round: {name}:   {tokens:3d} tokens  {line[:100]}")
        print(f"s-w1: round: {name}: text: {json.dumps(text)[:3000]}")
        if "probe" in hooks:
            hooks["probe"](model, tokenizer, ids)


if __name__ == "__main__":
    main()
