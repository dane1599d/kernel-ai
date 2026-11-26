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
Run [45728927723](https://github.com/dane1599d/kernel-ai/actions/runs/45728927723)
on 2026-11-26 (UTC), at commit 7fc9cd0d. Ubuntu 24.04 on a GitHub-hosted runner
(4 vCPUs), poppler-utils 24.02, hayro 0.9.0 built `--release`, 150 dpi, at
most 50 pages a PDF. Times are wall clock for the whole process, PNG encoding
included; ms/page is hayro's own render time for a page, without the PNG.
Memory is peak RSS.

| PDF | kind | hayro | pages | hayro s | ms/page | hayro MiB | pdftoppm | pages | pdftoppm s | pdftoppm MiB | compared | size diff | mean % px differing | worst page % | MAE |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| gen-text-30p.pdf | text, 30 pages, standard fonts | ok | 30 | 0.70 | 5 | 39 | ok | 30 | 13.04 | 26 | 30 | 30 | 8.12 | 8.43 | 12.99 |
| gen-scan-300dpi.pdf | scanned, grey JPEG at 300 dpi | ok | 1 | 0.16 | 33 | 43 | ok | 1 | 2.75 | 30 | 1 | 1 | 8.06 | 8.06 | 11.23 |
| gen-form.pdf | form, AcroForm fields | ok | 1 | 0.02 | 6 | 35 | ok | 1 | 0.09 | 26 | 1 | 1 | 0.32 | 0.32 | 0.38 |
| gen-cjk.pdf | CJK, non-embedded CID fonts | ok | 1 | 0.05 | 33 | 40 | ok | 1 | 0.22 | 58 | 1 | 1 | 2.60 | 2.60 | 4.47 |
| gen-vector.pdf | vector, 2,000 curves with alpha | ok | 1 | 1.64 | 1528 | 272 | ok | 1 | 14.71 | 29 | 1 | 1 | 0.15 | 0.15 | 1.97 |
| gen-enc-aes256.pdf | encrypted, AES-256, empty user password | ok | 30 | 0.71 | 5 | 39 | ok | 30 | 12.96 | 26 | 30 | 30 | 8.12 | 8.43 | 12.99 |
| gen-enc-rc4.pdf | encrypted, RC4-128, empty user password | ok | 30 | 0.70 | 5 | 39 | ok | 30 | 13.01 | 26 | 30 | 30 | 8.12 | 8.43 | 12.99 |
| gen-enc-password.pdf | encrypted, a user password (should refuse) | load Decryption(PasswordProtected) | 0 | 0.00 | – | 5 | exit 1 | 0 | 0.02 | 15 | 0 | 0 | – | – | – |
| gen-broken-truncated.pdf | broken, cut at 60% | ok | 30 | 0.45 | 3 | 40 | exit 1 | 0 | 0.01 | 15 | 0 | 0 | – | – | – |
| gen-broken-xref.pdf | broken, startxref points nowhere | ok | 30 | 0.71 | 5 | 40 | ok | 30 | 12.97 | 26 | 30 | 30 | 8.12 | 8.43 | 12.99 |
| gen-broken-bytes.pdf | broken, 400 bytes changed | ok | 30 | 0.34 | 2 | 39 | ok | 30 | 6.31 | 26 | 30 | 30 | 3.29 | 8.43 | 5.27 |
| gen-huge-500p.pdf | huge, 500 pages (first 50 rendered) | ok | 50 | 1.17 | 5 | 42 | ok | 50 | 21.75 | 26 | 50 | 50 | 8.16 | 8.46 | 13.04 |
| gen-huge-image.pdf | huge, an 8000x8000 JPEG on one page | ok | 1 | 0.40 | 247 | 224 | ok | 1 | 1.71 | 43 | 1 | 1 | 4.75 | 4.75 | 4.06 |
| tracemonkey.pdf | pdf.js: text and figures (a paper, 14 pages) | ok | 14 | 0.47 | 10 | 48 | ok | 14 | 7.35 | 28 | 14 | 14 | 5.23 | 6.87 | 6.72 |
| TAMReview.pdf | pdf.js: magazine pages, images | ok | 23 | 0.51 | 6 | 43 | ok | 23 | 7.15 | 28 | 23 | 23 | 4.46 | 6.72 | 5.69 |
| alphatrans.pdf | pdf.js: vector, transparency groups | ok | 1 | 0.02 | 6 | 35 | ok | 1 | 0.21 | 26 | 1 | 1 | 0.37 | 0.37 | 0.70 |
| vertical.pdf | pdf.js: CJK, vertical writing | ok | 3 | 0.01 | 1 | 27 | ok | 3 | 0.04 | 17 | 3 | 3 | 0.19 | 0.27 | 0.20 |
| annotation-text-widget.pdf | pdf.js: form, text widgets | ok | 1 | 0.02 | 8 | 35 | ok | 1 | 0.13 | 24 | 1 | 1 | 1.03 | 1.03 | 1.54 |
| shading_extend.pdf | pdf.js: vector, shadings | ok | 1 | 0.05 | 36 | 37 | ok | 1 | 0.13 | 26 | 1 | 1 | 0.69 | 0.69 | 0.88 |
| ccitt_EndOfBlock_false.pdf | pdf.js: scanned, CCITT fax | ok | 1 | 0.02 | 6 | 34 | ok | 1 | 0.09 | 24 | 1 | 1 | 0.44 | 0.44 | 1.03 |

Rendered by both and compared: 18 of 20 PDFs, 249 pages. Wall time over those: hayro 7.7 s, pdftoppm 114.6 s. Peak memory, the most of any PDF: hayro 272 MiB, pdftoppm 58 MiB.

## What it shows
- **Everything opened that should.** 19 of 20 rendered with no panic and no
  timeout, the encrypted ones included: AES-256 and RC4-128 with an empty
  user password. The one with a user password was refused as
  `Decryption(PasswordProtected)`, as pdftoppm refused it. A viewer would ask
  for the password there.
- **Broken files:** hayro is more forgiving than poppler.
  - hayro rendered the file cut at 60% (all 30 pages); pdftoppm refused it
    ("Couldn't read xref table"). The artifact keeps only first pages, so
    whether the pages past the cut are blank is not checked here.
  - Both rebuilt the file whose `startxref` points nowhere.
  - Both rendered the file with 400 changed bytes. Their pages differ less
    (3.3%), since both drop the same damaged content.
- **Speed:** about 15 times faster than pdftoppm over the 18 PDFs both
  rendered: 7.7 s against 114.6 s for 249 pages. A text page takes about
  5 ms to render. The slowest page is the vector one: 2,000 alpha-blended
  curves take 1.5 s, against pdftoppm's 14.7 s.
- **Memory is the cost.** hayro's peak was 27-48 MiB for ordinary PDFs, but
  272 MiB for the vector page and 224 MiB for the 8000 × 8000 image, against
  pdftoppm's 29 and 43 MiB. The sealed PDF process (M-3) needs a ceiling
  above that, or hayro's tile and image caches bounded.
- **Pixels:**
  - Vector art, transparency, shadings, forms, CJK, CCITT and vertical
    writing match poppler closely: 0.2% to 2.6% of pixels differ.
  - Text in the standard 14 fonts differs on about 8% of pixels (MAE 13),
    tracemonkey's on 5%. That is the fonts, not the layout: the lines,
    positions and words match, but hayro draws Helvetica, Times and Courier
    with its own embedded substitutes, which are lighter and shaped
    differently from poppler's URW fonts. M-3 can give hayro the fonts the OS
    ships through `font_resolver`, as this spike does for CJK.
  - The scanned grey JPEG differs on 8%: JPEG decoding and image scaling
    differ, since neither renderer interpolates the same way.
  - Every page is one pixel narrower and shorter than pdftoppm's: hayro
    truncates the page size, poppler rounds it up. The comparison is over the
    area both share, aligned top-left.
- **No refusal or crash on any page of the seven pdf.js samples**, which
  exercise CID fonts and vertical writing, transparency groups, shadings,
  CCITT fax images and widget appearances. JBIG2, JPX and Type 3 fonts are
  not in this corpus; a later run can add them.

For M-3 that means hayro is fit to render, given a font set for the standard 14
and a memory ceiling. The figures come from one run on one runner;
repeat runs are a `workflow_dispatch` away.
