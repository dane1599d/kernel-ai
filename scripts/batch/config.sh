# Sourced by every script in scripts/batch. Each setting is an environment
# variable; the defaults are the values the batcher ran with on 2026-11-18, so a
# plain run behaves as before.
#
#   KAI_REPO                   GitHub repository (owner/name)     dane1599d/kernel-ai
#   KAI_GIT_DIR                checkout to fetch and combine in   the checkout holding these scripts
#                              (only `git fetch`, `merge-tree`, `commit-tree` and `push`:
#                              its work tree and branches are never touched)
#   KAI_MAIN                   the branch batches land on         main
#   KAI_BATCH_STATE            log, stop file, batch-max, resume  <git common dir>/kai-batch
#                              state, CI outputs
#   KAI_BATCH_MAX              PRs per batch when                 15
#                              $KAI_BATCH_STATE/batch-max is absent (the file is re-read
#                              before every batch, so the cap changes without a restart)
#   KAI_BATCH_DELETE_BRANCHES  1: delete each landed PR's head    1
#                              branch (never main, release/*, parked/*, a fork's, or an
#                              open PR's base or head)
#   KAI_FLUENCY_MIN            fluency gate after one repair, %   90
#   KAI_MODEL_PATHS            paths that change what the model   see below
#                              sees: such a PR needs the fluency gate, one per batch
#   KAI_BATCH_NOBOOT           1: PRs that change nothing the     1
#                              boot jobs see (noboot.sh) land first in a lint-only batch
KAI_SCRIPTS=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
R=${KAI_REPO:-dane1599d/kernel-ai}
G=${KAI_GIT_DIR:-$(git -C "$KAI_SCRIPTS" rev-parse --show-toplevel 2>/dev/null)}
[ -n "$G" ] || { echo "config.sh: set KAI_GIT_DIR to a checkout of $R" >&2; exit 2; }
MAIN=${KAI_MAIN:-main}
S=${KAI_BATCH_STATE:-$(git -C "$G" rev-parse --path-format=absolute --git-common-dir)/kai-batch}
mkdir -p "$S" || { echo "config.sh: cannot create $S" >&2; exit 2; }
# 15 PRs a batch: their TCG boot shards fit the 75-minute limit (ci.yml, #1080;
# 10 PRs took a shard 39-50 minutes under the old 50). Lower it with the
# batch-max file if the shards near the limit again.
MAX=${KAI_BATCH_MAX:-15}
DELETE_BRANCHES=${KAI_BATCH_DELETE_BRANCHES:-1}
FLUENCY_MIN=${KAI_FLUENCY_MIN:-90}
MODEL_PATHS=${KAI_MODEL_PATHS:-'^(llmproto/|infer/src/|quant/|gguf/|user/src/bin/llm|user/src/pool\.rs|agentabi/|shellcore/src/(agentcore(\.rs|/)|state\.rs))'}
NOBOOT=${KAI_BATCH_NOBOOT:-1}
