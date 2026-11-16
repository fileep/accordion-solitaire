# Builds the paper's PDF from a Word export of its doc: 12-point Times New Roman on US Letter with 1-inch margins, tables and
# figures kept whole on a page, the formula set as text, and the figures drawn as SVG (figures.py) so their labels can be
# selected. Microsoft Edge prints the page; no other tool is needed.
#
#   python research/paper/build_pdf.py <export.docx> ["research/paper/The win rate of open Accordion solitaire.pdf"]
#
# The export can also be the saved JSON result of the docs connector's docx export, which holds the file as base64.
import base64
import html
import json
import re
import subprocess
import sys
import tempfile
import time
import zipfile
from pathlib import Path
from xml.etree import ElementTree

from figures import FIGURES

W = "{http://schemas.openxmlformats.org/wordprocessingml/2006/main}"
R = "{http://schemas.openxmlformats.org/officeDocument/2006/relationships}"
EDGE = Path(r"C:\Program Files (x86)\Microsoft\Edge\Application\msedge.exe")
HERE = Path(__file__).resolve().parent

CSS = """
@page { size: Letter; margin: 1in; @bottom-center { content: counter(page); font-family: "Times New Roman", Times, serif; font-size: 10pt; } }
html { font-family: "Times New Roman", Times, serif; font-size: 12pt; line-height: 1.35; color: #000; }
body { margin: 0; }
h1 { font-size: 17pt; text-align: center; margin: 0 0 6pt; line-height: 1.2; }
.byline { text-align: center; margin: 0 0 18pt; }
h2 { font-size: 13pt; margin: 16pt 0 6pt; break-after: avoid; page-break-after: avoid; }
h2.abstract { text-align: center; font-size: 12pt; margin-top: 0; }
h3 { font-size: 12pt; font-style: italic; margin: 12pt 0 4pt; break-after: avoid; page-break-after: avoid; }
p { margin: 0 0 8pt; text-align: justify; orphans: 3; widows: 3; }
p.abstract { margin: 0 0.4in 14pt; }
p.lead { break-after: avoid; page-break-after: avoid; }
a { color: #1a3d8f; text-decoration: none; }
code { font-family: "Courier New", monospace; font-size: 10.5pt; }
.formula { text-align: center; margin: 4pt 0 10pt; }
.formula .gap { display: inline-block; width: 3em; }
figure { margin: 10pt 0 14pt; break-inside: avoid; page-break-inside: avoid; }
figure svg { display: block; width: 100%; height: auto; }
figcaption { font-size: 11pt; text-align: center; margin-top: 4pt; }
table { border-collapse: collapse; width: 100%; margin: 4pt 0 12pt; font-size: 11pt; break-inside: avoid; page-break-inside: avoid; }
th, td { border: 0.5pt solid #888; padding: 3pt 5pt; text-align: left; vertical-align: top; }
th { background: #f0f0f0; font-weight: bold; }
ul, ol { margin: 0 0 8pt; padding-left: 1.6em; }
li { margin-bottom: 3pt; }
li p { margin: 0; text-align: left; }
ol.references { list-style: none; padding-left: 2.2em; counter-reset: ref; }
ol.references li { counter-increment: ref; position: relative; }
ol.references li::before { content: "[" counter(ref) "]"; position: absolute; left: -2.2em; }
ol.notes li p { word-spacing: 0.1em; }
"""


def load(path):
    path = Path(path)
    if path.suffix.lower() != ".docx":
        data = json.loads(path.read_text(encoding="utf-8"))
        payload = json.loads(data[1]["text"]) if isinstance(data, list) else data
        raw = base64.b64decode(payload["data"]["bytes_b64"])
        path = Path(tempfile.gettempdir()) / "paper-export.docx"
        path.write_bytes(raw)
    z = zipfile.ZipFile(path)
    body = ElementTree.fromstring(z.read("word/document.xml")).find(f"{W}body")
    rels = {r.get("Id"): r.get("Target") for r in ElementTree.fromstring(z.read("word/_rels/document.xml.rels"))}
    numbering = ElementTree.fromstring(z.read("word/numbering.xml"))
    abstract = {a.get(f"{W}abstractNumId"): a.find(f".//{W}numFmt").get(f"{W}val") for a in numbering.iter(f"{W}abstractNum")}
    kinds = {n.get(f"{W}numId"): ("ul" if abstract[n.find(f"{W}abstractNumId").get(f"{W}val")] == "bullet" else "ol")
             for n in numbering.iter(f"{W}num")}
    return body, rels, kinds


def run_html(r):
    out = []
    for child in r:
        if child.tag == f"{W}t":
            out.append(html.escape(child.text or "", quote=False))
        elif child.tag == f"{W}tab":
            out.append(" ")
        elif child.tag == f"{W}br":
            out.append("<br>")
    s = "".join(out)
    props = r.find(f"{W}rPr")
    if props is not None and s:
        style = props.find(f"{W}rStyle")
        if style is not None and style.get(f"{W}val") == "CodeChar":
            s = f"<code>{s}</code>"
        if props.find(f"{W}i") is not None:
            s = f"<em>{s}</em>"
        if props.find(f"{W}b") is not None:
            s = f"<strong>{s}</strong>"
    return s


