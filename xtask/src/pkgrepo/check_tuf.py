"""python-tuf's client against `cargo xtask pkgrepo build`'s repository (ADR 0011
section 3; CI only, by ADR 0011 question 3). Run by .github/workflows/pkgrepo.yml.

From the repository's own 1.root.json, python-tuf 7.0.1's ngclient refreshes the
top-level roles over HTTP, finds every package through the delegated `packages`
role, downloads it, and checks its length and SHA-256. Each package's header must
name what its `custom.kai` index entry says. Then one byte of a package is
changed, and a fresh client must refuse it.

Usage: python check_tuf.py REPO_DIR
"""

import functools
import http.server
import shutil
import sys
import tempfile
import threading
from pathlib import Path

from tuf.api import exceptions
from tuf.ngclient import Updater


class Quiet(http.server.SimpleHTTPRequestHandler):
    def log_message(self, *args: object) -> None:
        pass


def serve(directory: Path) -> http.server.ThreadingHTTPServer:
    handler = functools.partial(Quiet, directory=str(directory))
    server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), handler)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    return server


def client(base: str, root: Path, work: Path) -> Updater:
    """A client that trusts `root`, keeping its files under `work`."""
    (work / "metadata").mkdir(parents=True)
    (work / "targets").mkdir()
    shutil.copy(root, work / "metadata" / "root.json")
    return Updater(
        metadata_dir=str(work / "metadata"),
        metadata_base_url=f"{base}/metadata/",
        target_dir=str(work / "targets"),
        target_base_url=f"{base}/targets/",
    )


def header(package: bytes) -> dict[str, list[str]]:
    """The `kai-package 1` lines of a `kai-pkg 1` file, as a dict."""
    magic, first, rest = package.split(b"\n", 2)
    assert magic == b"kai-pkg 1", magic
    path, length = first.decode().removeprefix("file ").rsplit(" ", 1)
    assert path == "kai-package", path
    lines = rest[: int(length)].decode().splitlines()
    assert lines[0] == "kai-package 1", lines[0]
    fields: dict[str, list[str]] = {}
    for line in lines[1:]:
        key, value = line.split(" ", 1)
        fields.setdefault(key, []).append(value)
    return fields


def main() -> None:
    repo = Path(sys.argv[1])
    root = repo / "metadata" / "1.root.json"
    files = sorted((repo / "targets").iterdir())
    # A consistent snapshot's file is <sha256>.<target path>.
    targets = [f.name.split(".", 1)[1] for f in files]
    if not targets:
        sys.exit("the repository has no packages")
    server = serve(repo)
    base = f"http://127.0.0.1:{server.server_port}"
    with tempfile.TemporaryDirectory() as tmp:
        good = client(base, root, Path(tmp) / "good")
        good.refresh()
        for target in targets:
            info = good.get_targetinfo(target)
            if info is None:
                sys.exit(f"python-tuf finds no {target}")
            kai = info.unrecognized_fields["custom"]["kai"]
            package = Path(good.download_target(info)).read_bytes()
            fields = header(package)
            for key in ["name", "version", "kind", "summary", "licence"]:
                if fields[key] != [kai[key]]:
                    sys.exit(f"{target}: its header's {key} is {fields[key]}, its index says {kai[key]}")
            if fields.get("world") != ([kai["world"]] if "world" in kai else None):
                sys.exit(f"{target}: its header's world differs from its index")
            if fields.get("depends", []) != kai["depends"]:
                sys.exit(f"{target}: its header's depends differ from its index")
            if target != f"{kai['name']}-{kai['version']}.kpkg":
                sys.exit(f"{target} is not named for its name and version")
            print(f"python-tuf: {target} verified, {len(package)} bytes, actions {kai['actions']}")

        changed = files[0]
        original = changed.read_bytes()
        flipped = bytearray(original)
        flipped[40] ^= 1
        changed.write_bytes(bytes(flipped))
        try:
            bad = client(base, root, Path(tmp) / "bad")
            bad.refresh()
            bad.download_target(bad.get_targetinfo(targets[0]))
        except exceptions.RepositoryError as why:
            print(f"python-tuf: {targets[0]} with a changed byte is refused: {why}")
        else:
            sys.exit(f"python-tuf accepted {targets[0]} with a changed byte")
        finally:
            changed.write_bytes(original)
    server.shutdown()


if __name__ == "__main__":
    main()
