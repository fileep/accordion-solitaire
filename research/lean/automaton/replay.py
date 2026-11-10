# Checks the proofs of prove_all.py again with nanoda, a type checker for Lean written in Rust that shares no code with Lean's kernel.
# Each batch is exported with leanexport, and nanoda checks every declaration its theorems rest on, from the definitions of the rules up.
# Both come with the Lean toolchain. Batches already passed are skipped, so it can be stopped and run again.
#
#   python research/lean/automaton/replay.py research/lean/automaton/lean_results.csv research/lean/automaton/replay_results.csv <build dir> [jobs]     (from the repository root)
#
# The build dir is the one prove_all.py used, which holds the .olean of each batch.
import csv
import json
import os
import struct
import subprocess
import sys
import threading
import time
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path

from prove_all import LEAN, MEMORY_CAP_GB, cpu_seconds, memory_gb

FIELDS = ["seeds", "declarations", "export_mb", "cpu_seconds", "peak_gb", "ok"]
BIN = Path(LEAN).parent
STACK_BYTES = 8 << 30  # nanoda recurses deeply on these proofs, and Windows gives a main thread 1 MB unless the program says otherwise


def large_stack_copy(build):
    """A copy of nanoda whose header asks for a larger stack. Memory is only committed as it is used."""
    data = bytearray((BIN / "nanoda_bin.exe").read_bytes())
    header = struct.unpack_from("<I", data, 0x3C)[0]
    assert data[header:header + 4] == b"PE\0\0" and struct.unpack_from("<H", data, header + 24)[0] == 0x20B
    struct.pack_into("<Q", data, header + 24 + 72, STACK_BYTES)
    copy = build / "nanoda_large_stack.exe"
    copy.write_bytes(data)
    return copy


def run(command, output, env, cwd):
    """Runs a command with its output in a file, killing it at the memory cap. Returns (exit code, or None if killed; CPU seconds; peak GB)."""
    with output.open("wb") as out:
        process = subprocess.Popen([str(c) for c in command], env=env, cwd=cwd, stdout=out, stderr=subprocess.STDOUT)
        while process.poll() is None:
            if memory_gb(process)[0] > MEMORY_CAP_GB:
                process.kill()
                process.wait()
                return None, cpu_seconds(process), memory_gb(process)[1]
            time.sleep(0.1)
    return process.returncode, cpu_seconds(process), memory_gb(process)[1]


def check_export(export, nanoda, log, build):
    """Has nanoda check an export. Returns (the number of declarations it passed, or 0; exit code; CPU seconds; peak GB)."""
    config = export.with_suffix(".json")
    config.write_text(json.dumps({"export_file_path": str(export), "use_stdin": False, "permitted_axioms": ["propext", "Quot.sound"],
                                  "unpermitted_axiom_hard_error": True, "nat_extension": True, "string_extension": True, "print_success_message": True}))
    code, cpu, peak = run([nanoda, config], log, dict(os.environ), build)
    text = log.read_text(encoding="utf-8", errors="replace")
    found = text.split("Checked ")[-1].split(" declarations with no typechecker errors") if "with no typechecker errors" in text else None
    return (int(found[0]) if found and code == 0 else 0), code, cpu, peak


def replay(seeds, build, nanoda, env, record):
    module = f"B{seeds[0]}"
    export, log = build / f"{module}.export", build / f"{module}.nanoda.txt"
    code, export_cpu, export_peak = run([BIN / "leanexport.exe", module, "--", *(f"lost_{s}" for s in seeds)], export, env, build)
    declarations, cpu, peak, size = 0, 0.0, 0.0, export.stat().st_size / 1e6
    if code == 0:
        declarations, code, cpu, peak = check_export(export, nanoda, log, build)
    ok = declarations > 0
    record({"seeds": " ".join(map(str, seeds)), "declarations": declarations, "export_mb": round(size, 1), "cpu_seconds": round(export_cpu + cpu, 1),
            "peak_gb": round(max(export_peak, peak), 2), "ok": ok})
    if ok:
        export.unlink()
    else:
        print(seeds, "failed, exit code", code, flush=True)


def main():
    results, out, build = Path(sys.argv[1]), Path(sys.argv[2]), Path(sys.argv[3]).resolve()
    jobs = int(sys.argv[4]) if len(sys.argv) > 4 else 1
    rows = list(csv.DictReader(out.open(encoding="utf-8"))) if out.exists() else []
    done = {r["seeds"] for r in rows if r["ok"] == "True"}
    batches = [r["seeds"] for r in csv.DictReader(results.open(encoding="utf-8")) if r["ok"] == "True" and r["seeds"] not in done]
    env, nanoda, lock = dict(os.environ, LEAN_PATH=str(build)), large_stack_copy(build), threading.Lock()
    with out.open("a", newline="", encoding="utf-8") as f:
        writer = csv.DictWriter(f, fieldnames=FIELDS, lineterminator=chr(10))
        if not rows:
            writer.writeheader()

        def record(row):
            with lock:
                writer.writerow(row)
                f.flush()

        with ThreadPoolExecutor(jobs) as pool:
            for seeds in batches:
                pool.submit(replay, seeds.split(), build, nanoda, env, record)


if __name__ == "__main__":
    main()
