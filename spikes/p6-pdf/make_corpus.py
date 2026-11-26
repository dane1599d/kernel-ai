#!/usr/bin/env python3
"""Spike G13's 20 PDFs, into corpus/: 13 made here (reportlab, Pillow and
qpdf, all deterministic), 7 fetched from pdf.js's test corpus at a pinned
commit. None is committed. Each line of corpus/index.tsv is
`<file>\t<kind>\t<dpi>`; the runner renders each at that dpi.

Usage: make_corpus.py <corpus-dir>
"""

import os
import random
import subprocess
import sys
import urllib.request

from PIL import Image, ImageDraw, ImageFilter
from reportlab.lib.colors import Color
from reportlab.lib.pagesizes import A4
from reportlab.pdfbase import pdfmetrics
from reportlab.pdfbase.cidfonts import UnicodeCIDFont
from reportlab.pdfgen import canvas

PDFJS = "808186117c0e567d31c303a0b61111083d8de848"
FETCHED = [
    ("tracemonkey.pdf", "text and figures (a paper, 14 pages)"),
    ("TAMReview.pdf", "magazine pages, images"),
    ("alphatrans.pdf", "vector, transparency groups"),
    ("vertical.pdf", "CJK, vertical writing"),
    ("annotation-text-widget.pdf", "form, text widgets"),
    ("shading_extend.pdf", "vector, shadings"),
    ("ccitt_EndOfBlock_false.pdf", "scanned, CCITT fax"),
]

WORDS = (
    "the files of a person are theirs and an agent acts for them only when "
    "asked each action can be undone and every change is shown before it is "
    "made a page of text set in a plain face with numbers 0123456789"
).split()


def words(rng, n):
    return " ".join(rng.choice(WORDS) for _ in range(n))


def text_pages(path, pages, seed):
    rng = random.Random(seed)
    c = canvas.Canvas(path, pagesize=A4, invariant=1)
    for p in range(pages):
        c.setFont("Helvetica-Bold", 16)
        c.drawString(56, 800, f"Chapter {p + 1}")
        y = 770
        for line in range(60):
            face = ("Times-Roman", "Helvetica", "Courier")[line % 3]
            c.setFont(face, 10)
            c.drawString(56, y, words(rng, 14))
            y -= 12
        c.showPage()
    c.save()


def scan(path):
    """An A4 page at 300 dpi as one grey JPEG, as a scanner makes it."""
    rng = random.Random(7)
    img = Image.new("L", (2480, 3508), 236)
    draw = ImageDraw.Draw(img)
    for row in range(90):
        y = 180 + row * 36
        x = 200
        while x < 2200:
            w = rng.randint(20, 120)
            draw.rectangle([x, y, x + w, y + 18], fill=rng.randint(20, 70))
            x += w + rng.randint(12, 30)
    img = img.rotate(0.6, fillcolor=236).filter(ImageFilter.GaussianBlur(1.2))
    noise = Image.effect_noise((2480, 3508), 18)
    img = Image.blend(img, noise, 0.08)
    img.save(path, "PDF", resolution=300.0, quality=80)


def form(path):
    c = canvas.Canvas(path, pagesize=A4, invariant=1)
    c.setFont("Helvetica", 12)
    f = c.acroForm
    c.drawString(56, 780, "Name")
    f.textfield(name="name", x=140, y=770, width=300, height=20, value="Ada")
    c.drawString(56, 740, "Notes")
    f.textfield(name="notes", x=140, y=650, width=300, height=100, value="Two\nlines", fieldFlags="multiline")
    c.drawString(56, 620, "Agree")
    f.checkbox(name="agree", x=140, y=612, size=16, checked=True)
    for n, label in enumerate(["Small", "Large"]):
        c.drawString(180 + n * 120, 580, label)
        f.radio(name="size", value=label, x=160 + n * 120, y=578, size=14, selected=n == 1)
    f.choice(name="colour", x=140, y=530, width=150, height=20, options=["Red", "Blue"], value="Blue")
    c.showPage()
    c.save()


def cjk(path):
    """Non-embedded CID fonts in all four collections, as many CJK PDFs are."""
    fonts = [
        ("HeiseiMin-W3", "日本語の文章です。ファイルは本人のものです。"),
        ("STSong-Light", "这是中文文本。文件属于用户本人。"),
        ("MSung-Light", "這是繁體中文。檔案屬於使用者本人。"),
        ("HYSMyeongJo-Medium", "한국어 문장입니다. 파일은 본인의 것입니다."),
    ]
    c = canvas.Canvas(path, pagesize=A4, invariant=1)
    y = 780
    for name, text in fonts:
        pdfmetrics.registerFont(UnicodeCIDFont(name))
        for size in (24, 14, 9):
            c.setFont(name, size)
            c.drawString(56, y, text)
            y -= size * 1.8
        y -= 20
    c.showPage()
    c.save()


