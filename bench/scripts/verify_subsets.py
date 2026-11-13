#!/usr/bin/env python3
"""Check every subset in bench/data/ against bench/data/manifest.json (A1, B7).

The subsets are generated once in the Mastic codebase and copied here; this checks that the copy
is byte for byte the same rides. Per manifest entry:

- the file exists and its SHA-256 equals the manifest's;
- line endings are LF only (manifest "line_endings") and the file ends with a newline;
- the header has the coordinate columns and every row has finite coordinates;
- the number of data rows equals the manifest's, and equals round(N/3), round(2N/3), N (A1 step 4);
- the subsets are nested: each smaller subset is the first rows of every larger one (A1 step 4).

With --source <sample_busiest_day.csv> it also checks the source file's hash (the manifest records
the bytes on disk and the LF-normalised bytes) and that the full subset is a permutation of it.

    .\\.venv\\Scripts\\python.exe bench\\scripts\\verify_subsets.py
    .\\.venv\\Scripts\\python.exe bench\\scripts\\verify_subsets.py --source C:\\Users\\arabe\\IMP\\mastic_for_fuzzy_HH\\data\\mosaic\\sample_busiest_day.csv

Exits 1 with a FAIL banner on any mismatch. The other scripts call check_subsets() first.
"""

import argparse
import csv
import hashlib
import io
import math
import sys
from collections import Counter
from pathlib import Path

sys.dont_write_bytecode = True  # no __pycache__ in the repo
sys.path.insert(0, str(Path(__file__).resolve().parent))
import bench_common as bc  # noqa: E402


def _data_lines(raw):
    """Header and data lines of an LF file (bytes, without the trailing newline)."""
    lines = raw.split(b"\n")
    if lines and lines[-1] == b"":
        lines.pop()
    return lines[0], lines[1:]


def check_subsets(source=None, verbose=True):
    """(ok, problems, rows) for the subsets of the manifest; prints one line per check."""
    say = print if verbose else (lambda *a, **k: None)
    manifest = bc.load_manifest()
    problems = []

    def check(ok, label, detail=""):
        say(f"  {'PASS' if ok else 'FAIL'}  {label}{': ' + detail if detail else ''}")
        if not ok:
            problems.append(f"{label}{': ' + detail if detail else ''}")
        return ok

    check(manifest.get("dataset") == bc.DATASET, "manifest dataset", repr(manifest.get("dataset")))
    check(manifest.get("seed") == bc.SEED, "manifest seed", repr(manifest.get("seed")))
    n_source = manifest["source"]["rows_kept"]
    expected_rows = {"1of3": round(n_source / 3), "2of3": round(2 * n_source / 3), "3of3": n_source}

    data, header_of, rows = {}, {}, {}
    for entry in manifest["files"]:
        subset, path = entry["subset"], bc.ROOT / entry["path"]
        say(f"{entry['path']} (subset {subset}):")
        if not check(path.exists(), "exists"):
            continue
        raw = path.read_bytes()
        digest = hashlib.sha256(raw).hexdigest()
        check(digest == entry["sha256"], "SHA-256 equals the manifest",
              f"got {digest}, manifest {entry['sha256']}" if digest != entry["sha256"]
              else digest[:16] + "...")
        check(b"\r" not in raw and manifest.get("line_endings") == "LF", "LF line endings only")
        check(raw.endswith(b"\n"), "ends with a newline")
        header, lines = _data_lines(raw)
        header_of[subset], data[subset] = header, lines
        rows[subset] = len(lines)
        check(len(lines) == entry["rows"], "data rows equal the manifest",
              f"{len(lines)} (manifest {entry['rows']})")
        check(len(lines) == expected_rows.get(subset), "data rows equal A1's round(k*N/3)",
              f"{len(lines)} (expected {expected_rows.get(subset)})")
        columns = header.decode("utf-8").split(",")
        check(all(c in columns for c in bc.COORD_COLUMNS), "coordinate columns present")
        bad = 0
        for row in csv.DictReader(io.StringIO(raw.decode("utf-8"), newline="")):
            try:
                ok = all(math.isfinite(float(row[c])) for c in bc.COORD_COLUMNS)
            except (KeyError, TypeError, ValueError):
                ok = False
            bad += not ok
        check(bad == 0, "every row has finite coordinates", f"{bad} bad rows")
        check(entry.get("seed") == bc.SEED, "entry seed", repr(entry.get("seed")))
        check(entry["source"]["sha256"] == manifest["source"]["sha256"], "entry source hash")

    order = [s for s in bc.SUBSETS if s in data]
    if order:
        say("Across subsets:")
        check(len({header_of[s] for s in order}) == 1, "same header in every subset")
        for small, big in zip(order, order[1:]):
            n = len(data[small])
            check(data[big][:n] == data[small], f"{small} is the first {n} rows of {big}")

    if source is not None:
        source = Path(source)
        say(f"Source {source}:")
        if check(source.exists(), "exists"):
            raw = source.read_bytes()
            lf = raw.replace(b"\r\n", b"\n")
            digests = {hashlib.sha256(raw).hexdigest(), hashlib.sha256(lf).hexdigest()}
            src = manifest["source"]
            check(bool(digests & {src["sha256"], src["sha256_lf"]}),
                  "SHA-256 equals the manifest (bytes on disk or LF-normalised)")
            header, lines = _data_lines(lf)
            check(len(lines) == src["rows"], "rows equal the manifest", f"{len(lines)}")
            if "3of3" in data:
                check(header == header_of["3of3"], "same header as the subsets")
                check(Counter(lines) == Counter(data["3of3"]),
                      "day_3of3 is a permutation of the source rows")

    return not problems, problems, rows


def main():
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--source", help="Mastic's data/mosaic/sample_busiest_day.csv (optional)")
    args = parser.parse_args()
    ok, problems, rows = check_subsets(args.source)
    if ok:
        print(f"\nverify_subsets: PASS ({', '.join(f'{s}: {n} rows' for s, n in rows.items())}); "
              f"T at tau 0.02 = {[bc.threshold_abs(n, 0.02) for n in rows.values()]}, "
              f"at tau 0.005 = {[bc.threshold_abs(n, 0.005) for n in rows.values()]}")
        return
    print("\n" + "=" * 72)
    print("verify_subsets: FAIL. bench/data does not match manifest.json:")
    for p in problems:
        print(f"  - {p}")
    print("Re-copy bench/data/ from the Mastic codebase; do not benchmark until this passes.")
    print("=" * 72)
    raise SystemExit(1)


if __name__ == "__main__":
    main()
