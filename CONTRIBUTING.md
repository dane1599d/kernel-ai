# Contributing to Kernel AI

Thank you for helping. Kernel AI is pre-release (`0.1.0-dev`), so please open an issue to talk about a change before you start a large one.

## Licence: what you contribute is MPL-2.0

Kernel AI is under the [Mozilla Public License 2.0](LICENSE) (ADR [0009](docs/adr/0009-licence.md) D1). A contribution is licensed under the same terms as the file it changes ("inbound equals outbound"), with no contributor licence agreement:

- most of the tree: `MPL-2.0`;
- `kabi/`, `compsdk/` and the interface files under `wit/`: `MIT OR Apache-2.0`, because programs written for Kernel AI compile them in;
- `llmproto/src/examples.rs`, the worked examples the AI copies into what it writes for you: `0BSD`;
- a file copied from elsewhere keeps its upstream licence, recorded in its header and in the commit (ADR 0009 D4).

The patent licence comes from the licence itself (MPL-2.0 section 2.1(b); Apache-2.0 section 3). Everything committed before these terms were adopted is the founder's, and is under the same licences.

## Sign off every commit (DCO 1.1)

Each commit's message ends with a sign-off line, which certifies the [Developer Certificate of Origin 1.1](https://developercertificate.org/):

```
Signed-off-by: Your Name <you@example.org>
```

`git commit -s` adds it. The name and email must be the commit's author, and a real identity you can be reached at. The `dco` check on every pull request refuses a commit without a matching sign-off, unless its author is listed in [MAINTAINERS](MAINTAINERS).

## AI-assisted work

Every commit has a human author, who answers for the code (ADR 0009 D3). If an AI tool helped, say so with a `Co-Authored-By: <model>` or `Assisted-by: <model>` trailer. AI tools never add `Signed-off-by:`, and neither trailer counts as a sign-off: only you can certify the DCO.

## Dependencies

A new crate under MIT, Apache-2.0, BSD, ISC, Zlib or the other free licences in `deny.toml` may be added without asking; list it with its licence in the pull request. Anything else (MPL-2.0 dependencies, GPL, LGPL, AGPL, unlicensed or custom terms) needs the maintainer's approval first (ADR 0009 D4). `cargo xtask licences check` and `cargo xtask notices --check` must pass.

## The name

The licences grant no rights in the name "Kernel AI" (MPL-2.0 section 2.3). An unmodified release may be called Kernel AI; a modified build should be renamed, or say it is based on Kernel AI.
