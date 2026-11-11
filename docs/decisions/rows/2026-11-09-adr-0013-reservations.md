needed-by: Agents, built 2026-11-09 (lane E; the manager asked for ADR 0013's pre-tag reservations as one small refusing change; cherry-picked to release/v0.1)
decision: Before the v0.1 tag, ADR 0013 takes three row-group names, the package kind `agent` and two volume names, by refusing them
---
Each is a refusal, not a feature, so the tag can carry them and the agents track can build on them after it (ADR 0013, "Phases that depend on it").

- **Row groups.** `agent`, `goal` and `search` join `agentabi::RESERVED_GROUPS` (`memory` is there already, as the `memory` row's group). No package may take one of these names, so a package `search` exporting `files` can never make `search_files`, and the rows the track adds later (`agent_*`, `memory_*`, `search_files`, `goal_*`) cannot collide with a package's actions. The built-in agents' own names (`kai`, `goals`, `index`, `maint`) are not groups.
- **Package kind.** `kind agent` is refused by name in `kaipkg` ("reserved in format 1 (ADR 0013)"), as `kind tool` is, so v0.1's `pkg` refuses an agent package cleanly rather than as an unknown kind. The hostile-header test covers it.
- **Volumes.** `kaifs::vol::RESERVED_VOLUMES` names `memory` and `index`: `mkfs` makes neither, and `decode_volume` refuses an entry that names one (`Unsupported`), so an image of format 1 cannot hold a volume the track means differently. Adding them is a format change (ADR 0004's compat bits), made when the volumes land.

No WIT, no `actlog`, no running code changes: a package named like any of these now fails `valid_name`, and nothing in the v0.1 catalogue is.
