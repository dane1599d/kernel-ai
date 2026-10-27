"""kaituf's interoperability checks for spikes S11-1 and S08-3, run in CI by
.github/workflows/kaituf.yml with python-tuf 7.0.1 (CI only, ADR 0011 question 3).

1. Repositories python-tuf makes with Ed25519 keys, P-256 keys and both: the
   client downloads a package through the delegated `packages` role.
2. Tampered variants of the P-256 one: each must be refused.
3. tuf-on-ci v0.21.0's expected fixtures: every file parses, and each fixture's
   root, which needs a P-384 signature, is refused as "unsupported key type".
4. S08-3: kaituf's canonical bytes and serde_json's compact bytes of every
   metadata file's `signed`, against securesystemslib's encode_canonical.
5. A repository tuftool 0.17.0 makes with a P-256 key, if --tuftool is given.

Usage: python check.py --client KAITUF_CLIENT --tuf-on-ci DIR [--tuftool TUFTOOL]
"""

import argparse
import datetime
import glob
import hashlib
import http.server
import json
import os
import subprocess
import sys
import tempfile
import threading

from cryptography.hazmat.primitives.asymmetric import ec
from securesystemslib.formats import encode_canonical
from securesystemslib.signer import CryptoSigner
from tuf.api.metadata import (
    DelegatedRole,
    Delegations,
    Metadata,
    MetaFile,
    Root,
    Snapshot,
    TargetFile,
    Targets,
    Timestamp,
)
from tuf.api.serialization.json import JSONSerializer

NOW = datetime.datetime.now(datetime.timezone.utc).replace(microsecond=0)
LATER = NOW + datetime.timedelta(days=30)
PAST = NOW - datetime.timedelta(days=1)
TARGET = "notes-1.0.0.kpkg"
CONTENT = b"kai-pkg 1\nfile kai-package 14\nkai-package 1\n"
TOP = ["root", "timestamp", "snapshot", "targets"]
ROLES = [*TOP, "packages"]

failures: list[str] = []
summary: list[str] = []


def signer(kind: str) -> CryptoSigner:
    if kind == "ed25519":
        return CryptoSigner.generate_ed25519()
    if kind == "p256":
        return CryptoSigner.generate_ecdsa()
    if kind == "p384":
        return CryptoSigner(ec.generate_private_key(ec.SECP384R1()))
    raise ValueError(kind)


def sign(signed, signers) -> bytes:
    md = Metadata(signed)
    for s in signers:
        md.sign(s, append=True)
    return md.to_bytes(JSONSerializer())


class Repo:
    """A repository with consistent snapshots: root, targets delegating *.kpkg
    to `packages`, which names one package. files() signs it as it stands."""

    def __init__(self, kinds: dict[str, str]):
        self.keys = {role: [signer(kinds[role])] for role in ROLES}
        self.threshold = {role: 1 for role in ROLES}
        self.version = {role: 1 for role in ROLES}
        self.expires = {role: LATER for role in ROLES}
        self.signing: dict[str, list] = {}
        self.edits: dict[str, object] = {}
        self.next_root = None  # a callable giving root v2's bytes
        self.initial = sign(self.root(1, LATER), self.keys["root"])

    def root(self, version, expires) -> Root:
        root = Root(version=version, expires=expires, consistent_snapshot=True)
        for role in TOP:
            for s in self.keys[role]:
                root.add_key(s.public_key, role)
            root.roles[role].threshold = self.threshold[role]
        return root

    def signed(self, role, signed) -> bytes:
        if role in self.edits:
            self.edits[role](signed)
        return sign(signed, self.signing.get(role, self.keys[role]))

    def files(self) -> dict[str, bytes]:
        v = self.version
        out = {"metadata/1.root.json": self.initial}
        if self.next_root:
            out["metadata/2.root.json"] = self.next_root(self)
        packages = Targets(version=v["packages"], expires=self.expires["packages"])
        packages.targets[TARGET] = TargetFile.from_data(TARGET, CONTENT, ["sha256"])
        targets = Targets(version=v["targets"], expires=self.expires["targets"])
        delegate = DelegatedRole(
            "packages", [s.public_key.keyid for s in self.keys["packages"]], self.threshold["packages"], True, ["*.kpkg"]
        )
        keys = {s.public_key.keyid: s.public_key for s in self.keys["packages"]}
        targets.delegations = Delegations(keys, {"packages": delegate})
        out[f"metadata/{v['packages']}.packages.json"] = self.signed("packages", packages)
        out[f"metadata/{v['targets']}.targets.json"] = self.signed("targets", targets)
        meta = {"targets.json": MetaFile(v["targets"]), "packages.json": MetaFile(v["packages"])}
        snapshot = self.signed("snapshot", Snapshot(version=v["snapshot"], expires=self.expires["snapshot"], meta=meta))
        out[f"metadata/{v['snapshot']}.snapshot.json"] = snapshot
        named = MetaFile(v["snapshot"], len(snapshot), {"sha256": hashlib.sha256(snapshot).hexdigest()})
        out["metadata/timestamp.json"] = self.signed(
            "timestamp", Timestamp(version=v["timestamp"], expires=self.expires["timestamp"], snapshot_meta=named)
        )
        out[f"targets/{hashlib.sha256(CONTENT).hexdigest()}.{TARGET}"] = CONTENT
        return out


