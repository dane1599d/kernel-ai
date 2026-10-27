"""Spike S09-1 with S08-2: publish a boot image as a signed TUF target, then fetch
and verify it as a client would (ADR 0008).

Keys are throwaway ECDSA P-256 keys made here and dropped at exit: no YubiKey
exists yet, so nothing signed by this script is ever trusted outside the run.
Usage: python tuf_target.py IMAGE WORKDIR. Prints one `measure` line per number.
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


def publish(image, repo):
    expires = datetime.now(timezone.utc).replace(microsecond=0) + timedelta(days=7)
    signers = {role: CryptoSigner.generate_ecdsa() for role in ROLES}
    root = Metadata(Root(expires=expires))
    for role, signer in signers.items():
        root.signed.add_key(signer.public_key, role)
    targets = Metadata(Targets(expires=expires))
    start = time.perf_counter()
    name = os.path.basename(image)
    targets.signed.targets[name] = TargetFile.from_file(name, image, ["sha256"])
    measure("hash_image_s", f"{time.perf_counter() - start:.2f}")
    os.makedirs(os.path.join(repo, "targets"))
    shutil.copy(image, os.path.join(repo, "targets", name))
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
    return name


def serve(repo):
    handler = lambda *a, **k: http.server.SimpleHTTPRequestHandler(*a, directory=repo, **k)
    server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), handler)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    return server


def fetch(repo, client, name, url):
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
    info = updater.get_targetinfo(name)
    if info is None:
        sys.exit(f"{name} is not a target")
    path = updater.download_target(info)
    measure("client_refresh_download_verify_s", f"{time.perf_counter() - start:.2f}")
    return path


def tamper(repo, client, name, url):
    """A flipped byte in the published image must be refused."""
    path = os.path.join(repo, "targets", name)
    with open(path, "r+b") as f:
        f.seek(os.path.getsize(path) // 2)
        byte = f.read(1)
        f.seek(-1, os.SEEK_CUR)
        f.write(bytes([byte[0] ^ 0xFF]))
    try:
        fetch(repo, client, name, url)
    except Exception as e:  # tuf raises LengthOrHashMismatchError
        measure("tampered_image_refused", type(e).__name__)
        return
    sys.exit("a tampered image was accepted")


def main():
    image, work = sys.argv[1], sys.argv[2]
    repo = os.path.join(work, "repo")
    os.makedirs(repo)
    name = publish(image, repo)
    server = serve(repo)
    url = f"http://127.0.0.1:{server.server_address[1]}/"
    path = fetch(repo, os.path.join(work, "client"), name, url)
    with open(path, "rb") as a, open(image, "rb") as b:
        if a.read() != b.read():
            sys.exit("the downloaded image differs from the published one")
    measure("verified_target", name)
    tamper(repo, os.path.join(work, "client-tampered"), name, url)
    server.shutdown()


if __name__ == "__main__":
    main()
