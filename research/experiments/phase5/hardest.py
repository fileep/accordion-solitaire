"""Write the N deals that took the most positions in a per-deal file from `acc experiment --deals`.

Usage: python hardest.py deals/regression-phase4.jsonl 1000 > sets/hard.txt
"""
import json
import sys

path, n = sys.argv[1], int(sys.argv[2])
rows = [json.loads(line) for line in open(path, encoding="utf-8")]
rows.sort(key=lambda r: -r["nodes"])
print(f"# the {n} deals with the most positions in {path.replace(chr(92), '/')}"
      f" ({rows[n - 1]['nodes']:,} to {rows[0]['nodes']:,})")
for r in sorted(rows[:n], key=lambda r: r["seed"]):
    print(r["seed"])
