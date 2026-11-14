# Release signing: the `kernel-ai-updates` repository and the root ceremony

P4 task 18 (ADR 0008 §7). This is the founder's runbook. It sets up the TUF repository that signs v0.1 and every later release, and it holds the keys that every Kernel AI device trusts. Nothing here runs from a Claude session: the repository, its settings, the YubiKeys and every signature are yours. Steps marked **(unverified)** come from tuf-on-ci 0.21.0's documents, which say no more on them. Try them first on a throwaway repository (step 0).

Until the YubiKeys, releases are signed with two key files instead: the next section. The sections after it describe the YubiKey setup that replaces them.

## Interim: key files (until the YubiKeys)

You chose this on 2026-11-14. Two Ed25519 key files, each encrypted with its own passphrase, sign the image repository a release carries, `release/pkgrepo/`:

| File | Signs | Threshold |
|---|---|---|
| `root.key` | root (version 1, once) | 1 of 1 |
| `release.key` | targets, packages (empty), models, snapshot, timestamp | 1 of 1 each |

**What it gives a release.** The image carries `release/pkgrepo/` as `pkgrepo/`. Its `models` role names every model in `xtask/models.json`, so Models imports them, and it is what the download offers come from. Its `1.root.json` also ships as `kai.root.json`, the root that update disks and the online repository are checked against. One root covers both, and the YubiKey root later chains from it.

**What it does not do.** The kernel boots any model disk attached as `kai-model`, or a disk that starts with the GGUF magic, without a signature. That is how `cargo xtask vbox` VMs get their model, and attaching `kai-model.vmdk` to a release VM works without any key. On VirtualBox no disk can be the import disk (`kai-import`), so the role matters there only for downloads.

**Rules that keep it safe.**

- Two files on one PC under one person add nothing, so root is 1 of 1. The protection is the passphrase, a USB stick plugged in only to sign, and an encrypted copy kept in another building.
- Sign only in a WSL distro of its own, which no Claude session uses. Sessions use `Ubuntu` for their sandbox and QEMU, and a planted `openssl` or `.bashrc` there could catch a passphrase. Run every command through `env -i /bin/bash --noprofile --norc` and call `/usr/bin/openssl` by its full path.
- The sign script runs from the key stick, never from a checkout, which a session could change between your review and your run. Check its SHA-256 before each run. The script at the commit that added it has this SHA-256: `ea71c6a5a3ca98af417240ceacfca7709eb4b3b0561f3275602943488f213b4b`. A test in `xtask/src/release_repo/tests.rs` holds this page to the script, so a change to either shows in review.
- `release/pkgrepo/metadata/1.root.json` is frozen once committed. `cargo xtask release-repo` reuses it and never signs root again. CI fails any pull request that changes or removes it, and any tag whose root differs from the last release's. Two different "version 1" roots would split devices, because `updated` keeps a cached root of the same version.
- Never put a key file under the OneDrive Desktop folder, where the main checkout lives. Never type a passphrase into a Claude chat, or into a terminal a session controls.

### One-time setup (about 30 minutes)

