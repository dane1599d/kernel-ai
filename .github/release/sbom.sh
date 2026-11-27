#!/usr/bin/env bash
# The release's SBOM (P8 row 5): cargo-cyclonedx 0.5.9 writes one CycloneDX 1.5
# JSON file per workspace package, and this merges them into one file, `$1`,
# that is the same bytes on any runner for the same commit: no serial number or
# timestamp, components sorted, no checkout path. The per-package files are
# removed, so the tree is as it was. Run from the repository root with
# cargo-cyclonedx installed.
set -euo pipefail
out=${1:?usage: sbom.sh OUT.cdx.json}
version=${CYCLONEDX_VERSION:?set CYCLONEDX_VERSION}

cargo cyclonedx --format json --spec-version 1.5 --all-features --target all
boms=()
while IFS= read -r file; do boms+=("$file"); done < <(
  find . \( -path ./target -o -path ./.git \) -prune -o -name '*.cdx.json' -print | sort)
echo "${#boms[@]} per-package SBOMs"
test "${#boms[@]}" -gt 0
# Components are unique by purl (name, version, source); our own crates are
# components too, so the whole workspace shows. A path crate's bom-ref holds
# the checkout's path: it goes, the purl names the crate.
jq -s --arg ver "$version" '
  {
    bomFormat: "CycloneDX",
    specVersion: "1.5",
    version: 1,
    metadata: {
      tools: [{ vendor: "CycloneDX", name: "cargo-cyclonedx", version: $ver }],
      component: { type: "application", name: "kernel-ai", "bom-ref": "kernel-ai" }
    },
    components: (
      [ .[] | ((.metadata.component // empty), (.components // [])[]) ]
      | map(del(.components))
      | map(if (."bom-ref" // "" | startswith("path+file://")) then del(."bom-ref") else . end)
      | unique_by(.purl // (.name + "@" + .version))
      | sort_by(.name, .version)
    )
  }' "${boms[@]}" > "$out"
rm -f -- "${boms[@]}"
jq -r '"\(.components | length) components"' "$out"
