# Spike S14-4 `web-compare`

ADR 0014 (in PR #1268) asks whether a site list loaded live in Firefox can serve as
the reference that the browser's site gates measure against (plan B-15). This
branch, `spike/web-compare`, is **never merged**.

- `sites.txt`: plan p6-parity §4.4's tier 1, one page per site.
- `capture.py`: one pass in Firefox headless at 1280x800.
  - Each page's text is kept in reading order (`document.body.innerText`), with
    one screenshot of the first screen.
  - The profile is fresh each run, and every cookie is refused.
  - Read-only: no logins, form posts, clicks or scrolling.
  - 5 s pass between sites.
- `compare.py`: two runs line by line. A site passes when its text is the same
  on both. For one that differs, it reports the changed lines and the words the
  runs share, plus a diff.
- `.github/workflows/web-compare.yml`: two runs one after the other, on a GitHub
  runner only. It runs by hand, or on a push to this branch. The texts,
  screenshots, diffs and report are a 30-day artifact, never committed.

Bar (ADR 0014): the harness gives the same text on two runs. If it can't, the
site list's gate becomes text-only.
