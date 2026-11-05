"""Round 1: the probe. No edit: guess's first answer on the unedited bf16
weights, and how each of its layers' MLP output norms grow over the answer,
to see where the length comes from before any weight is touched."""

import torch


def probe(model, tokenizer, ids):
    # One forward pass over the prompt: each layer's down_proj output norm at the
    # last prompt token, the place the answer's plan is set.
    norms = {}
    hooks = []
    for i, layer in enumerate(model.model.layers):
        def keep(_module, _inputs, output, i=i):
            norms[i] = output[0, -1].float().norm().item()
        hooks.append(layer.mlp.down_proj.register_forward_hook(keep))
    with torch.no_grad():
        model(ids)
    for h in hooks:
        h.remove()
    line = " ".join(f"{i}:{norms[i]:.1f}" for i in sorted(norms))
    print(f"s-w1: round: probe: down_proj output norm at the last prompt token by layer: {line}")