def vector(path):
    rng = random.Random(3)
    c = canvas.Canvas(path, pagesize=A4, invariant=1)
    for _ in range(2000):
        c.setFillColor(Color(rng.random(), rng.random(), rng.random(), alpha=rng.uniform(0.2, 1)))
        c.setStrokeColor(Color(rng.random(), rng.random(), rng.random()))
        c.setLineWidth(rng.uniform(0.2, 4))
        c.setDash(rng.choice([[], [3, 2], [6, 3, 1, 3]]))
        p = c.beginPath()
        p.moveTo(rng.uniform(0, 595), rng.uniform(0, 842))
        for _ in range(3):
            pts = [rng.uniform(0, 595) if i % 2 == 0 else rng.uniform(0, 842) for i in range(6)]
            p.curveTo(*pts)
        p.close()
        c.drawPath(p, fill=1, stroke=1)
    c.linearGradient(50, 50, 545, 300, (Color(1, 0, 0), Color(0, 0, 1)), extend=False)
    c.showPage()
    c.save()


def huge_image(path):
    """One page holding an 8000 x 8000 RGB JPEG."""
    rng = random.Random(11)
    img = Image.new("RGB", (8000, 8000))
    draw = ImageDraw.Draw(img)
    for _ in range(4000):
        x, y = rng.randint(0, 7900), rng.randint(0, 7900)
        draw.ellipse([x, y, x + rng.randint(20, 400), y + rng.randint(20, 400)],
                     fill=(rng.randint(0, 255), rng.randint(0, 255), rng.randint(0, 255)))
    img.save(path, "PDF", resolution=600.0, quality=85)


def qpdf(*args):
    subprocess.run(["qpdf", *args], check=True)


def main():
    out = sys.argv[1]
    os.makedirs(out, exist_ok=True)
    at = lambda name: os.path.join(out, name)
    rows = []

    text_pages(at("gen-text-30p.pdf"), 30, 1)
    rows.append(("gen-text-30p.pdf", "text, 30 pages, standard fonts", 150))
    scan(at("gen-scan-300dpi.pdf"))
    rows.append(("gen-scan-300dpi.pdf", "scanned, grey JPEG at 300 dpi", 150))
    form(at("gen-form.pdf"))
    rows.append(("gen-form.pdf", "form, AcroForm fields", 150))
    cjk(at("gen-cjk.pdf"))
    rows.append(("gen-cjk.pdf", "CJK, non-embedded CID fonts", 150))
    vector(at("gen-vector.pdf"))
    rows.append(("gen-vector.pdf", "vector, 2,000 curves with alpha", 150))

    plain = at("gen-text-30p.pdf")
    qpdf("--encrypt", "", "owner", "256", "--", plain, at("gen-enc-aes256.pdf"))
    rows.append(("gen-enc-aes256.pdf", "encrypted, AES-256, empty user password", 150))
    # RC4 on purpose: old PDFs use it, and qpdf writes it only when asked.
    qpdf("--allow-weak-crypto", "--encrypt", "", "owner", "128", "--use-aes=n", "--", plain, at("gen-enc-rc4.pdf"))
    rows.append(("gen-enc-rc4.pdf", "encrypted, RC4-128, empty user password", 150))
    qpdf("--encrypt", "secret", "owner", "256", "--", plain, at("gen-enc-password.pdf"))
    rows.append(("gen-enc-password.pdf", "encrypted, a user password (should refuse)", 150))

    data = open(plain, "rb").read()
    open(at("gen-broken-truncated.pdf"), "wb").write(data[: len(data) * 6 // 10])
    rows.append(("gen-broken-truncated.pdf", "broken, cut at 60%", 150))
    at_xref = data.rfind(b"startxref")
    bad = data[:at_xref] + b"startxref\n999999\n%%EOF\n"
    open(at("gen-broken-xref.pdf"), "wb").write(bad)
    rows.append(("gen-broken-xref.pdf", "broken, startxref points nowhere", 150))
    rng = random.Random(5)
    junk = bytearray(data)
    for _ in range(400):
        i = rng.randrange(len(junk) // 4, len(junk) * 3 // 4)
        junk[i] = rng.randrange(256)
    open(at("gen-broken-bytes.pdf"), "wb").write(bytes(junk))
    rows.append(("gen-broken-bytes.pdf", "broken, 400 bytes changed", 150))

    text_pages(at("gen-huge-500p.pdf"), 500, 2)
    rows.append(("gen-huge-500p.pdf", "huge, 500 pages (first 50 rendered)", 150))
    huge_image(at("gen-huge-image.pdf"))
    rows.append(("gen-huge-image.pdf", "huge, an 8000x8000 JPEG on one page", 150))

    for name, kind in FETCHED:
        url = f"https://raw.githubusercontent.com/mozilla/pdf.js/{PDFJS}/test/pdfs/{name}"
        with urllib.request.urlopen(url) as r:
            open(at(name), "wb").write(r.read())
        rows.append((name, f"pdf.js: {kind}", 150))

    with open(at("index.tsv"), "w") as f:
        for name, kind, dpi in rows:
            f.write(f"{name}\t{kind}\t{dpi}\n")
    print(f"{len(rows)} PDFs in {out}")


if __name__ == "__main__":
    main()
