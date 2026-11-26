# Spike G13: hayro 0.9 renders PDFs

P6 groundwork G13 (plan M-3, the PDF viewer). Can hayro 0.9.0 (Apache-2.0
OR MIT) render the PDFs a person opens, and how fast, how big, and how close
to poppler's `pdftoppm`?

**This is a spike.** The `spike/hayro-pdf` branch is never merged (ADR 0000:43),
nothing here ships, and it runs only on a GitHub runner
(`.github/workflows/spike-hayro-pdf.yml`). The PDFs are made by
`make_corpus.py` or fetched from pdf.js's test corpus at a pinned commit, and
are never committed.

## What runs
- `src/bin/hayro-render.rs` renders up to 50 pages of a PDF at 150 dpi on
  white, one PNG a page. A page that panics is reported, not fatal.
  Non-embedded CJK fonts fall back to Noto Sans CJK, as poppler's fontconfig
  does; other fonts fall back to hayro's embedded standard 14.
- `pdftoppm -r 150 -l 50 -png` (poppler, Ubuntu 24.04) renders the same pages.
- Both run under `/usr/bin/time` (wall time, peak RSS) and `timeout 300`.
- `src/bin/pdfdiff.rs` compares each page both rendered. A pixel differs when
  any channel is more than 48 apart; MAE is the mean absolute difference per
  channel (0-255). Anti-aliasing and hinting differ between any two renderers,
  so a few percent is expected on text.
- `drive.py` runs everything and writes the table below.

## The corpus (20 PDFs)
| kind | made here | from pdf.js |
|---|---|---|
| text-heavy | 30 pages of standard-14 text | tracemonkey (a 14-page paper) |
| scanned | an A4 grey JPEG at 300 dpi | a CCITT fax page |
| forms | AcroForm text, check, radio and choice fields | text widgets |
| CJK | non-embedded CID fonts in all four collections | vertical writing |
| vector | 2,000 curves with alpha and dashes | transparency groups; shadings |
| encrypted | AES-256 and RC4-128 with an empty user password; one with a user password | |
| broken | cut at 60%; `startxref` pointing nowhere; 400 bytes changed | |
| huge | 500 pages; an 8000 × 8000 JPEG on one page | TAMReview (magazine pages) |

## Figures
Run pending.
