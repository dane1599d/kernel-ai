needed-by: P4 task 18, rc.2 (the founder's choice of 2026-11-14; the manager's design and decisions; AP-2 builds, K reviews)
decision: until the YubiKeys, two founder-held Ed25519 key files sign the image repository a release ships, release/pkgrepo/, and its root is also the image's kai.root.json
---
**The keys:** `root.key` signs root, 1 of 1. `release.key` signs targets, packages (empty), models, snapshot and timestamp, 1 of 1 each. Each is encrypted with its own passphrase (PKCS#8, scrypt) and made with OpenSSL 3. Two files on one PC under one person add nothing, so root is not 2 of 2: the protection is the passphrase, a stick plugged in only to sign, and an encrypted copy off-site.

**xtask never reads a private key:**
- `cargo xtask release-repo prepare` writes each role's canonical bytes.
- The founder runs `scripts/release-sign.sh` on a machine no Claude session runs on (their Mac with Homebrew `openssl@3`, or a live-USB Linux), from a copy on the key stick pinned by SHA-256, through `env -i bash --noprofile --norc`. Sessions run as the founder's Windows user, so a WSL distro of its own isolates nothing on this PC (the manager's review of #863).
- `prepare-timestamp` follows, then a second signing round, because the timestamp holds the signed snapshot's hash.
- `assemble` checks each signature with kaituf and the repository as a device does, then writes `release/pkgrepo/metadata/`.

**What it signs:** the `models` role lists `models::catalog()` only, never a test fixture, and every catalog model needs its committed chunk list. Non-root roles take version `release_seq`, so they only rise.

**Root files are frozen once committed:** root is never signed again. CI fails a pull request or a push that changes a committed `*.root.json`, and a tag whose root files differ from an earlier release's. The root key is also pinned outside the repository, as `RELEASE_ROOT_KEY` in the protected `release` environment, which `publish` checks. `updated` keeps a cached root of the same version, so a second version 1 would split devices.

**The image:** `release-image` checks the committed repository and bundles it as `pkgrepo/`, with its root as `kai.root.json` (root ceremony step 6.1). `--test-root` replaces `pkgrepo/` alone. Once a repository is committed, release-boot boots the release image itself and imports qwen3-0.6b under the founder's role.

**Later:** root version 2 on three YubiKeys (2 of 3), signed with `root.key` and two of the YubiKeys. Online snapshot and timestamp keys need that version 2 first. The runbook is `docs/release/root-ceremony.md`, "Interim: key files".