1. In your own PowerShell window, run `wsl --install -d Debian`. When it asks, pick a user name and password for it. This is the signing distro. Never ask a session to use it.
2. In that Debian window, run `sudo apt update && sudo apt install -y openssl`, then `/usr/bin/openssl version`. It must start with `OpenSSL 3`.
3. Plug in a USB stick kept only for these keys. If Windows shows it as `F:`, run `sudo mkdir -p /mnt/f && sudo mount -t drvfs F: /mnt/f`. Use the stick's own letter if it is not `F:`.
4. Run `env -i /bin/bash --noprofile --norc`, then `cd /mnt/f && mkdir -p kai-keys && cd kai-keys`.
5. Make the root key: `/usr/bin/openssl genpkey -algorithm ed25519 | /usr/bin/openssl pkcs8 -topk8 -v2 aes-256-cbc -scrypt -out root.key`. It asks for a new passphrase twice. Use six or more random words.
6. Make the release key the same way, with a different passphrase: `/usr/bin/openssl genpkey -algorithm ed25519 | /usr/bin/openssl pkcs8 -topk8 -v2 aes-256-cbc -scrypt -out release.key`.
7. Run `head -1 root.key release.key`. Both must say `-----BEGIN ENCRYPTED PRIVATE KEY-----`. If either does not, delete it and repeat its step.
8. Write the public halves: `/usr/bin/openssl pkey -in root.key -pubout -out root.pub`, then `/usr/bin/openssl pkey -in release.key -pubout -out release.pub`. Each asks for its key's passphrase.
9. Get the sign script at the commit the session names (this page's pull request, once merged): `/usr/bin/curl -fsSLo release-sign.sh https://raw.githubusercontent.com/dane1599d/kernel-ai/<commit>/scripts/release-sign.sh`. Read it: it is under 70 lines. Then run `/usr/bin/sha256sum release-sign.sh`. It must match the SHA-256 above. Write it down with the passphrases.
10. Save both passphrases in your password manager, and on paper kept away from the stick.
11. Copy `root.key`, `release.key` and `release-sign.sh` to a second stick. Check the copies open: `/usr/bin/openssl pkey -in <copy> -noout` for each key, with its passphrase. Keep that stick in another building, and check it again once a year: sticks fail without warning.
12. Give the session `root.pub` and `release.pub`, the files or their text. They are public. The session commits them as `release/keys/`.
13. Run `exit`, `cd /`, then `sudo umount /mnt/f`, and eject the stick in Windows.

### Each release

The session prepares the files to sign in a folder it names, for example `E:\kai-release\rc2`, which Debian sees as `/mnt/e/kai-release/rc2`. Signing takes two rounds, because the timestamp holds the hash of the signed snapshot.

1. The session runs `cargo xtask release-repo prepare --dir <folder>`. It tells you the folder and pastes the `sha256` line `prepare` printed for each file.
2. Plug in the key stick and, in Debian, mount it as in setup step 3.
3. Run `/usr/bin/sha256sum /mnt/f/kai-keys/release-sign.sh`. It must match the SHA-256 you wrote down. If it does not, stop.
4. Run `env -i /bin/bash --noprofile --norc /mnt/f/kai-keys/release-sign.sh /mnt/f/kai-keys /mnt/e/kai-release/rc2`, using the folder the session named. Type each passphrase it asks for.
   - Root's passphrase is asked only for the first release, the one that signs root. If it asks for root's on any later release, press Ctrl-C and stop: root is frozen, and a request for it means someone changed the folder.
   - For each file it prints `signed <file> with <key>: sha256 <hash>`. Each hash must be the one the session pasted for that file. If one differs, the folder changed after `prepare`: tell the session, and nothing goes further.
5. Run `sudo umount /mnt/f`, eject the stick, and tell the session "signed".
6. The session runs `cargo xtask release-repo prepare-timestamp` and pastes its one `sha256` line. Repeat steps 2 to 5: this time it asks only for the release passphrase.
7. The session runs `cargo xtask release-repo assemble`. It checks every signature, the repository as a device does, and that every signed byte is what `prepare` writes for `release/keys/` and `xtask/models.json`, with no expiry later than its period allows. Then it commits `release/pkgrepo/` and opens a pull request. CI checks the same on clean runners, and the release dry run boots the image and imports qwen3-0.6b under the role you signed.

The session never needs the stick or a passphrase. A model added to `xtask/models.json` makes a release fail until it is signed again this way.

**What is still trusted:** you sign the bytes the session prepared. The checks above catch a repository that is not what xtask writes, and a folder changed after `prepare`. They cannot catch a session that prepares other bytes and lies about their hashes, then keeps the signatures instead of committing them. Your checks against that are the rule on root's passphrase in step 4, and signing only when a session asks for a release you expect.

### Expiry and online updates

- Model import never checks expiry, so the role in an image keeps working.
- Update disks and the online repository are checked at the real time. A repository signed this way stops being accepted 7 days after its timestamp is signed. Until there is an online key, an online repository signed offline needs a longer timestamp expiry, for example 30 days, re-signed by hand.
- Root expires 365 days after you sign it. After that, update disks and online updates stop until `2.root.json` is published. Set a reminder for month 10 to make the YubiKey root (below), or a root version 2 signed with `root.key`.
- rc.2's root names `release.key` for snapshot and timestamp. An online CI key therefore needs a root version 2 first.
- rc.1 carries no `kai.root.json`, so an rc.1 install can never take an update. It must be reinstalled from rc.2 or later.

### Moving to the YubiKeys

Installed devices follow, because the new root is signed by the old one. Nothing for this is built yet. A session builds the tools when you have the YubiKeys.

1. Set up the YubiKeys as in section 3 below. Their keys never leave them.
2. A session builds root version 2: root is the three YubiKey P-256 keys, 2 of 3. Targets, packages and models are two YubiKeys, 1 of 2. Snapshot and timestamp are the online key chosen then.
3. You sign version 2 twice over: with `root.key`, which version 1 asks for, and with two of the three YubiKeys, which version 2 asks for. kaituf checks a new root against both, and requires the version to rise by one. Its tests cover an Ed25519 root rotating to a P-256 one.
4. `2.root.json` is published beside `1.root.json`, and both are kept forever: in `kernel-ai-updates`, and in each later release's `release/pkgrepo/metadata/`. Releases from then on are signed with the YubiKeys.
5. Devices fetch `2.root.json` at their next update check and trust only the YubiKeys from then on. Rotation ignores expiry, so a device offline for more than a year still follows.
6. Once a test VM has followed the rotation, delete `root.key` and `release.key` from both sticks.

### If a key file is lost

- **One stick, or one passphrase, with the other copy still good:** use the copy, and make a new second copy.
- **`release.key` and both its copies:** `root.key` signs a root version 2 that names a new release key.
- **`root.key` and both its copies, or its passphrase:** no one can sign a new root. Devices keep working, and model import keeps working, but they can take no update until they are reinstalled from an image with a new root. This is why the second copy and the paper passphrase both matter.
- **Either key may have leaked:** stop releasing, and have a session make root version 2 with new keys at once. Anyone with the file and its passphrase can sign anything until then.

## What you end with

| Role | Keys | Threshold | Expiry | Signs |
|---|---|---|---|---|
| `root` | 3 YubiKeys (PIV, ECC P-256), one kept off-site | 2 of 3 | 365 days, re-signed from 60 days before | the other roles' keys |
| `targets` | 2 YubiKeys | 1 of 2 | 365 days | slot images, R images, `SHA256SUMS`; delegates the two below |
| `packages` | 2 YubiKeys | 1 of 2 | 365 days | `*.kpkg` (ADR 0011), terminating |
| `models` | 2 YubiKeys | 1 of 2 | 365 days | `*.gguf` and their `kai-chunks 1` lists, `<sha256>.chunks` |
| `snapshot`, `timestamp` | one online key in a cloud KMS, reached through GitHub OIDC | 1 | 30 and 7 days, re-signed by CI | the repository's current state |

The same three YubiKeys can hold the root key and the `targets`, `packages` and `models` keys, each in its own PIV slot. ADR 0008 asks only that root needs 2 of 3 and that one key stays off-site.

**A model's chunk list** (P5 tasks 6 and 9). Beside each `<id>.gguf`, the `models` role signs the target `<sha256 of the GGUF>.chunks`, in lower-case hex. It has the list's length and SHA-256, and no `custom` fields. The GGUF's bytes are never in the repository, but the list's are. With consistent snapshots, Pages serves a list at:

```text
https://<Pages host>/targets/<sha256 of the list>.<sha256 of the GGUF>.chunks
```

The list's SHA-256 is the one the `models` role signs. `kaituf` verifies only metadata and forms no target URL, so `updated` builds this URL itself. It holds the bytes to the signed length and SHA-256, then to the model: the same blob SHA-256 and size, and 2 MiB chunks. The lists come from `release.yml`'s `models-chunks-<version>` artifact, which checks them against the pinned GGUFs, and are signed with the models in the same event.

## Before you start: two decisions

1. **The online key needs a cloud KMS.** tuf-on-ci 0.21.0 signs snapshot and timestamp with a key in Google Cloud KMS, AWS KMS or Azure Key Vault, or with Sigstore. It has no file keys. The P4 plan's default for question 3 (Ed25519 file keys in a protected GitHub environment) therefore does not work with tuf-on-ci. Sigstore needs no account, but it is experimental, and `kaituf` checks only Ed25519 and ECDSA P-256 signatures, so devices could not verify it. **Pick one KMS account.** The key should be ECDSA P-256 with SHA-256, which `kaituf` checks; on Google Cloud that is `EC_SIGN_P256_SHA256`.
2. **Where the image files live (unverified).** tuf-on-ci's documents do not say where target files are hosted. A slot image is about 700 MB, over git's 100 MB file limit. ADR 0008 §7 puts images on the repository's Releases, named `<sha256>.<name>`, with the metadata on Pages. Check in step 0 that tuf-on-ci can sign a target whose file it does not host. If it cannot, `release.yml` adds the target's length and hashes to the `targets` metadata in the signing event, and uploads the file to Releases itself.

## 0. A dry run (recommended)

Do steps 2-6 once on a throwaway repository made from the same template with a spare YubiKey or two, then delete it. Check three things: the signing event, the first publish, and a release's target signed and fetched. `kaituf`'s client and python-tuf's both read Pages the way devices will.

## 1. The repository

`dane1599d/kernel-ai-updates` exists (public). It was made on 2026-10-28 from tuf-on-ci's own template (`theupdateframework/tuf-on-ci-template`). Its workflows call tuf-on-ci's actions, pinned to v0.21.0 by commit (`662382a8295b89a9d538390ce8b9bb85b207a93d`):

- `signing-event.yml`: on each push to a `sign/*` branch, it checks the event and asks its signers.
- `create-signing-events.yml`: it opens an event for any offline role near expiry.
- `online-sign.yml`: it signs snapshot and timestamp with the KMS key.
- `publish.yml`: it deploys to Pages from `publish`.
- `test.yml`: it checks the published repository with a TUF client.

The second commit adds this runbook as `docs/root-ceremony.md` and the repository's README. It also turns on `signing-event.yml`'s owner guard (`github.repository_owner == 'dane1599d'`), so forks never run it. The repository holds no key and no signature until the ceremony. Until then the scheduled runs of `online-sign`, `create-signing-events` and `test` fail, because there is no metadata yet, and tuf-on-ci opens an issue for that. Close it after the first publish.

## 2. Repository settings

- **Pages:** Settings → Pages → Source: GitHub Actions.
- **Environments:** Settings → Environments → `github-pages`. Change the deployment branch from `main` to `publish`.
- **Actions:** Settings → Actions → General. Turn on "Allow GitHub Actions to create and approve pull requests", unless you use a GitHub App token, which the template supports through `TUF_ON_CI_APP_ID`.
- **Online key:** add the variables for your KMS:
  - Google Cloud: `GCP_WORKLOAD_IDENTITY_PROVIDER`, `GCP_SERVICE_ACCOUNT`.
  - AWS: `AWS_ROLE_TO_ASSUME`, `AWS_REGION`.
  - Azure: the secrets `AZURE_CLIENT_ID`, `AZURE_TENANT_ID`, `AZURE_SUBSCRIPTION_ID`.
- In `kernel-ai` itself: Settings → Environments → `release`, with you as a required reviewer. `release.yml`'s `publish` job runs only there.

## 3. Each YubiKey (PIV)

You need firmware 5.7 or later, so a later root can add Ed25519 (ADR 0008 §7). Install `pip install yubikey-manager` and `ykcs11` (Yubico's PKCS#11 module; `apt install ykcs11`, or yubico-piv-tool on Windows). Then, on each key:

```
ykman piv reset
ykman piv access change-pin
ykman piv access change-puk
ykman piv access change-management-key --algorithm aes256 --protect
ykman piv keys generate -a eccp256 9c pub.pem
ykman piv certificates generate 9c --subject dane1599d pub.pem
```

Slot 9c is the PIV signature slot, and `tuf-on-ci-delegate sign/init` needs ECC P-256. Write the PIN and PUK down and keep them apart from the keys. Label each key 1, 2 and 3, and take key 3 off-site once the ceremony is done.

## 4. The signing tool

```
pip install tuf-on-ci-sign
```

In your clone of `kernel-ai-updates`, create `.tuf-on-ci-sign.ini`:

```ini
[settings]
user-name = @dane1599d
pull-remote = origin
push-remote = origin
```

On WSL, attach the YubiKey to Linux with `usbipd-win` first.

## 5. The ceremony

1. `tuf-on-ci-delegate sign/init`, with key 1 inserted. It asks for the repository's details:
   - **Root:** signers `@dane1599d`, threshold 1 for now, expiry 365 days.
   - **Targets:** you, 1 of 1 for now.
   - **Online roles:** your KMS key, snapshot 30 days and timestamp 7 days. These are ADR 0008 §7's figures, not tuf-on-ci's defaults.

   It signs and pushes the first metadata to a signing event branch. Merge the event's pull request: CI makes the first snapshot and timestamp and publishes to Pages.
2. **Root's other two keys:** `tuf-on-ci-delegate sign/root-2of3 root`. Raise the signers to three and the threshold to 2. Each new key is accepted by signing the event: insert key 2, run `tuf-on-ci-sign sign/root-2of3`, then do the same with key 3. Then merge the event's pull request.
3. **Targets' second key:** `tuf-on-ci-delegate sign/targets-1of2 targets`. Use two signers, with threshold 1.
4. **The two delegations,** each 1 of 2, with expiry 365 days:
   - `tuf-on-ci-delegate sign/packages packages`, paths `*.kpkg`, terminating;
   - `tuf-on-ci-delegate sign/models models`, paths `*.gguf` and `*.chunks`, terminating, as xtask's test repositories delegate it.
5. **Check the result:** a fresh python-tuf or `kaituf` client, given only `1.root.json`, refreshes from Pages and lands on the newest root, which needs 2 of 3 signatures.
6. **Close the issue** tuf-on-ci opened for the scheduled runs that failed before there was metadata. From now on, those runs keep snapshot and timestamp fresh, open re-signing events and test what Pages serves.

## 6. Wire `kernel-ai` to it

1. Commit the new `2.root.json` from `kernel-ai-updates` to `kernel-ai` beside `release/pkgrepo/metadata/1.root.json`, which stays as it is (see "Moving to the YubiKeys" above). `cargo xtask release-image` already bundles `release/pkgrepo/` as `pkgrepo/` and its root as each slot's `kai.root.json`, for release builds only. CI keeps signing with the throwaway test root (P4 plan section 4, item 3).
2. In `release.yml`, replace the two stubs (task 18's PR in lane U):
   - **YubiKey signing event:** on a tag, open a signing event in `kernel-ai-updates` that adds the release's targets. Use a token or App with `contents: write` on that repository only, kept in the `release` environment.
   - **Publish:** after your touch, tuf-on-ci signs snapshot and timestamp and publishes.
3. Tag `v0.1.0-rc.1`. Approve the `release` environment, touch your YubiKey for the targets event, and publish the draft GitHub Release by hand (ADR 0009 D7). A CI VM then takes the release from an update disk (`release-dry`'s `vm` job) with the real root in place of the test one.

## If a key is lost

- **One root key:** the other two (2 of 3) sign a new root that replaces it. Do it soon, while two remain.
- **A `targets`, `packages` or `models` key:** the second key keeps signing. Root replaces the lost one in a `tuf-on-ci-delegate` event.
- **The online key:** root (2 of 3) names a new KMS key for snapshot and timestamp. Until it does, devices keep the last timestamp and refuse anything newer.
- **Two root keys:** no one can rotate the root. Devices then need a release carrying a new `root.json` installed by hand from the USB stick (ADR 0008 §9). This is why one key stays off-site.
