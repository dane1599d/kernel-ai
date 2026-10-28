# kernel-ai-updates

The update repository for [Kernel AI](https://github.com/dane1599d/kernel-ai): the TUF metadata (TUF 1.0.37) that signs every release, package and model a Kernel AI device installs (ADR 0008 §7, ADR 0011 §3). It is run by [tuf-on-ci](https://github.com/theupdateframework/tuf-on-ci), and the metadata is published on this repository's GitHub Pages. Devices trust it through the `root.json` each release carries, and check everything with `kaituf`.

| Role | Signs | Keys |
|---|---|---|
| `root` | the other roles' keys | 2 of 3 YubiKeys |
| `targets` | OS slot images, recovery images, `SHA256SUMS` | 1 of 2 YubiKeys |
| `packages` | `*.kpkg` | 1 of 2 YubiKeys |
| `models` | `*.gguf` and their chunk lists | 1 of 2 YubiKeys |
| `snapshot`, `timestamp` | the repository's current state | an online key, re-signed by CI |

Releases are made in `kernel-ai` by `.github/workflows/release.yml`, which opens a signing event here for each tag. How the repository and its keys were set up is in `kernel-ai`'s `docs/release/root-ceremony.md`.
