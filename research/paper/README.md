# Paper

Figures for a paper on the win rate, in preparation and not yet published. `paper_numbers.py` prints every figure
it takes from the runs, from the records in `research/runs`, `research/lean` and `hard-deals`.

## The PDF

`build_pdf.py` prints the paper from a Word export of its doc (Export → Word, or the docs connector's docx export): 12-point
Times New Roman on US Letter with 1-inch margins, page numbers, tables and figures kept whole on a page, and the figures drawn
as SVG by `figures.py`, so their text can be selected. Microsoft Edge does the printing. `figures.py` copies the doc's figures by
hand, so a change to a figure in the doc has to be made there too.

```bash
python research/paper/build_pdf.py "The win rate of open Accordion solitaire.docx"
```
