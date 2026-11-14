# Release signing: the `kernel-ai-updates` repository and the root ceremony

P4 task 18 (ADR 0008 §7). This is the founder's runbook. It sets up the TUF repository that signs v0.1 and every later release, and it holds the keys that every Kernel AI device trusts. Nothing here runs from a Claude session: the repository, its settings, the YubiKeys and every signature are yours. Steps marked **(unverified)** come from tuf-on-ci 0.21.0's documents, which say no more on them. Try them first on a throwaway repository (step 0).

Until the YubiKeys, releases are signed with two key files instead: the next section. The sections after it describe the YubiKey setup that replaces them.

## Interim: key files (until the YubiKeys)

You chose this on 2026-11-14. Two Ed25519 key files, each encrypted with its own passphrase, sign the image repository a release carries, `release/pkgrepo/`:

| File | Signs | Threshold |
|---|---|---|
| `root.key` | root (version 1, once) | 1 of 1 |
| `release.key` | targets, packages (empty), models, snapshot, timestamp | 1 of 1 each |

**What it gives a release.** The image carries `release/pkgrepo/` as `pkgrepo/`. Its `packages` role signs the built-in apps (Files, Notes, Terminal and the rest) as packages, which the image carries beside it. The shell learns each app's actions from its installed package, so Kai can call an app by name before it has run. Its `models` role names every model in `xtask/models.json`, so Models imports them, and it is what the download offers come from. Its `1.root.json` also ships as `kai.root.json`, the root that update disks and the online repository are checked against. One root covers both, and the YubiKey root later chains from it.

**What it does not do.** The kernel boots any model disk attached as `kai-model`, or a disk that starts with the GGUF magic, without a signature. That is how `cargo xtask vbox` VMs get their model, and attaching `kai-model.vmdk` to a release VM works without any key. On VirtualBox no disk can be the import disk (`kai-import`), so the role matters there only for downloads.

**Where to sign.** Sign on a machine no Claude session runs on: your Mac with Homebrew's `openssl@3`, or a live-USB Linux, such as Ubuntu's "Try Ubuntu" session booted from a stick. This PC is not that machine. Anything that runs as your Windows user, every Claude session included, can:

- read a stick plugged into this PC;
- run commands in any WSL distro as root without a password;
- change a shell's startup files;
- swap the script between its check and its run.

A WSL distro of its own and `env -i` do not stop that. WSL2 also does not start on this PC at present.

- **Fallback, only if neither is at hand:**
  1. Close every Claude session and Claude Desktop, then restart Windows, so that nothing they started is still running.
  2. Open a fresh Git Bash and sign there, with Git for Windows' `/usr/bin/openssl`.
  3. Check the script against the SHA-256 on your paper, not against one any program shows you.

  On this path you trust Windows, Git for Windows, and that nothing harmful already runs as your user: a keylogger planted earlier would get both passphrases. Prefer the Mac.

**Rules that keep it safe.**

- Two files on one machine under one person add nothing, so root is 1 of 1. The protection is the passphrase, a key stick plugged only into the signing machine, and an encrypted copy kept in another building.
- The key stick never goes into this PC. Files move between this PC and the signing machine on a second stick, the transfer stick. What it carries is public.
- The sign script runs from the key stick, never from a checkout, because a session could change a checkout between your review and your run.
  - Check its SHA-256 before each run against the one you wrote down.
  - The script at the commit that added it has this SHA-256: `7845ec9070ffa59d8b54a2daf280101d00abf6847a08926c04d1032bff90c117`.
  - A test in `xtask/src/release_repo/tests.rs` holds this page to the script, so a change to either shows in review.
  - It runs in macOS's bash 3.2 and with Linux's or Git for Windows' bash. It uses the first OpenSSL 3 it finds at `/usr/bin/openssl` or Homebrew's `openssl@3`.
- Root files are frozen once committed. `cargo xtask release-repo` reuses `1.root.json` and never signs root again. CI fails a pull request or a push to main that changes or removes any committed `*.root.json`, and a tag whose root files differ from any earlier release's. Two different "version 1" roots would split devices, because `updated` keeps a cached root of the same version.
- **Both keys are pinned outside the repository.** In `kernel-ai`'s protected `release` environment, you set `RELEASE_ROOT_KEY` and `RELEASE_KEY` to the two keys' 64 hex digits (setup step 7).
  - The `publish` job refuses a release unless root names the root key and every other role names the release key.
  - Once the pins are set, it also refuses a release with no root.
  - Without the pins, a pull request could bring keys of its own and every other check would pass. That includes just a `release.pub` of its own, which your root would then trust.
