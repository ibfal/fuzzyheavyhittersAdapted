#!/usr/bin/env python3
"""Convert the ride CSVs in bench/data/ into Mosaic's input format (B2.9) and check the conversion.

Mosaic's client, servers and plaintext read a JSON list of integer points (mosaic_client.rs:6-13,
mosaic_others.rs:28-35), not the CSV. This script runs Mosaic's own converter,
`ride_austin_json_generator` (src/bin/data/ride_austin_json_generator.rs, encode_csv in
src/data_toolkit/encoding/mod.rs:201-257), with every encoding parameter explicit
(bench_common.ENCODINGS), for the three day subsets and the smoke-test CSV:

    bench/data/encodings/<encoding>/<name>.json            the points
    bench/data/encodings/<encoding>/<name>.encoding.json   the generator's metadata
    bench/data/encodings/manifest.json                     arguments, counts and hashes

`points_sha256` in the manifest hashes the points as compact JSON, so it doesn't depend on line
endings (Git may check the JSON files out with CRLF); ground_truth.py checks it.

Checks (each one fatal):
  (a) the generator on the full day CSV gives start_busiest_day.json exactly (order and values);
  (b) Mosaic's grid formula in Python (bench_common.to_grid, data_toolkit/mod.rs:10-42) on the full
      day CSV gives start_busiest_day.json exactly;
  (c) the Python formula equals the generator's mosaic_grid output for every converted CSV;
  (d) the day_3of3 grid points equal start_busiest_day.json as a multiset;
  (e) Morton = grid - (634, 693) at every point (the default box, morton.rs:10-12);
  (f) no ride dropped or clamped by any encoding, so N = rows of the CSV (A2's T uses N);
  (g) every H3 point satisfies the derived coordinate: k = i - j + 1024 (cli_config.rs:95-98).

    .\\.venv\\Scripts\\python.exe bench\\scripts\\convert_inputs.py --source-dir C:\\Users\\arabe\\IMP\\mastic_for_fuzzy_HH\\data\\mosaic

--source-dir holds sample_busiest_day.csv and start_busiest_day.json (Mastic's data/mosaic/; this
repo's data/ is gitignored and empty). Needs target/release/ride_austin_json_generator(.exe).
"""

import argparse
import csv
import json
import subprocess
import sys
import tempfile
from collections import Counter
from pathlib import Path

sys.dont_write_bytecode = True  # no __pycache__ in the repo
sys.path.insert(0, str(Path(__file__).resolve().parent))
import bench_common as bc  # noqa: E402
from verify_subsets import check_subsets  # noqa: E402

SMOKE_CSV = bc.DATA_DIR / "ten_locations.csv"
END_COLUMNS = ("end_location_lat", "end_location_long")


def with_end_columns(csv_path, tmp_dir):
    """The generator's Ride struct needs end_location_lat/long columns (encoding/mod.rs:192-199)
    even for pickups only. A CSV without them is copied with the pickup as the drop-off;
    --output-type 0 never reads them."""
    with open(csv_path, newline="", encoding="utf-8") as f:
        reader = csv.DictReader(f)
        fields = list(reader.fieldnames or [])
        if all(c in fields for c in END_COLUMNS):
            return Path(csv_path)
        rows = list(reader)
    out = Path(tmp_dir) / f"{Path(csv_path).stem}_with_end.csv"
    with open(out, "w", newline="", encoding="utf-8") as f:
        writer = csv.DictWriter(f, fieldnames=fields + list(END_COLUMNS), lineterminator="\n")
        writer.writeheader()
        for row in rows:
            writer.writerow({**row, END_COLUMNS[0]: row[bc.COORD_COLUMNS[0]],
                             END_COLUMNS[1]: row[bc.COORD_COLUMNS[1]]})
    return out


def run_generator(csv_path, out_path, encoding, tmp_dir):
    """Run ride_austin_json_generator for one CSV and encoding; return (points, metadata, args)."""
    gen = bc.release_binary("ride_austin_json_generator")
    src = with_end_columns(csv_path, tmp_dir)
    out_path = Path(out_path)
    out_path.parent.mkdir(parents=True, exist_ok=True)
    args = ["--output-type", "0", *bc.ENCODINGS[encoding]["generator_args"]]
    proc = subprocess.run([str(gen), "--input", str(src), "--output", str(out_path), *args],
                          capture_output=True, text=True)
    if proc.returncode != 0 or not out_path.exists():
        raise SystemExit(f"ride_austin_json_generator failed for {csv_path} ({encoding}):\n"
                         f"{proc.stdout[-2000:]}\n{proc.stderr[-2000:]}")
    with open(bc.metadata_path(out_path), encoding="utf-8") as f:
        metadata = json.load(f)
    return bc.load_json_points(out_path), metadata, args


