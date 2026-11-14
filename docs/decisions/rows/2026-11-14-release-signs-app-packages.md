needed-by: rc.2 (founder test 2, test 5 option (b); the manager's call with two conditions; A-core's item 3 needs it; AP-2 builds)
decision: a release signs its built-in apps as packages, packed on two CI runners that must agree, and release-image refuses a tag whose apps are not the signed bytes
---
**Why.** The shell learns an app's actions from its installed package: it asks `pkg` for `actions <package>`, and `pkg` reads only the image's `pkgrepo/`. A release whose `packages` role was empty (#863) would know no app's actions until the app ran, the bug the founder hit in VirtualBox.

**How.**
- `release-repo prepare` builds the components as `release-image` does, packs the built-in apps (`pkgrepo::pack_all`), and signs them in the `packages` role. It writes their bytes and a `SHA256SUMS` beside the role files.
- `release-prepare.yml` runs it on two runners and goes on only if both packed byte-identical packages. Its summary lists the SHA-256s, and it uploads runner a's files for the founder to sign.
- The sign script prints each package's name and SHA-256 before any passphrase, and the founder holds them to that summary.

**The gates.**
- `assemble` holds the role to the prepared packages.
- `release-image` packs the apps from its own build. It refuses unless the committed role signs exactly those, each of its length and SHA-256; a difference names the app and both hashes. It never packs again to fit. It then bundles the bytes at `pkgrepo/targets/<name>-<version>.kpkg`.
- `ci.yml`'s `release-repo check` holds the role by shape and hash only, since it builds no components.

**The cost.** Every release signs the packages again, and any app change means a new signing.
