# The paper's five figures as plain SVG, for the printed PDF (build_pdf.py). They are drawn from the same coordinates as the
# figures in the paper's doc, with Times New Roman and type sized for a 6.5-inch column, and every label is SVG text, so it
# can be selected and searched in the PDF. If a figure changes in the doc, change it here too.

INK, QUIET, EDGE = "#000000", "#444444", "#7a7a7a"
ACCENT, BAD, GOOD = "#1f5fbf", "#b3261e", "#2e7d32"
FONT = "'Times New Roman', Times, serif"
TITLE, BODY, SMALL = 17, 15.5, 13  # at 760 units to 6.5 inches: about 10.5, 9.5 and 8 points


def esc(s):
    return s.replace("&", "&amp;").replace("<", "&lt;").replace(">", "&gt;")


def text(x, y, s, size=BODY, fill=INK, bold=False, anchor="start"):
    weight = ' font-weight="bold"' if bold else ""
    return f'<text x="{x}" y="{y}" font-size="{size}" fill="{fill}" text-anchor="{anchor}"{weight}>{esc(s)}</text>'


def svg(height, label, body, marker=None):
    defs = ""
    if marker:
        defs = (f'<defs><marker id="{marker}" viewBox="0 0 10 10" refX="9" refY="5" markerWidth="7" markerHeight="7" '
                f'orient="auto-start-reverse"><path d="M0 0L10 5L0 10z" fill="{EDGE}"/></marker></defs>')
    return (f'<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 760 {height}" role="img" aria-label="{esc(label)}" '
            f'font-family="{FONT}">{defs}{body}</svg>')


def rect(x, y, w, h, rx=6, stroke=EDGE, width=1.25, fill="none", opacity=1):
    return (f'<rect x="{x}" y="{y}" width="{w}" height="{h}" rx="{rx}" fill="{fill}" fill-opacity="{opacity}" '
            f'stroke="{stroke}" stroke-width="{width}"/>')


def path(d, marker, dash=None):
    dashed = f' stroke-dasharray="{dash}"' if dash else ""
    return f'<path d="{d}" fill="none" stroke="{EDGE}" stroke-width="1.4"{dashed} marker-end="url(#{marker})"/>'


def fig1():
    """One choice wins this five-card row and the other loses."""
    b = [text(24, 30, "One choice wins this five-card row and the other loses", TITLE, bold=True),
         text(24, 52, "A pile moves onto the pile 1 place left (slide) or 3 places left (leap) if their top cards share a suit or rank.",
              SMALL, QUIET),
         text(244, 142, "Start", BODY, bold=True, anchor="end"), text(244, 160, "2 legal moves", SMALL, QUIET, anchor="end")]
    for i, c in enumerate(["J♦", "7♠", "8♠", "7♦", "8♣"]):
        b += [rect(260 + 50 * i, 120, 40, 52), text(280 + 50 * i, 152, c, 16.5, anchor="middle")]
    b += [path("M430 116Q355 66 282 116", "f1a"), path("M380 176Q355 198 332 176", "f1a"),
          path("M380 216V226H190V238", "f1a"), path("M380 216V226H570V238", "f1a"),
          text(355, 84, "leap: same suit", SMALL, QUIET, anchor="middle"), text(356, 206, "slide: same suit", SMALL, QUIET, anchor="middle")]
    b += [rect(24, 240, 332, 148, 8, BAD, 1.5, BAD, 0.08), text(44, 268, "Slide 8♠ onto 7♠", BODY, bold=True)]
    for i, c in enumerate(["J♦", "8♠", "7♦", "8♣"]):
        b += [rect(44 + 40 * i, 284, 32, 42), text(60 + 40 * i, 311, c, BODY, anchor="middle")]
    b += [text(44, 350, "No card matches the one 1 or 3 places to its left.", SMALL),
          text(44, 370, "No moves, 4 piles left: lost", SMALL, bold=True)]
    b += [rect(404, 240, 332, 272, 8, GOOD, 1.5, GOOD, 0.08), text(424, 268, "Leap 7♦ onto J♦", BODY, bold=True)]
    rows = [(284, ["7♦", "7♠", "8♠", "8♣"], "next: 7♠ onto 7♦"), (340, ["7♠", "8♠", "8♣"], "next: 8♠ onto 7♠"),
            (396, ["8♠", "8♣"], "next: 8♣ onto 8♠"), (452, ["8♣"], None)]
    for y, cards, note in rows:
        for i, c in enumerate(cards):
            b += [rect(424 + 40 * i, y, 32, 42), text(440 + 40 * i, y + 27, c, BODY, anchor="middle")]
        if note:
            b.append(text(596, y + 25, note, SMALL, QUIET))
    b.append(text(596, 477, "One pile left: won", SMALL, bold=True))
    return svg(536, "One choice wins this five-card row and the other loses", "".join(b), "f1a")