class Handler(http.server.BaseHTTPRequestHandler):
    def do_GET(self) -> None:
        data = self.server.files.get(self.path.lstrip("/"))
        if data is None:
            self.send_error(404)
            return
        self.send_response(200)
        self.send_header("Content-Length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)

    def log_message(self, *args) -> None:
        pass


SERVER = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Handler)
SERVER.files = {}
threading.Thread(target=SERVER.serve_forever, daemon=True).start()
BASE = f"http://127.0.0.1:{SERVER.server_port}"


def client_download(client: str, states: list[dict[str, bytes]], target=TARGET) -> subprocess.CompletedProcess:
    """Initializes a client from the first state's 1.root.json, then downloads
    `target` once per state, returning the last run."""
    with tempfile.TemporaryDirectory() as tmp:
        md, tg = os.path.join(tmp, "metadata"), os.path.join(tmp, "targets")
        os.mkdir(md)
        os.mkdir(tg)
        root = os.path.join(tmp, "root.json")
        with open(root, "wb") as f:
            f.write(states[0]["metadata/1.root.json"])
        subprocess.run([client, "--metadata-dir", md, "init", root], check=True)
        for files in states:
            SERVER.files = files
            run = subprocess.run(
                [client, "--metadata-dir", md, "--metadata-url", f"{BASE}/metadata/", "--target-name", target,
                 "--target-base-url", f"{BASE}/targets/", "--target-dir", tg, "download"],
                capture_output=True, text=True,
            )
        if run.returncode == 0:
            got = open(os.path.join(tg, target), "rb").read()
            if got != CONTENT:
                run = subprocess.CompletedProcess(run.args, 1, "", "downloaded the wrong bytes")
        return run


def flip(data: bytes) -> bytes:
    """`data` with the first digit after "expires" changed: valid JSON, a
    different signed message."""
    at = data.index(b'"expires"')
    at += next(i for i, b in enumerate(data[at:]) if chr(b).isdigit())
    return data[:at] + bytes([ord("0") + (data[at] - ord("0") + 1) % 10]) + data[at + 1:]


def flipped(path_prefix: str):
    def make(repo: Repo):
        files = repo.files()
        name = next(n for n in files if n.startswith(path_prefix))
        files[name] = flip(files[name]) if name.endswith(".json") else files[name][:-1] + b"?"
        return [files]
    return make


def expired(role: str):
    def make(repo: Repo):
        if role == "root":
            repo.next_root = lambda r: sign(r.root(2, PAST), r.keys["root"])
        else:
            repo.expires[role] = PAST
        return [repo.files()]
    return make


def rolled_back(role: str):
    """First a repository with `role` at version 2, then one with it at 1 and
    everything that names it moved on."""
    def make(repo: Repo):
        for r in ROLES[1:]:
            repo.version[r] = 2
        first = repo.files()
        repo.version[role] = 1
        for r in ROLES[1:]:
            if r != role:
                repo.version[r] = 3
        return [first, repo.files()]
    return make


