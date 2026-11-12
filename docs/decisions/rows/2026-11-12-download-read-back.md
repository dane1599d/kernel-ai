needed-by: P5 task 6 (lane N-t; the manager's call of 2026-11-12 on #770's finding)
decision: a download whose partial other holders could write is read back and checked in full before it moves in
---
**The finding (#770):** `kainet::download` hashed each chunk as fetched, wrote it, and moved the target in without reading it back. Another holder of the partial's folder could change the bytes in between, at the same length, and the change would move in as verified.

**The manager's first choice** was a `.partial` folder only the downloader's grant can write, moved out at finish. `fs` cannot express that for a shared destination today:
- Access goes by `Dir` handle. Any holder of `/home/user` opens any path beneath it, and no folder beneath a `Dir` can be fenced off.
- `Rename` is served within one `Dir`. The protocol names a destination `Dir` moved with the request, but `fs` never receives a moved handle: its `Call` carries the caller, the badge, the rights and the bytes.
- A folder of one program's own, like `/system/apps/<name>` through `dir=apps`, is on the `system` volume, and a user's targets are on `home`.

**So the fallback:**
- **The read-back:** `download` reads every chunk back and checks it, and the whole target, just before `Partial::finish`. A chunk that no longer matches sets the mark back to it and refuses the move (`CHANGED_ON_DISK`), so a resume fetches it again.
- **The exemption:** a partial that only its downloader can change says so with `Partial::held_alone`. It skips the pass:
  - `updated`'s idle slot: its `finish` hashes the whole slot itself;
  - the model store's `.partial/`: only `dir=models` opens it, and the sealed copy into the store hashes it again (#824).
- **The verified-hash rule is unchanged:** each chunk is checked before it is written, and the whole target before it moves in.

**Cost:** one read of the target at kaifs's read speed. #810's split measured about 29 MB/s through 7 KiB `fs` messages, about 2.2 s for 64 MiB (est.).
