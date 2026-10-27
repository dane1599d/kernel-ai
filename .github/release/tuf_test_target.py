"""release.yml's test-root signing (ADR 0008 section 8, P4 task 17): publish
the release's files as TUF targets of a throwaway repository, then fetch and
verify each as a client would, and check that a tampered copy is refused.

Keys are throwaway ECDSA P-256 keys made here and dropped at exit. The real
signing event, tuf-on-ci with the founder's YubiKeys, is P4 task 18; until then
nothing signed by this script is ever trusted outside the run.
Usage: python tuf_test_target.py WORKDIR FILE... Prints one `measure` line per number.
"""

import http.server
import os
import shutil
import sys
import threading
import time
from datetime import datetime, timedelta, timezone

from securesystemslib.signer import CryptoSigner
from tuf.api.metadata import (
    Metadata,
    MetaFile,
    Root,
    Snapshot,
    TargetFile,
    Targets,
    Timestamp,
)
from tuf.api.serialization.json import JSONSerializer
from tuf.ngclient import Updater

ROLES = (Root.type, Targets.type, Snapshot.type, Timestamp.type)


def measure(name, value):
    print(f"measure {name} {value}")


def publish(files, repo):
    expires = datetime.now(timezone.utc).replace(microsecond=0) + timedelta(days=7)
    signers = {role: CryptoSigner.generate_ecdsa() for role in ROLES}
    # Plain file names: the spike serves one version of each role.
    root = Metadata(Root(expires=expires, consistent_snapshot=False))
    for role, signer in signers.items():
        root.signed.add_key(signer.public_key, role)
    targets = Metadata(Targets(expires=expires))
    os.makedirs(os.path.join(repo, "targets"))
    start = time.perf_counter()
    for path in files:
        name = os.path.basename(path)
        targets.signed.targets[name] = TargetFile.from_file(name, path, ["sha256"])
        shutil.copy(path, os.path.join(repo, "targets", name))
    measure("hash_targets_s", f"{time.perf_counter() - start:.2f}")
    snapshot = Metadata(Snapshot(expires=expires))
    snapshot.signed.meta["targets.json"] = MetaFile(version=1)
    timestamp = Metadata(Timestamp(expires=expires))
    timestamp.signed.snapshot_meta = MetaFile(version=1)
    serializer = JSONSerializer(compact=False)
    start = time.perf_counter()
    for role, md in zip(ROLES, (root, targets, snapshot, timestamp)):
        md.sign(signers[role])
        path = os.path.join(repo, f"1.{role}.json" if role == Root.type else f"{role}.json")
        md.to_file(path, serializer)
        measure(f"{role}_json_bytes", os.path.getsize(path))
    measure("sign_four_roles_ms", f"{(time.perf_counter() - start) * 1000:.1f}")


def serve(repo):
    handler = lambda *a, **k: http.server.SimpleHTTPRequestHandler(*a, directory=repo, **k)
    server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), handler)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    return server


def fetch(repo, client, names, url):
    os.makedirs(os.path.join(client, "downloads"))
    shutil.copy(os.path.join(repo, "1.root.json"), os.path.join(client, "root.json"))
    start = time.perf_counter()
    updater = Updater(
        metadata_dir=client,
        metadata_base_url=url,
        target_base_url=url + "targets/",
        target_dir=os.path.join(client, "downloads"),
    )
    updater.refresh()
    paths = []
    for name in names:
        info = updater.get_targetinfo(name)
        if info is None:
            sys.exit(f"{name} is not a target")
        paths.append(updater.download_target(info))
    measure("client_refresh_download_verify_s", f"{time.perf_counter() - start:.2f}")
    return paths


def tamper(repo, client, name, url):
    """A flipped byte in the published image must be refused."""
    path = os.path.join(repo, "targets", name)
    with open(path, "r+b") as f:
        f.seek(os.path.getsize(path) // 2)
        byte = f.read(1)
        f.seek(-1, os.SEEK_CUR)
        f.write(bytes([byte[0] ^ 0xFF]))
    try:
        fetch(repo, client, [name], url)
    except Exception as e:  # tuf raises LengthOrHashMismatchError
        measure("tampered_image_refused", type(e).__name__)
        return
    sys.exit("a tampered image was accepted")


def main():
    work, files = sys.argv[1], sys.argv[2:]
    repo = os.path.join(work, "repo")
    os.makedirs(repo)
    publish(files, repo)
    server = serve(repo)
    url = f"http://127.0.0.1:{server.server_address[1]}/"
    names = [os.path.basename(f) for f in files]
    paths = fetch(repo, os.path.join(work, "client"), names, url)
    for path, original, name in zip(paths, files, names):
        with open(path, "rb") as a, open(original, "rb") as b:
            if a.read() != b.read():
                sys.exit(f"the downloaded {name} differs from the published one")
        measure("verified_target", name)
    tamper(repo, os.path.join(work, "client-tampered"), names[0], url)
    server.shutdown()


if __name__ == "__main__":
    main()