def root_v2(sign_with_old: bool):
    def make(repo: Repo):
        old = repo.keys["root"]
        repo.keys["root"] = [signer("p256")]
        signers = old + repo.keys["root"] if sign_with_old else repo.keys["root"]
        repo.next_root = lambda r: flip(sign(r.root(2, LATER), signers)) if sign_with_old else sign(r.root(2, LATER), signers)
        return [repo.files()]
    return make


def below_threshold(repo: Repo):
    repo.keys["snapshot"].append(signer("p256"))
    repo.threshold["snapshot"] = 2
    repo.initial = sign(repo.root(1, LATER), repo.keys["root"])
    repo.signing["snapshot"] = repo.keys["snapshot"][:1]
    return [repo.files()]


def foreign_snapshot_key(repo: Repo):
    repo.signing["snapshot"] = [signer("p256")]
    return [repo.files()]


def foreign_snapshot_hash(repo: Repo):
    repo.edits["timestamp"] = lambda ts: setattr(ts.snapshot_meta, "hashes", {"sha256": "ab" * 32})
    return [repo.files()]


def padded_timestamp(repo: Repo):
    repo.edits["timestamp"] = lambda ts: ts.unrecognized_fields.update({"padding": "x" * 20_000})
    return [repo.files()]


def long_target(repo: Repo):
    files = repo.files()
    name = next(n for n in files if n.startswith("targets/"))
    files[name] = CONTENT + b"more"
    return [files]


def p384_packages(repo: Repo):
    repo.keys["packages"] = [signer("p384")]
    return [repo.files()]


CASES = [
    ("a changed timestamp", flipped("metadata/timestamp.json"), "below the signature threshold"),
    ("a changed snapshot", flipped("metadata/1.snapshot.json"), "length or hash"),
    ("a changed targets", flipped("metadata/1.targets.json"), "below the signature threshold"),
    ("a changed packages", flipped("metadata/1.packages.json"), "below the signature threshold"),
    ("a changed root v2", root_v2(True), "below the signature threshold"),
    ("a changed package", flipped("targets/"), "length or hash"),
    ("an expired root", expired("root"), "expired: root"),
    ("an expired timestamp", expired("timestamp"), "expired: timestamp"),
    ("an expired snapshot", expired("snapshot"), "expired: snapshot"),
    ("an expired targets", expired("targets"), "expired: targets"),
    ("an expired packages", expired("packages"), "expired: packages"),
    ("a timestamp rollback", rolled_back("timestamp"), "bad version"),
    ("a snapshot rollback", rolled_back("snapshot"), "bad version"),
    ("a targets rollback", rolled_back("targets"), "bad version"),
    ("a packages rollback", rolled_back("packages"), "bad version"),
    ("below the snapshot threshold", below_threshold, "below the signature threshold"),
    ("root v2 without the old key", root_v2(False), "below the signature threshold"),
    ("a snapshot signed by another key", foreign_snapshot_key, "below the signature threshold"),
    ("a timestamp naming another snapshot", foreign_snapshot_hash, "length or hash"),
    ("an oversize timestamp", padded_timestamp, "too large"),
    ("an oversize package", long_target, "too large"),
    ("packages needing a P-384 key", p384_packages, "unsupported key type"),
]


def check(name: str, ok: bool, detail: str) -> None:
    summary.append(f"| {name} | {'pass' if ok else 'FAIL'} | {detail} |")
    print(f"{'pass' if ok else 'FAIL'}: {name}: {detail}")
    if not ok:
        failures.append(name)


def python_tuf(client: str, out: str) -> None:
    for kinds in ["ed25519", "p256", "mixed"]:
        roles = {r: (kinds if kinds != "mixed" else ("p256" if r in ("root", "targets") else "ed25519")) for r in ROLES}
        repo = Repo(roles)
        files = repo.files()
        for name, data in files.items():
            if name.endswith(".json"):
                path = os.path.join(out, kinds, name)
                os.makedirs(os.path.dirname(path), exist_ok=True)
                open(path, "wb").write(data)
        run = client_download(client, [files])
        check(f"python-tuf {kinds} repository", run.returncode == 0, run.stderr.strip() or "downloaded and verified")
    for name, make, expect in CASES:
        run = client_download(client, make(Repo({r: "p256" for r in ROLES})))
        refused = run.returncode != 0 and expect in run.stderr
        check(f"refuses {name}", refused, run.stderr.strip().removeprefix("kaituf-client: ") or "accepted")