def main():
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--source-dir", required=True,
                        help="folder with sample_busiest_day.csv and start_busiest_day.json "
                             "(Mastic's data/mosaic)")
    args = parser.parse_args()
    source_dir = Path(args.source_dir)
    full_csv = source_dir / "sample_busiest_day.csv"
    full_json = source_dir / "start_busiest_day.json"
    for p in (full_csv, full_json):
        if not p.exists():
            raise SystemExit(f"{p} not found (--source-dir must hold Mosaic's day files)")

    print("Subsets against manifest.json:")
    ok, problems, _ = check_subsets(verbose=False)
    if not ok:
        raise SystemExit("verify_subsets fails; run bench/scripts/verify_subsets.py:\n  "
                         + "\n  ".join(problems))
    print("  PASS (verify_subsets.py)")

    manifest = bc.load_manifest()
    inputs = [(f"{manifest['dataset']}_{f['subset']}", bc.ROOT / f["path"], f["sha256"])
              for f in manifest["files"]]
    inputs.append((SMOKE_CSV.stem, SMOKE_CSV, bc.sha256_file(SMOKE_CSV)))

    failures = []

    def check(ok, label):
        print(f"  {'PASS' if ok else 'FAIL'}  {label}")
        if not ok:
            failures.append(label)

    entries, points = [], {}
    with tempfile.TemporaryDirectory() as tmp:
        print("\nConverting with ride_austin_json_generator:")
        for name, csv_path, csv_sha in inputs:
            rows = len(bc.read_points(csv_path))
            for enc, spec in bc.ENCODINGS.items():
                out = bc.encoded_path(enc, name)
                pts, meta, gen_args = run_generator(csv_path, out, enc, tmp)
                points[(name, enc)] = pts
                entries.append({
                    "encoding": enc,
                    "name": name,
                    "path": bc.relpath(out),
                    "metadata": bc.relpath(bc.metadata_path(out)),
                    "source": {"path": bc.relpath(csv_path), "sha256": csv_sha, "rows": rows},
                    "generator_args": gen_args,
                    "d": meta["d"],
                    "h1": meta["h1"],
                    "total_rides": meta["total_rides"],
                    "kept_rides": meta["kept_rides"],
                    "clamped_rides": meta["clamped_rides"],
                    "dropped_rides": meta["dropped_rides"],
                    "points_sha256": bc.points_sha256(pts),
                    "file_sha256": bc.sha256_file(out),
                })
                print(f"  {name:13} {enc:12} {meta['kept_rides']:6} kept, {meta['dropped_rides']} "
                      f"dropped, {meta['clamped_rides']} clamped, d = {meta['d']}, h1 = {meta['h1']}"
                      f"  -> {bc.relpath(out)}")
                # (f) every ride kept, unchanged; d and h1 as in ENCODINGS
                check(meta["total_rides"] == rows and meta["kept_rides"] == rows
                      and meta["dropped_rides"] == 0 and meta["clamped_rides"] == 0,
                      f"(f) {name} {enc}: all {rows} rides kept, none dropped or clamped")
                check((meta["d"], meta["h1"]) == (spec["d"], spec["h1"]),
                      f"(f) {name} {enc}: d, h1 = {spec['d']}, {spec['h1']}")

        print("\nFull day CSV (start_busiest_day.json check):")
        with open(full_json, encoding="utf-8") as f:
            reference_raw = json.load(f)
        reference = [[int(a), int(b)] for a, b in reference_raw]
        gen_full, _, _ = run_generator(full_csv, Path(tmp) / "full_day" / "start_busiest_day.json",
                                       "mosaic_grid", tmp)
        same = sum(a == b for a, b in zip(gen_full, reference))
        check(len(gen_full) == len(reference) == same,
              f"(a) generator(sample_busiest_day.csv) == start_busiest_day.json: {same} of "
              f"{len(reference)} points equal in file order")
        py_full = [list(bc.to_grid(lat, lon)) for lat, lon in bc.read_points(full_csv)]
        same_py = sum(a == b for a, b in zip(py_full, reference))
        check(len(py_full) == len(reference) == same_py,
              f"(b) Python grid formula(sample_busiest_day.csv) == start_busiest_day.json: "
              f"{same_py} of {len(reference)} points equal in file order")

    print("\nConverted inputs:")
    for name, csv_path, _ in inputs:
        py = [list(bc.to_grid(lat, lon)) for lat, lon in bc.read_points(csv_path)]
        grid = points[(name, "mosaic_grid")]
        check(py == grid, f"(c) {name}: Python grid formula == generator mosaic_grid "
                          f"({sum(a == b for a, b in zip(py, grid))} of {len(grid)})")
        sx, sy = bc.MORTON_GRID_SHIFT
        morton = points[(name, "morton")]
        check(len(morton) == len(grid) and all(m == [g[0] - sx, g[1] - sy]
                                               for m, g in zip(morton, grid)),
              f"(e) {name}: Morton == grid - ({sx}, {sy}) at every point")
        d = bc.H3_DERIVED
        h3 = points[(name, "h3")]
        check(all(p[d["index"]] == p[d["plus"]] + d["offset"] - p[d["minus"]] for p in h3),
              f"(g) {name}: every H3 point has k = i - j + {d['offset']}")
    full_name = f"{manifest['dataset']}_3of3"
    check(Counter(map(tuple, points[(full_name, "mosaic_grid")])) == Counter(map(tuple, reference)),
          f"(d) {full_name} grid points == start_busiest_day.json as a multiset")

    bc.write_json(bc.ENCODED_DIR / "manifest.json", {
        "generator": "target/release/ride_austin_json_generator (src/bin/data/ride_austin_json_generator.rs)",
        "source_dir_check": {
            "csv": full_csv.name,
            "csv_sha256": bc.sha256_file(full_csv),
            "json": full_json.name,
            "json_sha256": bc.sha256_file(full_json),
            "points": len(reference),
        },
        "checks_passed": not failures,
        "files": entries,
    })
    print(f"\nWrote {bc.relpath(bc.ENCODED_DIR / 'manifest.json')} ({len(entries)} files)")
    if failures:
        print("\n" + "=" * 72)
        print("convert_inputs: FAIL")
        for f in failures:
            print(f"  - {f}")
        print("=" * 72)
        raise SystemExit(1)
    print("convert_inputs: PASS")


if __name__ == "__main__":
    main()