- **Your check before publishing, which no session can change.** The draft release's notes list both keys' digits under "Signing keys". Before you press **Publish release**, hold them against your paper. Sessions push to `main` as your account, and that account can also rewrite `RELEASE_ROOT_KEY` and `RELEASE_KEY`, so the paper has the last word.
- **CODEOWNERS** names you for `release/`, `scripts/release-sign.sh`, `.github/workflows/` and itself.
  - Requiring that review is a ruleset: Settings → Rules → Rulesets → a branch ruleset on `main` with "Require review from Code Owners".
  - A ruleset cannot tell sessions from you, because they push as your account (`batch-ff` pushes to `main` as `dane1599d`). A bypass for you lets batches skip the review too. Without one, every batch that touches those paths stops.
  - The stronger fix is your choice, for later: give sessions a GitHub identity of their own, a machine user or a GitHub App, without admin rights. Rulesets, environments and their variables would then answer to you alone.
- **The sticks.** Format all three as exFAT, which the Mac, Linux and Windows all read and write: the key stick (`KAIKEYS`), its off-site copy (`KAIKEYS2`) and the transfer stick (`TRANSFER`).
- Never put a key file under the OneDrive Desktop folder, where the main checkout lives. Never type a passphrase into a Claude chat, or into a terminal a session controls.

### One-time setup (about 30 minutes)

Every command runs in one shell, with three names set for your machine:

| Name | Apple-silicon Mac | Intel Mac | Live-USB Linux |
|---|---|---|---|
| `O`, OpenSSL 3 | `/opt/homebrew/opt/openssl@3/bin/openssl` | `/usr/local/opt/openssl@3/bin/openssl` | `/usr/bin/openssl` |
| `K`, the key folder | `/Volumes/KAIKEYS/kai-keys` | `/Volumes/KAIKEYS/kai-keys` | `/media/<user>/KAIKEYS/kai-keys` |
| `T`, the transfer stick | `/Volumes/TRANSFER` | `/Volumes/TRANSFER` | `/media/<user>/TRANSFER` |

On the fallback path, in Git Bash on this PC, `O` is `/usr/bin/openssl`, and the sticks are under their drive letters, as `/f/kai-keys`.

1. Format the three sticks as exFAT, named `KAIKEYS`, `KAIKEYS2` and `TRANSFER` (on the Mac: Disk Utility → Erase → ExFAT).
   - On a Mac, install Homebrew if you do not have it (brew.sh), then run `brew install openssl@3`.
   - On a live-USB Linux, run `sudo apt update && sudo apt install -y curl ca-certificates openssl`.
2. Open a terminal and run `env -i /bin/bash --noprofile --norc`. Set the three names from the table, for example `O=/opt/homebrew/opt/openssl@3/bin/openssl K=/Volumes/KAIKEYS/kai-keys T=/Volumes/TRANSFER`. Then run `$O version`: it must start with `OpenSSL 3`. Do every step below in this shell.
3. Plug in the key stick, then run `mkdir -p "$K" && cd "$K"`.
4. Make the root key: `$O genpkey -algorithm ed25519 | $O pkcs8 -topk8 -v2 aes-256-cbc -scrypt -scrypt_N 16384 -scrypt_r 8 -scrypt_p 64 -out root.key`.
   - It asks for a new passphrase twice. Use six or more words picked by dice (Diceware) or by your password manager's passphrase generator, not words you choose.
   - Why these settings: OpenSSL cannot decrypt a key whose scrypt needs more than 32 MiB, so N cannot go higher than 16384. p=64 makes each guess 64 times as costly as the default instead, about 2 seconds a use.
