"""Downloads every pinned model of xtask/models.json into OUT and checks its
SHA-256: for the `models` role's chunk lists (model-chunks.yml and release.yml's
sign job). The models are only hashed, never run, on GitHub's runners.

Each catalog entry is checked before it is used: the repository is `owner/name`,
the revision 40 hex digits, the SHA-256 64, and the file a bare `.gguf` name,
so nothing in the catalog can write outside OUT or fetch another revision.

    python3 -I .github/release/fetch_models.py OUT
"""

import hashlib
import json
import re
import subprocess
import sys
from pathlib import Path

PART = re.compile(r"[A-Za-z0-9][A-Za-z0-9_.-]*")
FILE = re.compile(r"[A-Za-z0-9][A-Za-z0-9_.-]*\.gguf")
REVISION = re.compile(r"[0-9a-f]{40}")
SHA256 = re.compile(r"[0-9a-f]{64}")


def checked(model: dict) -> dict:
    repo, revision, file, sha = model["repo"], model["revision"], model["file"], model["sha256"]
    owner, _, name = repo.partition("/")
    if not (PART.fullmatch(owner) and PART.fullmatch(name)) or ".." in repo:
        sys.exit(f"{model['id']}: repo {repo!r} is not owner/name")
    if not REVISION.fullmatch(revision):
        sys.exit(f"{model['id']}: revision {revision!r} is not 40 hex digits")
    if not FILE.fullmatch(file) or ".." in file or Path(file).name != file:
        sys.exit(f"{model['id']}: file {file!r} is not a bare .gguf name")
    if not SHA256.fullmatch(sha):
        sys.exit(f"{model['id']}: sha256 {sha!r} is not 64 hex digits")
    return model


def sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as f:
        while block := f.read(1 << 22):
            digest.update(block)
    return digest.hexdigest()


def main() -> None:
    out = Path(sys.argv[1])
    out.mkdir(parents=True, exist_ok=True)
    catalog = json.loads(Path("xtask/models.json").read_text())
    for model in map(checked, catalog["models"]):
        path = out / model["file"]
        url = f"https://huggingface.co/{model['repo']}/resolve/{model['revision']}/{model['file']}"
        if not path.exists():
            subprocess.run(["curl", "-fL", "--retry", "5", "-o", str(path), url], check=True)
        got = sha256(path)
        if got != model["sha256"]:
            path.unlink()
            sys.exit(f"{model['id']}: {path.name} has SHA-256 {got}, not the pinned {model['sha256']}")
        print(f"fetch_models: {model['id']} {path.name} {path.stat().st_size} bytes, SHA-256 as pinned")


if __name__ == "__main__":
    main()