def fig2():
    """Every deal ends as a replayed win or a proven loss."""
    def step(y0, h):
        return rect(24, y0, 440, h, 8)

    def end(y0, h):
        return rect(528, y0, 208, h, 8, ACCENT, 2, ACCENT, 0.12)

    m = "f2a"
    b = [text(24, 30, "Every deal ends as a replayed win or a proven loss", TITLE, bold=True)]
    b += [path(d, m) for d in ["M244 108V140", "M244 212V260", "M244 332V380", "M464 176H632V192",
                               "M464 296H632V280", "M464 416H700V280", "M464 536H528"]]
    b.append(path("M244 452V500", m, "5 4"))  # dashed: the Lean check was made after the run
    boxes = [(52, 56, "Deal from a seed", ["SplitMix64, then xoshiro256**, then a Fisher–Yates shuffle"]),
             (140, 72, "GPU: semi-random prefix, short endgame search",
              ["Plays 40 or 38 moves weighted by patterns from won games, then", "searches up to 700 positions. Up to 400 tries a deal."]),
             (260, 72, "CPU: thorough search",
              ["Shorter semi-random prefixes, then restarting depth-first search,", "with a dead-end test and complete endgame tables up to 11 piles"]),
             (380, 72, "Exhaustive search", ["Visits every reachable position with an exact table,", "in two separate searches that must agree"]),
             (500, 72, "After the run: certificate checked in Lean",
              ["A compact description of every position the deal can reach,", "checked in Lean to be closed under moves, with no win (Section 3)"])]
    for y0, h, name, lines in boxes:
        b += [step(y0, h), text(40, y0 + 24, name, BODY, bold=True)]
        b += [text(40, y0 + 41 + 16 * i, line, SMALL, QUIET) for i, line in enumerate(lines)]
    b += [end(192, 88), text(544, 216, "Replay, then won", BODY, bold=True)]
    b += [text(544, 233 + 16 * i, s, SMALL) for i, s in enumerate(["A separate check", "replays all 51 moves before", "the deal is counted as won"])]
    b += [end(500, 72), text(544, 530, "Proven lost", BODY, bold=True), text(544, 549, "No line of play can win", SMALL)]
    b += [text(254, 240, "missed: about 5.5 deals in 100,000", SMALL, QUIET), text(254, 360, "no win found", SMALL, QUIET),
          text(254, 480, "no win exists: 721 deals in 1,000,000,000,000", SMALL, QUIET)]
    b += [text(478, 168, "win", SMALL, QUIET), text(478, 288, "win", SMALL, QUIET), text(478, 408, "win", SMALL, QUIET)]
    return svg(596, "Every deal ends as a replayed win or a proven loss", "".join(b), m)