def tuf_on_ci(client: str, fixtures: str) -> list[str]:
    files = sorted(glob.glob(os.path.join(fixtures, "*", "metadata", "*.json")))
    bad = [f for f in files if subprocess.run([client, "parse", f], capture_output=True).returncode != 0]
    check(f"tuf-on-ci: {len(files)} fixture files parse", not bad, ", ".join(bad) or "all parse")
    for fixture in sorted(glob.glob(os.path.join(fixtures, "*"))):
        SERVER.files = {}
        for f in glob.glob(os.path.join(fixture, "**", "*"), recursive=True):
            if os.path.isfile(f):
                SERVER.files[os.path.relpath(f, fixture).replace(os.sep, "/")] = open(f, "rb").read()
        with tempfile.TemporaryDirectory() as tmp:
            md = os.path.join(tmp, "metadata")
            os.mkdir(md)
            subprocess.run([client, "--metadata-dir", md, "init", os.path.join(fixture, "metadata", "1.root.json")], check=True)
            run = subprocess.run([client, "--metadata-dir", md, "--metadata-url", f"{BASE}/metadata/", "refresh"],
                                 capture_output=True, text=True)
        named = run.returncode != 0 and "unsupported key type: root" in run.stderr
        check(f"tuf-on-ci {os.path.basename(fixture)}: its P-384 root is refused by name", named, run.stderr.strip())
    return files


def canonical(client: str, files: list[str]) -> None:
    same = {"kaituf": 0, "serde_json": 0}
    differ = {"kaituf": [], "serde_json": []}
    for f in files:
        want = encode_canonical(json.load(open(f, "rb"))["signed"]).encode("utf-8")
        for form, tool in [("canonical", "kaituf"), ("serde", "serde_json")]:
            got = subprocess.run([client, form, f], capture_output=True).stdout
            if got == want:
                same[tool] += 1
            else:
                differ[tool].append(os.path.basename(f))
    n = len(files)
    check(f"S08-3: kaituf's canonical bytes on {n} files", same["kaituf"] == n, f"{same['kaituf']}/{n} identical")
    serde = f"{same['serde_json']}/{n} identical; differ: {', '.join(sorted(set(differ['serde_json']))[:8])}"
    summary.append(f"| S08-3: serde_json's compact bytes on {n} files | recorded | {serde} |")
    print(f"recorded: serde_json: {serde}")


def ed25519_pkcs8_v2(path: str) -> None:
    """An Ed25519 key as PKCS#8 v2 (with its public key), the form ring and
    aws-lc-rs make and the only Ed25519 form tough loads; openssl writes v1."""
    from cryptography.hazmat.primitives import serialization
    from cryptography.hazmat.primitives.asymmetric.ed25519 import Ed25519PrivateKey

    key = Ed25519PrivateKey.generate()
    raw = serialization.Encoding.Raw
    seed = key.private_bytes(raw, serialization.PrivateFormat.Raw, serialization.NoEncryption())
    public = key.public_key().public_bytes(raw, serialization.PublicFormat.Raw)
    der = bytes.fromhex("3053020101300506032b657004220420") + seed + bytes.fromhex("a123032100") + public
    open(path, "wb").write(der)


# How to make a key tuftool 0.17.0 signs with: from a DER PKCS#8 file only, for
# Ed25519 and ECDSA (tough's parse_keypair). Its own tests sign with RSA.
KEY_RECIPES = {
    "P-256 (openssl genpkey)": lambda key, _: [
        ["openssl", "genpkey", "-algorithm", "EC", "-pkeyopt", "ec_paramgen_curve:P-256", "-outform", "DER", "-out", key]
    ],
    "P-256 (openssl ecparam, pkcs8 -topk8)": lambda key, work: [
        ["openssl", "ecparam", "-name", "prime256v1", "-genkey", "-noout", "-out", os.path.join(work, "ec.pem")],
        ["openssl", "pkcs8", "-topk8", "-nocrypt", "-in", os.path.join(work, "ec.pem"), "-outform", "DER", "-out", key],
    ],
    "Ed25519 (PKCS#8 v2)": None,
}


