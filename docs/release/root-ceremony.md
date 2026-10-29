# Release signing: the `kernel-ai-updates` repository and the root ceremony

P4 task 18 (ADR 0008 §7). This is the founder's runbook. It sets up the TUF repository that signs v0.1 and every later release, and it holds the keys that every Kernel AI device trusts. Nothing here runs from a Claude session: the repository, its settings, the YubiKeys and every signature are yours. Steps marked **(unverified)** come from tuf-on-ci 0.21.0's documents, which say no more on them. Try them first on a throwaway repository (step 0).

## What you end with

| Role | Keys | Threshold | Expiry | Signs |
|---|---|---|---|---|
| `root` | 3 YubiKeys (PIV, ECC P-256), one kept off-site | 2 of 3 | 365 days, re-signed from 60 days before | the other roles' keys |
| `targets` | 2 YubiKeys | 1 of 2 | 365 days | slot images, R images, `SHA256SUMS`; delegates the two below |
| `packages` | 2 YubiKeys | 1 of 2 | 365 days | `*.kpkg` (ADR 0011), terminating |
| `models` | 2 YubiKeys | 1 of 2 | 365 days | `*.gguf` and their `kai-chunks 1` lists |
| `snapshot`, `timestamp` | one online key in a cloud KMS, reached through GitHub OIDC | 1 | 30 and 7 days, re-signed by CI | the repository's current state |

The same three YubiKeys can hold the root key and the `targets`, `packages` and `models` keys, each in its own PIV slot. ADR 0008 asks only that root needs 2 of 3 and that one key stays off-site.

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
   - `tuf-on-ci-delegate sign/models models`, paths `*.gguf` and `*.kai-chunks`.
5. **Check the result:** a fresh python-tuf or `kaituf` client, given only `1.root.json`, refreshes from Pages and lands on the newest root, which needs 2 of 3 signatures.
6. **Close the issue** tuf-on-ci opened for the scheduled runs that failed before there was metadata. From now on, those runs keep snapshot and timestamp fresh, open re-signing events and test what Pages serves.

## 6. Wire `kernel-ai` to it

1. Commit the newest `root.json` from `kernel-ai-updates` to `kernel-ai` as `release/root.json` (P4 task 18). `cargo xtask release-image` bundles it as each slot's `kai.root.json` for release builds only. CI keeps signing with the throwaway test root (P4 plan section 4, item 3).
2. In `release.yml`, replace the two stubs (task 18's PR in lane U):
   - **YubiKey signing event:** on a tag, open a signing event in `kernel-ai-updates` that adds the release's targets. Use a token or App with `contents: write` on that repository only, kept in the `release` environment.
   - **Publish:** after your touch, tuf-on-ci signs snapshot and timestamp and publishes.
3. Tag `v0.1.0-rc.1`. Approve the `release` environment, touch your YubiKey for the targets event, and publish the draft GitHub Release by hand (ADR 0009 D7). A CI VM then takes the release from an update disk (`release-dry`'s `vm` job) with the real root in place of the test one.

## If a key is lost

- **One root key:** the other two (2 of 3) sign a new root that replaces it. Do it soon, while two remain.
- **A `targets`, `packages` or `models` key:** the second key keeps signing. Root replaces the lost one in a `tuf-on-ci-delegate` event.
- **The online key:** root (2 of 3) names a new KMS key for snapshot and timestamp. Until it does, devices keep the last timestamp and refuse anything newer.
- **Two root keys:** no one can rotate the root. Devices then need a release carrying a new `root.json` installed by hand from the USB stick (ADR 0008 §9). This is why one key stays off-site.
