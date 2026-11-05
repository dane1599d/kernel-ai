"""Spike S-W1 (never merged): compare two GGUF files tensor by tensor and key by
key. It prints the metadata keys whose values differ and how many tensors'
bytes match, so a requantized file can be told apart from the pinned one by
its data, not by its header alone. Pure Python, GGUF version 2 or 3.

Usage: python3 compare.py A.gguf B.gguf
"""

import hashlib
import struct
import sys

# GGUF metadata value types.
SCALARS = {0: "<B", 1: "<b", 2: "<H", 3: "<h", 4: "<I", 5: "<i", 6: "<f", 7: "<?", 10: "<Q", 11: "<q", 12: "<d"}
STRING, ARRAY = 8, 9


class Reader:
    def __init__(self, f):
        self.f = f

    def take(self, fmt):
        size = struct.calcsize(fmt)
        return struct.unpack(fmt, self.f.read(size))[0]

    def string(self):
        n = self.take("<Q")
        return self.f.read(n).decode("utf-8", "replace")

    def value(self, kind):
        if kind in SCALARS:
            return self.take(SCALARS[kind])
        if kind == STRING:
            return self.string()
        if kind == ARRAY:
            inner, n = self.take("<I"), self.take("<Q")
            items = [self.value(inner) for _ in range(n)]
            # Long arrays (the vocabulary) are compared by hash.
            if n > 64:
                return "array[%d] %s" % (n, hashlib.sha256(repr(items).encode()).hexdigest()[:16])
            return items
        raise ValueError("unknown GGUF type %d" % kind)


def load(path):
    with open(path, "rb") as f:
        r = Reader(f)
        assert f.read(4) == b"GGUF", path
        version = r.take("<I")
        tensors, keys = r.take("<Q"), r.take("<Q")
        meta = {}
        for _ in range(keys):
            key = r.string()
            meta[key] = r.value(r.take("<I"))
        infos = []
        for _ in range(tensors):
            name = r.string()
            dims = [r.take("<Q") for _ in range(r.take("<I"))]
            kind, offset = r.take("<I"), r.take("<Q")
            infos.append((name, dims, kind, offset))
        align = meta.get("general.alignment", 32)
        start = (f.tell() + align - 1) // align * align
        f.seek(0, 2)
        end = f.tell()
        ordered = sorted(infos, key=lambda t: t[3])
        hashes = {}
        for i, (name, dims, kind, offset) in enumerate(ordered):
            stop = ordered[i + 1][3] if i + 1 < len(ordered) else end - start
            f.seek(start + offset)
            data = f.read(stop - offset)
            hashes[name] = (kind, dims, hashlib.sha256(data).hexdigest())
        return version, meta, hashes


def main():
    a, b = load(sys.argv[1]), load(sys.argv[2])
    print("s-w1: compare: versions %d and %d" % (a[0], b[0]))
    for key in sorted(set(a[1]) | set(b[1])):
        if a[1].get(key) != b[1].get(key):
            print("s-w1: compare: key %s: %r | %r" % (key, a[1].get(key), b[1].get(key)))
    same = [n for n in a[2] if b[2].get(n) == a[2][n]]
    diff = [n for n in a[2] if n in b[2] and b[2][n] != a[2][n]]
    print("s-w1: compare: %d of %d tensors byte-identical, %d differ, %d only in one file" % (
        len(same), len(a[2]), len(diff), len(set(a[2]) ^ set(b[2]))))
    for name in diff[:12]:
        print("s-w1: compare: differs %s type %d | %d" % (name, a[2][name][0], b[2][name][0]))


if __name__ == "__main__":
    main()
