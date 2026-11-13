needed-by: P5 task 6 (lane N-t; the manager's call of 2026-11-12 on #770's finding)
decision: a download whose partial other holders could write is checked when it is moved in: read back in full, with its length
---
**The finding (#770):** `kainet::download` hashed each chunk as fetched, wrote it, and moved the target in without reading it back. Another holder of the partial's folder could change the bytes in between, at the same length, and the change would move in as verified.

**The manager's first choice** was a `.partial` folder only the downloader's grant can write, moved out at finish. `fs` cannot express that for a shared destination today:
- Access goes by `Dir` handle. Any holder of `/home/user` opens any path beneath it, and no folder beneath a `Dir` can be fenced off.
- `Rename` is served within one `Dir`. The protocol names a destination `Dir` moved with the request, but `fs` never receives a moved handle: its `Call` carries the caller, the badge, the rights and the bytes.
- A folder of one program's own, like `/system/apps/<name>` through `dir=apps`, is on the `system` volume, and a user's targets are on `home`.

**So the fallback:**
- **The check when it is moved in:** just before `Partial::finish`, `download` reads every chunk back and checks it, then the whole target against its hash, then the partial's length (`Partial::size`) against the target's.
  - A chunk that no longer matches or cannot be read, or a partial found shorter, sets the mark back to that chunk and refuses the move (`CHANGED_ON_DISK`). A resume fetches that chunk and every one after it again.
  - A partial longer than the target, as when a tail was appended, is refused (`LONGER_ON_DISK`). `download` cannot cut it, so the caller removes it and starts over.
  - A shared partial that cannot give its length is never moved in (`NO_SIZE`, the default).
- **It is a check at that moment, not a snapshot.** Another holder can still change the bytes after the check, before or after the rename. A target in a shared folder is only as trustworthy as that folder's other writers.
- **The exemption:** a partial that only its downloader can change says so with `Partial::held_alone`. It skips the pass:
  - `updated`'s idle slot: its `finish` hashes the whole slot itself;
  - the model store's `.partial/`: only `dir=models` opens it, and the sealed copy into the store hashes it again (#824).
- **The verified-hash rule is unchanged:** each chunk is checked before it is written, and the whole target before it moves in.

**Cost:** one read of the target at kaifs's read speed. #810's split measured about 29 MB/s through 7 KiB `fs` messages, about 2.2 s for 64 MiB (est.).