def fig3():
    """A whole certificate for a six-card row."""
    r, m = 14, "f3a"

    def link(x1, y1, x2, y2):
        dx, dy = x2 - x1, y2 - y1
        d = (dx * dx + dy * dy) ** 0.5
        ux, uy = dx / d, dy / d
        return path(f"M{x1 + ux * r:.2f} {y1 + uy * r:.2f}L{x2 - ux * (r + 2):.2f} {y2 - uy * (r + 2):.2f}", m)

    def under(x1, x2, cy):
        return path(f"M{x1 + 6} 233Q{(x1 + x2) / 2} {cy} {x2 - 6} 235", m)

    def over(x1, x2, cy):
        return path(f"M{x1 + 6} 207Q{(x1 + x2) / 2} {cy} {x2 - 6} 205", m)

    def node(x, y):
        return f'<circle cx="{x}" cy="{y}" r="{r}" fill="none" stroke="{EDGE}" stroke-width="1.25"/>'

    b = [text(24, 28, "Four of the five positions share one tail", TITLE, bold=True),
         text(24, 48, "Automaton of the row 8♣ 2♦ 2♠ J♠ Q♥ 4♣. Each path from the start to the double circle spells one position.",
              SMALL, QUIET),
         path("M18 160H40", m)]
    b += [link(56, 160, 250, 84), link(250, 84, 470, 84), link(470, 84, 704, 160), link(56, 160, 150, 220),
          link(150, 220, 258, 220), link(258, 220, 366, 220), link(366, 220, 474, 220), link(474, 220, 590, 220),
          link(590, 220, 704, 160), under(150, 366, 290), under(150, 474, 350), over(258, 474, 150)]
    b += [node(x, y) for x, y in [(56, 160), (250, 84), (470, 84), (150, 220), (258, 220), (366, 220), (474, 220), (590, 220)]]
    b += [f'<circle cx="704" cy="160" r="{r}" fill="{ACCENT}" fill-opacity="0.12" stroke="{ACCENT}" stroke-width="2"/>',
          f'<circle cx="704" cy="160" r="10" fill="none" stroke="{ACCENT}" stroke-width="1.25"/>']
    labels = [(142, 112, "4♣"), (360, 74, "J♠"), (600, 108, "Q♥"), (88, 208, "8♣"), (204, 212, "2♦"), (312, 212, "2♠"),
              (420, 212, "J♠"), (532, 212, "Q♥"), (662, 208, "4♣"), (258, 256, "2♠"), (312, 286, "J♠"), (366, 172, "J♠")]
    b += [text(x, y, s, BODY, anchor="middle") for x, y, s in labels]
    b += [text(24, 336, "Accepted: 8♣ 2♦ 2♠ J♠ Q♥ 4♣ (the deal), 8♣ 2♦ J♠ Q♥ 4♣, 8♣ 2♠ J♠ Q♥ 4♣, 8♣ J♠ Q♥ 4♣, and 4♣ J♠ Q♥.", SMALL, QUIET),
          text(24, 356, "Every legal move from an accepted word gives an accepted word, and no accepted word has one letter, so the row is lost.",
               SMALL, QUIET)]
    return svg(380, "Four of the five positions share one tail", "".join(b), m)


def card_grid(cards, marked, title, subtitles):
    b = [text(24, 28, title, TITLE, bold=True)]
    b += [text(24, 50 + 16 * i, s, SMALL, QUIET) for i, s in enumerate(subtitles) if s]
    for p, c in enumerate(cards.split(), 1):
        x, y = 24 + ((p - 1) % 13) * 54.6, 96 + ((p - 1) // 13) * 84
        hot = p in marked
        b += [text(round(x + 24, 1), y, str(p), 12, QUIET, anchor="middle"),
              rect(round(x, 1), y + 6, 48, 56, 6, ACCENT if hot else EDGE, 2 if hot else 1, ACCENT if hot else "none", 0.12 if hot else 1),
              text(round(x + 24, 1), y + 40, c, 17, bold=True, anchor="middle")]
    return svg(428, title, "".join(b))


def suits(cards):
    return " ".join(c[:-1].replace("T", "10") + {"C": "♣", "D": "♦", "H": "♥", "S": "♠"}[c[-1]] for c in cards.split())


def fig4():
    deal = ("JD 2H KS 9S JS 8S QH 7C 6D 5C 9H 4H 8H AC 2D QS 5D 7S 4C 7H QC AD 8D 9C 2S 3H TS 5H QD JH 4D 6H KC 5S KD 4S 3C "
            "8C AH 7D AS 6S TC KH 9D TH 3S TD 2C JC 6C 3D")
    return card_grid(suits(deal), {23, 24}, "Seed 112,249,515, the lowest lost seed, gets down to two piles",
                     ["None of its 19,386,305 reachable positions has one pile. The best line (Note 1) ends with two piles, topped by",
                      "8♦ and 9♣, which share neither suit nor rank, so neither can move onto the other."])


def fig5():
    deal = ("AC 4D TS JD 3H 5C 3S 6D 2H 9C 7S KC TH 3C JH 8S 5H 9S KH 2D QH JC 4S 8C 5S 2C 7D TC 9H 8H QS 5D 6C AH 6S 8D QC KD "
            "4H AS 7H AD 4C 3D 2S 6H KS 9D 7C QD JS TD")
    return card_grid(suits(deal), {29, 30}, "Seed 124,280,819,596 has one legal move, and is lost",
                     ["The only legal move in the row is 8♥ onto 9♥, from position 30 to 29. After it, no card matches",
                      "the card 1 or 3 places to its left, so 51 piles remain and the deal is lost."])


FIGURES = {1: fig1, 2: fig2, 3: fig3, 4: fig4, 5: fig5}