def tuftool(client: str, tool: str, out: str) -> list[str]:
    """A repository tuftool makes with each key recipe: one tuftool cannot load
    is recorded as tuftool's limit; one it makes that kaituf refuses fails."""
    made_files: list[str] = []
    for n, (name, recipe) in enumerate(KEY_RECIPES.items()):
        work = os.path.join(out, f"tuftool-{n}")
        os.makedirs(os.path.join(work, "input"), exist_ok=True)
        key, root = os.path.join(work, "key.der"), os.path.join(work, "root.json")
        open(os.path.join(work, "input", TARGET), "wb").write(CONTENT)
        if recipe is None:
            ed25519_pkcs8_v2(key)
            keygen = []
        else:
            keygen = recipe(key, work)
        # Whole seconds: from a relative date ("in 7 days") tuftool writes fractional
        # seconds, which the spec's YYYY-MM-DDTHH:MM:SSZ, python-tuf and kaituf refuse.
        later = LATER.strftime("%Y-%m-%dT%H:%M:%SZ")
        steps = [
            *keygen,
            [tool, "root", "init", root],
            [tool, "root", "expire", root, later],
            *[[tool, "root", "set-threshold", root, role, "1"] for role in TOP],
            [tool, "root", "add-key", root, "-k", key, *[a for role in TOP for a in ("--role", role)]],
            [tool, "root", "sign", root, "-k", key],
            [tool, "create", "--root", root, "--key", key, "--add-targets", os.path.join(work, "input"),
             "--targets-expires", later, "--targets-version", "1", "--snapshot-expires", later,
             "--snapshot-version", "1", "--timestamp-expires", later, "--timestamp-version", "1",
             "--outdir", os.path.join(work, "repo")],
        ]
        label = f"tuftool 0.17.0 repository, {name}"
        failed = None
        for step in steps:
            run = subprocess.run(step, capture_output=True, text=True)
            if run.returncode != 0:
                failed = f"{' '.join(os.path.basename(a) for a in step[:3])}: {run.stderr.strip()[:300]}"
                break
        if failed:
            summary.append(f"| {label} | recorded | tuftool did not make it: {failed} |")
            print(f"recorded: {label}: tuftool did not make it: {failed}")
            continue
        repo = os.path.join(work, "repo")
        files = {}
        for f in glob.glob(os.path.join(repo, "**", "*"), recursive=True):
            if os.path.isfile(f):
                files[os.path.relpath(f, repo).replace(os.sep, "/")] = open(f, "rb").read()
        if "metadata/1.root.json" not in files:
            check(label, False, f"no metadata/1.root.json in {sorted(files)}")
            continue
        run = client_download(client, [files])
        check(label, run.returncode == 0, run.stderr.strip() or "downloaded and verified")
        made_files += sorted(glob.glob(os.path.join(repo, "metadata", "*.json")))
    return made_files


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--client", required=True)
    parser.add_argument("--tuf-on-ci", required=True)
    parser.add_argument("--tuftool")
    args = parser.parse_args()
    client = os.path.abspath(args.client)
    with tempfile.TemporaryDirectory() as out:
        python_tuf(client, out)
        files = sorted(glob.glob(os.path.join(out, "**", "*.json"), recursive=True))
        files += tuf_on_ci(client, os.path.join(args.tuf_on_ci, "tests", "expected"))
        if args.tuftool:
            files += tuftool(client, args.tuftool, out)
        canonical(client, files)
    report = "| check | result | detail |\n|---|---|---|\n" + "\n".join(summary) + "\n"
    if os.environ.get("GITHUB_STEP_SUMMARY"):
        open(os.environ["GITHUB_STEP_SUMMARY"], "a").write(report)
    print(f"{len(summary) - len(failures)} of {len(summary)} checks hold")
    if failures:
        sys.exit(f"failed: {', '.join(failures)}")


if __name__ == "__main__":
    main()
