needed-by: P4, measured 2026-11-04 (model lane, ADR 0013 spike S13-2 `batch-decode`; #670 closed unmerged)
decision: Batched decode fails S13-2's bar (1.6x total at 2 sequences); decode stays serial
---
The spike lived on draft #670 (branch `dane/s13-2-batch-decode`, kept unmerged). `infer::Model::decode_batch` decodes up to 32 sequences in one step, each with its own KV cache, and its bits are each sequence's alone in every tier for 1, 2 and 4 sequences. `cargo xtask bench batch` times it on GitHub runners against a generated model of Qwen3-4B's shapes in its Q4_K_M mix.

Total tokens/s against one sequence, 4 threads, at 2 sequences (after 32 positions / after 1,024):
- run 41866038969 (avx512+vnni, the 8-token group kernels alone): 1.00x / 0.96x; 1.60x and 1.79x at 4 and 8 sequences;
- run 41859773382 (avx2, same kernels): 0.83 to 0.99x;
- run 41876741042 (avx512+vnni, with a 2-token Q4_K kernel, `q4_k_dot4_n`): 1.33x / 1.46x;
- run 41890832822 (avx2, with #699's 4-row Q6_K kernel and the small-batch path on the caller, merged): 1.28x / 1.24x; 1 thread 1.39x.

Both 1.00x and 1.28x are right: the first is the plain kernels, the second the best path measured. The best is still under 1.6x, so **S13-2 fails** and ADR 0013's default stands: the foreground and agents decode one sequence at a time. Deferred, not scheduled: attention split by (sequence, head) across a batch, guest figures under KVM, and the foreground's tokens/s when it joins a batch.