def para(p, rels):
    """(inner html, plain text, has a picture)"""
    parts, picture = [], False
    for child in p:
        if child.tag == f"{W}r":
            if child.find(f".//{W}drawing") is not None:
                picture = True
            parts.append(run_html(child))
        elif child.tag == f"{W}hyperlink":
            inner = "".join(run_html(r) for r in child.iter(f"{W}r"))
            target = rels.get(child.get(f"{R}id"), "")
            parts.append(f'<a href="{html.escape(target)}">{inner}</a>' if target else inner)
    inner = "".join(parts)
    plain = html.unescape(re.sub(r"<[^>]+>", "", inner))
    return inner, plain, picture


def formula(latex):
    """The rules' two moves, written as italic letters and arrows instead of LaTeX source."""
    def side(s):
        return " ".join(f"<i>{ch}</i>" for ch in s.replace("\\,", " ").split())

    rules = []
    for rule in latex.split("\\qquad\\qquad"):
        lhs, rhs = rule.split("\\;\\to\\;")
        rules.append(f"{side(lhs)} → {side(rhs)}")
    return '<p class="formula">' + '<span class="gap"></span>'.join(rules) + "</p>"


def table(tbl, rels):
    rows = []
    for i, tr in enumerate(tbl.findall(f"{W}tr")):
        header = tr.find(f"{W}trPr/{W}tblHeader") is not None or i == 0
        cells = []
        for tc in tr.findall(f"{W}tc"):
            inner = "<br>".join(para(p, rels)[0] for p in tc.findall(f"{W}p"))
            cells.append(f"<th>{inner}</th>" if header else f"<td>{inner}</td>")
        rows.append("<tr>" + "".join(cells) + "</tr>")
    return "<table>" + "".join(rows) + "</table>"


def build(body, rels, kinds):
    blocks, picture, lst, section = [], False, None, ""

    def close_list():
        nonlocal lst
        if lst:
            kind, cls, items = lst
            blocks.append(f'<{kind}{cls}>' + "".join(items) + f"</{kind}>")
            lst = None

    for el in body:
        if el.tag == f"{W}tbl":
            close_list()
            if blocks and blocks[-1].startswith("<p>"):
                blocks[-1] = '<p class="lead">' + blocks[-1][3:]
            blocks.append(table(el, rels))
            continue
        if el.tag != f"{W}p":
            continue
        props = el.find(f"{W}pPr")
        style = props.find(f"{W}pStyle").get(f"{W}val") if props is not None and props.find(f"{W}pStyle") is not None else ""
        num = props.find(f"{W}numPr/{W}numId") if props is not None else None
        inner, plain, has_picture = para(el, rels)
        if has_picture:
            close_list()
            picture = True
            continue
        if picture and (m := re.match(r"Figure (\d+)\.", plain)):
            picture = False
            blocks.append(f"<figure>{FIGURES[int(m.group(1))]()}<figcaption>{inner}</figcaption></figure>")
            continue
        if num is not None:
            kind = kinds.get(num.get(f"{W}val"), "ul")
            cls = ' class="references"' if section == "References" else ' class="notes"' if section == "Notes" else ""
            if not lst or lst[0] != kind:
                close_list()
                lst = (kind, cls, [])
            lst[2].append(f"<li><p>{inner}</p></li>")
            continue
        close_list()
        if not plain.strip():
            continue
        if style == "Heading1":
            blocks.append(f"<h1>{inner}</h1>")
        elif style in ("Heading2", "Heading3"):
            section = plain.strip()
            tag = "h2" if style == "Heading2" else "h3"
            cls = ' class="abstract"' if section == "Abstract" else ""
            blocks.append(f"<{tag}{cls}>{inner}</{tag}>")
        elif style == "Code":
            blocks.append(formula(plain) if "\\to" in plain else f"<pre><code>{inner}</code></pre>")
        elif len(blocks) == 1 and "@" in plain:
            date, _, author = plain.partition("·")
            blocks.append(f'<p class="byline">{html.escape(author.strip().lstrip("@"))}<br>{html.escape(date.strip())}</p>')
        else:
            cls = ' class="abstract"' if section == "Abstract" else ""
            blocks.append(f"<p{cls}>{inner}</p>")
    close_list()
    title = html.unescape(re.sub(r"<[^>]+>", "", blocks[0]))
    return (f'<!doctype html><html lang="en"><head><meta charset="utf-8"><title>{html.escape(title)}</title>'
            f"<style>{CSS}</style></head><body>" + "\n".join(blocks) + "</body></html>")


def main():
    body, rels, kinds = load(sys.argv[1])
    out = Path(sys.argv[2]) if len(sys.argv) > 2 else HERE / "The win rate of open Accordion solitaire.pdf"
    page = out.with_suffix(".html")
    page.write_text(build(body, rels, kinds), encoding="utf-8")
    # Edge's launcher returns at once and the print lands a few seconds later, so wait for a new, settled file.
    before = out.stat().st_mtime if out.exists() else 0
    profile = Path(tempfile.gettempdir()) / "accordion-paper-edge"
    subprocess.run([str(EDGE), "--headless=new", "--disable-gpu", "--no-first-run", "--no-pdf-header-footer",
                    f"--user-data-dir={profile}", f"--print-to-pdf={out.resolve()}", page.resolve().as_uri()],
                   check=True, timeout=120, capture_output=True)
    size, deadline = -1, time.time() + 90
    while time.time() < deadline:
        if out.exists() and out.stat().st_mtime > before and out.stat().st_size == size > 0:
            break
        size = out.stat().st_size if out.exists() and out.stat().st_mtime > before else -1
        time.sleep(1)
    else:
        sys.exit(f"Edge did not write {out}")
    print(f"{out} ({out.stat().st_size:,} bytes), from {page.name}")


if __name__ == "__main__":
    main()
