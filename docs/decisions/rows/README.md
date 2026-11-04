# Decision rows, one file each

A new decision is a file here, not a row appended to the table in [decisions.md](../../decisions.md). Every pull request used to append to the end of that one table, so GitHub found nearly all of them in conflict with each other and with main, ran no CI on them, and the lanes kept merging main in. Two pull requests that add two files never touch the same line.

```bash
cargo xtask decisions new my-slug     # docs/decisions/rows/<today>-my-slug.md, from the template
cargo xtask decisions check           # what CI runs
```

A row file is two fields, a `---` line, then the detail:

```text
needed-by: P5, built 2026-11-04 (AP-1; K reviews)
decision: What was decided, in a sentence
---
The detail: what was decided and why. Paragraphs are joined into one table cell.
Write \| for a literal pipe.
```

- The name is `YYYY-MM-DD-slug.md`: the day you wrote it, and a slug of `a-z`, `0-9` and hyphens. Rows sort by name.
- `needed-by:` and `decision:` may run on to more lines; either order is fine. These are the table's first two columns, and the detail is the third.
- Save with LF line endings, and end the file with a newline.
- Edit your own row as often as you like. A row someone else wrote is theirs: change it by a new row that says what changed, or with their review.
- Do not edit `ROWS.md` or the rows of `decisions.md`. `ROWS.md` is the table of every row file, written by `cargo xtask decisions write` in the batch that merges the rows; a pull request that wrote it would conflict with every other one that adds a row. The lint refuses a row in `ROWS.md` that no file here has.
- Rows already in `decisions.md` stay where they are, and a correction to one of them is an ordinary edit there. Only a *new* row goes in a file; `docs/decisions/LEGACY` says how many rows `decisions.md` may have, and the lint warns (later: fails) when it has more.