5. Make the release key the same way, with a different passphrase: `$O genpkey -algorithm ed25519 | $O pkcs8 -topk8 -v2 aes-256-cbc -scrypt -scrypt_N 16384 -scrypt_r 8 -scrypt_p 64 -out release.key`.
6. Run `head -1 root.key release.key`. Both must say `-----BEGIN ENCRYPTED PRIVATE KEY-----`. If either does not, delete it and repeat its step. Then write the public halves: `$O pkey -in root.key -pubout -out root.pub`, then `$O pkey -in release.key -pubout -out release.pub`. Each asks for its key's passphrase. Leave both `.pub` files in `$K`: the script shows them before every signing.
7. Pin both keys:
   - Run `for k in root release; do echo "$k: $($O pkey -pubin -in $k.pub -outform DER | tail -c 32 | od -An -v -tx1 | tr -d ' \n')"; done`. It prints two lines of 64 hex digits. Write both on the paper you keep with the passphrases: these are "the digits on your paper".
   - In a browser, open `kernel-ai` on GitHub, then Settings → Environments → `release` → Environment variables. Add `RELEASE_ROOT_KEY` with root's digits, and `RELEASE_KEY` with release's. Do this yourself, not through a session.
8. Get the sign script at the commit the session names (this page's pull request, once merged): `/usr/bin/curl -fsSLo release-sign.sh https://raw.githubusercontent.com/dane1599d/kernel-ai/<commit>/scripts/release-sign.sh`.
   - Read it: it is under 90 lines.
   - Run `$O dgst -sha256 release-sign.sh`. It must match the SHA-256 above.
   - Write it on the paper too.
9. Save both passphrases in your password manager, and on the paper, kept away from the sticks.
10. Plug in `KAIKEYS2` and copy everything in `$K` to a `kai-keys` folder on it. Check that each key copy opens: `$O pkey -in <copy> -noout`, with its passphrase. Keep that stick in another building, and check it again once a year: sticks fail without warning.
11. Plug in the transfer stick and run `cp root.pub release.pub "$T"/`. Give the stick to the session, which commits them as `release/keys/`.
12. Run `cd /` and `exit`, then eject every stick.

### Each release

Signing takes two rounds, because the timestamp holds the hash of the signed snapshot.

- **Round 1** signs exactly `targets.canon`, `packages.canon`, `models.canon` and `snapshot.canon`, and `root.canon` too at the first release only.
- **Round 2** signs `timestamp.canon` alone.

The script refuses a folder that holds anything else to sign, or misses one of these.

**A release number is signed once.** If anything goes wrong after round 1 is signed (a refusal, a changed file, a round done again), don't sign that number again: the session raises `release/seq`, and you start again at step 1. The snapshot names each role by its version alone, so files from two signings of one number could be mixed.

**Every release signs the packages again**, along with targets and the rest. You sign each built-in app's exact bytes, and any change to an app changes them. A tag whose apps are not the signed bytes is refused (`release-image` names the app and both hashes), and you sign again from step 1.

1. The session runs the "Release prepare" workflow on GitHub on the commit to be released, and tells you its branch and commit.
   - Two runners build the apps, and it goes on only when both packed byte-identical packages. Its summary page lists each model and chunk list with its SHA-256 and length (a model with its name, licence and source too), and each package with its SHA-256, grants, actions and whether it is yanked.
   - Open that page yourself, not from a link: on GitHub, **Actions**, then **Release prepare**, then the run. Its summary names a branch and a commit. The branch must be `main` or `release/v…`, and the commit the one the session told you. If either is not, stop.
   - With the transfer stick in this PC, the session downloads the workflow's files (`release-prepare-<commit>`) to `<transfer stick>\kai-release\<version>`. It pastes the `sha256` line `prepare` printed for each file.
   - Eject the transfer stick.
2. On the signing machine, plug in the key stick and the transfer stick. Run `env -i /bin/bash --noprofile --norc`, and set `O`, `K` and `T` as in setup step 2.
3. Run `$O dgst -sha256 "$K/release-sign.sh"`. It must match the SHA-256 on your paper. If it does not, stop.
4. Run `/bin/bash --noprofile --norc "$K/release-sign.sh" "$K" "$T/kai-release/<version>"`. Before any passphrase, it prints `round 1 of release <n>: signs …` (in step 6, `round 2 of release <n>: signs timestamp`), then `root.key is …` and `release.key is …`, then for each file the keys it names.
   - The round must be the one you are on, and `<n>` the release number the session told you. If not, press Ctrl-C and stop.
   - Every one of those numbers must be one of the two on your paper. If any is not, press Ctrl-C and stop.
   - At the first release, it holds `root.canon` to version 1 of the root xtask writes for your two keys: root.key for root, release.key for targets, snapshot and timestamp, each key named by its own id. It refuses any other root, and any root on a later release.
   - In round 2, it first checks that `snapshot.sig` is release.key's signature of `snapshot.canon`, and prints `round 1 signed snapshot.canon as <n> bytes, sha256 <hash>`: the timestamp must name exactly that.
   - Then, in round 1, it prints `models.canon signs <file>, sha256 <hash>, length <n>` for each model and chunk list (with `name`, `licence` and `source` for a model), and `packages.canon signs <name>, sha256 <hash>, grants [...], actions [...], yanked false` for each package. Each whole line must be on the Release prepare summary page from step 1. Today every built-in app says `grants []`. If a line is not there, press Ctrl-C and stop.
   - Last, it prints `<file>.canon expires <date>, in <n> days` for each file: 365 days for root, targets, packages and models, 30 for snapshot and 7 for timestamp. It refuses one not within a week of that, so this machine's clock must be right.
   - If the script stops with "is not one line of printable ASCII", "is not what xtask writes", "not one round's files", "is not a snapshot", "a key that is not lower-case hex Ed25519", "are one key", "is not release.key's signature" or "not within a week of", stop too: someone changed the folder, or this machine's clock is wrong.
   - Then type each passphrase it asks for. Root's is asked only for the first release, the one that signs root. If it asks for root's on any later release, press Ctrl-C and stop: root is frozen, so a request for it means someone changed the folder.
   - For each file it prints `signed <file> with <key>: sha256 <hash>`. Each hash must be the one the session pasted for that file. If one differs, the folder changed after `prepare`: tell the session, and nothing goes further.
5. Run `exit`, eject both sticks, take the transfer stick back to this PC, and tell the session "signed".
6. The session runs `cargo xtask release-repo prepare-timestamp` and pastes its one `sha256` line. Repeat steps 2 to 5. This time it asks only for the release passphrase.
7. The session runs `cargo xtask release-repo assemble`, which checks:
   - every signature;
   - the repository, as a device checks it;
   - that every signed byte is what `prepare` writes for `release/keys/` and `xtask/models.json`, and that the packages role signs exactly the packages from step 1, each packed again from the app inside it, its grants and actions included;
   - that each expiry is within a week of its full period.

   Then it commits `release/pkgrepo/` and opens a pull request. CI checks the same on clean runners, on that pull request and on every later one. The release dry run boots the image and imports qwen3-0.6b under the role you signed.
8. When the tag's release workflow has drafted the release, open the draft on GitHub. Its notes end with "Signing keys", root's digits and release's. They must be the digits on your paper. Only then press **Publish release**. If they differ, delete the draft and find out why before anything is signed or published again.

The session never needs the key stick or a passphrase. A change to `xtask/models.json` or its chunk lists fails CI until the repository is signed again this way. A change to an app fails the tag's build until its package is signed again. Allow a week from step 1 to step 7: past that, `assemble` refuses the expiries, and you start again at step 1.

**What is still trusted:** you sign the bytes the session prepared.
- The checks catch a repository that is not what xtask writes, a key that is not yours, and a folder changed after `prepare`.
- They cannot catch a session that prepares other bytes naming only your keys, lies about their hashes, and keeps the signatures instead of committing them.
- Your checks against that are:
  - the key numbers the script shows (step 4);
  - the rule on root's passphrase;
  - the digits in the draft (step 8);
  - signing only when a session asks for a release you expect.
- The image's repository (`pkgrepo/`) and the online repository trust the same root and `release.key`: what you sign with it is good for both.

### Expiry and online updates

- Model import never checks expiry, so the role in an image keeps working.
- Update disks and the online repository are checked at the real time. A repository signed this way stops being accepted 7 days after its timestamp is signed. Until there is an online key, an online repository signed offline needs a longer timestamp expiry, for example 30 days, re-signed by hand.
- Root expires 365 days after you sign it. After that, update disks and online updates stop until `2.root.json` is published.
- From 60 days before then, `release-repo check` refuses the committed repository, so release builds and CI fail until root version 2 is made: the YubiKey root (below), or a version 2 signed with `root.key`. Plan it for month 10.
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
- **`release.key` and both its copies:** `root.key` signs a root version 2 that names a new release key, and you set `RELEASE_KEY` to its digits.
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
