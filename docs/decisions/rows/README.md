# Decision rows, one file each

A new decision is a file here, not a row appended to the table in [decisions.md](../../decisions.md). Every pull request used to append to the end of that one table, so GitHub found nearly all of them in conflict with each other and with main, ran no CI on them, and the lanes kept merging main in. Two pull requests that add two files never touch the same line.

```bash
cargo xtask decisions new my-slug     # docs/decisions/rows/<today>-my-slug.md, from the template
cargo xtask decisions check           # the format check CI runs on every row file
cargo xtask decisions                 # prints the table of all the row files
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
- Nothing generated is committed: the table of all the rows is `cargo xtask decisions`, printed when you want it. A shared generated file would be one more place every pull request conflicts.
- Rows already in `decisions.md` stay where they are, and a correction to one of them is an ordinary edit there. Only a *new* row goes in a file. On a pull request, CI compares `decisions.md` with its base and warns when it gained a row (it fails once `--deny-new-rows` is added to the CI step).
