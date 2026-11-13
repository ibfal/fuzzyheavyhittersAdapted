#!/usr/bin/env python3
"""Mosaic's plaintext answer per encoding, subset and tau (B4), and the reference hotspots (A5).

    .\\.venv\\Scripts\\python.exe bench\\scripts\\ground_truth.py
    .\\.venv\\Scripts\\python.exe bench\\scripts\\ground_truth.py --check-rust
    .\\.venv\\Scripts\\python.exe bench\\scripts\\ground_truth.py --csv bench\\data\\ten_locations.csv --threshold 0.1

Without --csv: for every subset in bench/data/manifest.json (checked first), every tau in
bench_common.GROUND_TRUTH_TAUS (0.02 and 0.005) and every variant in bench_common.GT_VARIANTS
(the four encodings at delta 5, plus H3 at delta 4), the exact answer is written to
bench/ground_truth/<dataset>_<subset>_<variant>_tau<tau>.json, and the reference (A5: squares on
Mosaic's grid at tau 0.02, delta 5, with 8-neighbourhood areas and their peaks) to
bench/reference/<dataset>_<subset>.json. The input points are the converted subsets in
bench/data/encodings/ (convert_inputs.py), checked against its manifest.

With --csv and --threshold: any ride CSV, converted with Mosaic's generator into a temporary
folder, every variant, written to bench/ground_truth/smoke_<name>_<variant>_tau<tau>.json.

How the answer is computed (B2.4, B4). Two independent methods must agree exactly, points and
counts, or the script stops:

1. crawl(): Mosaic's unknown-dictionary crawl in plaintext, as the servers run it
   (src/fuzzy_match/protocol.rs:559-760; the same search as src/data_toolkit/crawl.rs:136-213 and,
   for configs without a derived coordinate, mosaic_others.rs:40-148). It also gives the number of
   prefixes checked and kept per level: the bits the servers reveal (A7).
2. dense(): the definition evaluated at every point of the domain, by summing shifted copies of
   the client histogram over the L-infinity neighbourhood (no prefix boxes, no integral image).

--check-rust also runs Mosaic's own plaintext, `mosaic_others ground-truth`, with a config equal to
ours and compares its points with ours. For H3 it runs it twice: with derived_coordinate (the
fork's crawl, crawl.rs) and without it (the authors' crawl of all three coordinates,
mosaic_others.rs:40-148), whose answer restricted to real cells must also equal ours.

Every point is decoded with Mosaic's `heavy_hitters_decoder` (cell label and centre); its Mosaic-grid
square is bench_common.to_grid(centre) (A5).
"""

import argparse
import csv
import json
import subprocess
import sys
import tempfile
import time
from collections import Counter, defaultdict
from pathlib import Path

import numpy as np

sys.dont_write_bytecode = True  # no __pycache__ in the repo
sys.path.insert(0, str(Path(__file__).resolve().parent))
import bench_common as bc  # noqa: E402
from verify_subsets import check_subsets  # noqa: E402

DEFINITION = (
    "Mosaic's unknown-dictionary fuzzy heavy hitters (B2.4): every point x of [0, 2^h1)^d with "
    "#{clients y : max_i |x_i - y_i| <= delta} >= T. All points of the domain are candidates "
    "(mosaic_others.rs:50,124-134); count and >= at mosaic_others.rs:77-84; L-infinity and its "
    "bound delta at util.rs:187-199,279-281; T absolute (cli_config.rs:28-29). The servers find it "
    "with the crawl of protocol.rs:559-760: a prefix box is kept if #{y : lo_i - delta <= y_i <= "
    "hi_i + delta for all i} >= T (the optimistic bound of util.rs:255-276; client intervals are "
    "clipped to the domain, share_phase.rs:58-64, so there is no wrap-around). H3: the crawl "
    "branches on i and j only (cli_config.rs:89-92); each full point gets its only possible "
    "k = i - j + 1024, is dropped if k is outside [0, 2^h1) (cli_config.rs:95-98), and gets a final "
    "check with all three coordinates (protocol.rs:768-840), so the answer is the set of H3 cells "
    "with >= T clients within delta hexagon steps."
)

CHUNK = 512  # candidate boxes per vectorised brute-force count


# --- Clients -----------------------------------------------------------------------------------


class Clients:
    """The client points of one input: unique points with multiplicities, and an integral image of
    coordinates 0 and 1 for box counts."""

    def __init__(self, points, d, h1):
        pts = np.asarray(points, dtype=np.int64).reshape(-1, d)
        self.n, self.d, self.h1, self.size = len(pts), d, h1, 1 << h1
        if len(pts) and (pts.min() < 0 or pts.max() >= self.size):
            raise SystemExit(f"client coordinates outside [0, 2^{h1})")
        self.unique, self.weight = np.unique(pts, axis=0, return_counts=True)
        self.weight = self.weight.astype(np.int64)
        # Histogram of coordinates (0, 1) and its integral image S[x, y] = sum H[< x, < y].
        self.hist2 = np.zeros((self.size, self.size), dtype=np.int64)
        np.add.at(self.hist2, (self.unique[:, 0], self.unique[:, 1]), self.weight)
        self.integral = np.zeros((self.size + 1, self.size + 1), dtype=np.int64)
        self.integral[1:, 1:] = self.hist2.cumsum(0).cumsum(1)

    def box_counts(self, lo, hi):
        """Clients y with lo_i <= y_i <= hi_i for every i, for each row of lo / hi."""
        lo, hi = np.asarray(lo, np.int64), np.asarray(hi, np.int64)
        if len(lo) == 0:
            return np.zeros(0, np.int64)
        # Boxes that cover every value of the coordinates beyond the first two (the H3 crawl with
        # its derived coordinate still at the empty prefix) need only coordinates 0 and 1.
        rest_full = (lo[:, 2:] <= 0).all(1) & (hi[:, 2:] >= self.size - 1).all(1)
        out = np.empty(len(lo), np.int64)
        if rest_full.any():
            out[rest_full] = self._integral_counts(lo[rest_full], hi[rest_full])
        if (~rest_full).any():
            out[~rest_full] = self._brute_counts(lo[~rest_full], hi[~rest_full])
        return out

    def _integral_counts(self, lo, hi):
        m = self.size - 1
        x1, x2 = np.clip(lo[:, 0], 0, m), np.clip(hi[:, 0], 0, m)
        y1, y2 = np.clip(lo[:, 1], 0, m), np.clip(hi[:, 1], 0, m)
        s = self.integral
        res = s[x2 + 1, y2 + 1] - s[x1, y2 + 1] - s[x2 + 1, y1] + s[x1, y1]
        return np.where((x1 <= x2) & (y1 <= y2), res, 0)

    def _brute_counts(self, lo, hi):
        out = np.empty(len(lo), np.int64)
        for i in range(0, len(lo), CHUNK):
            a, b = lo[i:i + CHUNK, None, :], hi[i:i + CHUNK, None, :]
            inside = ((self.unique[None] >= a) & (self.unique[None] <= b)).all(-1)
            out[i:i + CHUNK] = inside @ self.weight
        return out


# --- Method 1: the crawl -----------------------------------------------------------------------


def crawl(clients, delta, threshold, derived):
    """Mosaic's unknown-dictionary crawl in plaintext; returns (points, counts, levels).

    levels: [{"level", "dim", "checked", "kept"}], one entry per level the servers run.
    """
    d, h1, size = clients.d, clients.h1, clients.size
    # The coordinates the crawl branches on: all of them, or all but the derived one
    # (DerivedCoordinate::crawl_dims, cli_config.rs:89-92; protocol.rs:584).
    dims = [k for k in range(d) if derived is None or k != derived["index"]]
    # A prefix stands for the box of the points that extend it (crawl.rs:56-64). The crawl starts
    # from the empty prefix of every coordinate, the whole domain (protocol.rs:594-599).
    lo = np.zeros((1, d), np.int64)
    hi = np.full((1, d), size - 1, np.int64)
    counts = np.zeros(1, np.int64)
    levels = []
    for level in range(1, len(dims) * h1 + 1):
        # One bit of one coordinate per level, the coordinates in turn (protocol.rs:630-631,
        # crawl.rs:151); each prefix gets the children for bit 0 and bit 1 (protocol.rs:633-655).
        dim = dims[(level - 1) % len(dims)]
        half = (hi[:, dim] - lo[:, dim] + 1) // 2
        child_lo = np.repeat(lo, 2, axis=0)
        child_hi = np.repeat(hi, 2, axis=0)
        child_hi[0::2, dim] = lo[:, dim] + half - 1
        child_lo[1::2, dim] = lo[:, dim] + half
        # A child's count: the clients whose delta-box meets the child's box, i.e. within delta
        # of the box's closest point (util.rs:255-276, crawl.rs:102-115). Coordinates still at
        # the empty prefix count every client as inside (share_phase.rs:135-138).
        counts = clients.box_counts(child_lo - delta, child_hi + delta)
        keep = counts >= threshold  # kept iff count >= T (mosaic_others.rs:121, protocol.rs:701-720)
        levels.append({"level": level, "dim": dim, "checked": len(child_lo),
                       "kept": int(keep.sum())})
        lo, hi, counts = child_lo[keep], child_hi[keep], counts[keep]
        if len(lo) == 0:  # an empty frontier ends the crawl (protocol.rs:605)
            return np.zeros((0, d), np.int64), np.zeros(0, np.int64), levels
    if derived is None:
        # Full points: the box is one point and its count is the exact neighbourhood count
        # (mosaic_others.rs:77-84).
        assert (lo == hi).all()
        return lo, counts, levels
    # Derived coordinate (protocol.rs:768-840, crawl.rs:189-212): every full point gets its only
    # possible value k = point[plus] + offset - point[minus], is dropped if k is outside
    # [0, 2^h1) (DerivedCoordinate::value, cli_config.rs:95-98), and is checked once more with
    # every coordinate.
    assert (lo[:, dims] == hi[:, dims]).all()
    value = lo[:, derived["plus"]] + derived["offset"] - lo[:, derived["minus"]]
    valid = (value >= 0) & (value < size)
    points = lo[valid].copy()
    points[:, derived["index"]] = value[valid]
    counts = clients.box_counts(points - delta, points + delta)
    keep = counts >= threshold
    levels.append({"level": len(dims) * h1 + 1, "dim": derived["index"],
                   "checked": int(valid.sum()), "kept": int(keep.sum())})
    return points[keep], counts[keep], levels


# --- Method 2: the definition at every point ---------------------------------------------------


def _add_shifted(acc, hist, da, db):
    """acc[x, y] += hist[x + da, y + db] where both are inside the array (no wrap-around)."""
    n = hist.shape[0]
    acc[max(0, -da):n - max(0, da), max(0, -db):n - max(0, db)] += \
        hist[max(0, da):n - max(0, -da), max(0, db):n - max(0, -db)]


def dense(clients, delta, threshold, derived):
    """The definition evaluated at every point of the domain; returns (points, counts).

    2D: count(x) = sum of the client histogram over the (2 delta + 1)^2 square around x.
    H3 (derived coordinate): only real cells (i, j, i - j + o) can be outputs; a client y is within
    delta of the cell x iff |di| <= delta, |dj| <= delta and |di - dj| <= delta (the third
    coordinate's difference, since every client point has k = i - j + o: convert_inputs.py check
    (g)), i.e. within delta hexagon steps.
    """
    d, size = clients.d, clients.size
    hist = clients.hist2.astype(np.int32)
    acc = np.zeros_like(hist)
    for da in range(-delta, delta + 1):
        for db in range(-delta, delta + 1):
            if derived is not None and abs(da - db) > delta:
                continue
            _add_shifted(acc, hist, da, db)
    mask = acc >= threshold
    if derived is None:
        if d != 2:
            raise SystemExit("dense(): only 2 coordinates, or 3 with a derived coordinate")
        pts = np.argwhere(mask)
        return pts.astype(np.int64), acc[mask].astype(np.int64)
    if d != 3 or derived["index"] != 2:
        raise SystemExit("dense(): the derived coordinate must be coordinate 2 of 3")
    a, b = np.nonzero(mask)
    k = a + derived["offset"] - b
    valid = (k >= 0) & (k < size)
    pts = np.stack([a[valid], b[valid], k[valid]], axis=1).astype(np.int64)
    return pts, acc[a[valid], b[valid]].astype(np.int64)


def as_rows(points, counts):
    """Sorted [[coords..., count]] rows."""
    rows = [[*map(int, p), int(c)] for p, c in zip(points, counts)]
    rows.sort()
    return rows


# --- Mosaic's own binaries ---------------------------------------------------------------------


def rust_ground_truth(points_path, d, h1, delta, threshold, n, derived):
    """The points `mosaic_others ground-truth` (the plaintext in this repo) finds for this input."""
    exe = bc.release_binary("mosaic_others")
    protocol = {
        "delta": delta, "match_threshold": threshold, "enable_sketch": True, "h1": h1, "h2": 1,
        "h3": max(1, n.bit_length()), "d": d, "share_method": "FSS",
        "dictionary_type": "Unknown", "check_property": "Equality", "distance_metric": "Linf",
        "num_clients": n, "sketch_modulus": 892270022585185806328177,
    }
    if derived is not None:
        protocol["derived_coordinate"] = derived
    with tempfile.TemporaryDirectory() as tmp:
        out = Path(tmp) / "ground_truth.json"
        cfg = Path(tmp) / "config.json"
        cfg.write_text(json.dumps({
            "data_file": str(Path(points_path).resolve()),
            "query_file": str(Path(points_path).resolve()),
            "protocol": protocol,
            "output": {"verbose": False, "show_intermediate": False, "output_file": str(out)},
        }), encoding="utf-8")
        start = time.perf_counter()
        proc = subprocess.run([str(exe), "ground-truth", "--config", str(cfg)],
                              stdout=subprocess.DEVNULL, stderr=subprocess.PIPE, text=True)
        seconds = time.perf_counter() - start
        if proc.returncode != 0 or not out.exists():
            raise SystemExit(f"mosaic_others failed for {points_path}:\n{proc.stderr[-2000:]}")
        with open(out, encoding="utf-8") as f:
            found = json.load(f)["heavy_hitters"]
    return {tuple(int(v) for v in p) for p in found}, seconds


def decode(points, metadata_file):
    """{coords: (label, lat, lon)} for the points that are cells, by heavy_hitters_decoder."""
    if len(points) == 0:
        return {}
    exe = bc.release_binary("heavy_hitters_decoder")
    with tempfile.TemporaryDirectory() as tmp:
        inp, out = Path(tmp) / "points.json", Path(tmp) / "decoded.csv"
        inp.write_text(json.dumps([[int(v) for v in p] for p in points]), encoding="utf-8")
        proc = subprocess.run([str(exe), "--input", str(inp), "--metadata", str(metadata_file),
                               "--output", str(out)], capture_output=True, text=True)
        if proc.returncode != 0:
            raise SystemExit(f"heavy_hitters_decoder failed:\n{proc.stderr[-2000:]}")
        decoded = {}
        with open(out, newline="", encoding="utf-8") as f:
            for row in csv.DictReader(f):
                coords = tuple(int(v) for v in row["point"].split())
                decoded[coords] = (row["cell_0"], float(row["lat_0"]), float(row["lon_0"]))
    return decoded


def h3_parent(label, res):
    """The H3 index string of a cell's parent at resolution `res` (H3 index bit layout: resolution
    in bits 52-55, the digit of resolution r in bits 3 (15 - r) to 3 (15 - r) + 2, unused digits 7)."""
    h = int(label, 16)
    cur = (h >> 52) & 0xF
    h = (h & ~(0xF << 52)) | (res << 52)
    for r in range(res + 1, cur + 1):
        h |= 0x7 << ((15 - r) * 3)
    return f"{h:x}"


H3_A2_PREFIX = "82489ffffffffff"  # A2's H3 prefix: the resolution-2 cell Mastic encodes below


# --- One answer --------------------------------------------------------------------------------


def answer(points_path, metadata_file, variant, tau, check_rust):
    """Everything about one input x variant x tau; stops on any disagreement."""
    var = bc.GT_VARIANTS[variant]
    spec = bc.ENCODINGS[var["encoding"]]
    delta = var["delta"]
    derived = spec["params"].get("derived_coordinate")
    pts = bc.load_json_points(points_path)
    clients = Clients(pts, spec["d"], spec["h1"])
    n = clients.n
    threshold = bc.threshold_abs(n, tau)
    label = f"{Path(points_path).stem} {variant} tau {bc.tau_tag(tau)}"

    t0 = time.perf_counter()
    c_pts, c_counts, levels = crawl(clients, delta, threshold, derived)
    t1 = time.perf_counter()
    d_pts, d_counts = dense(clients, delta, threshold, derived)
    t2 = time.perf_counter()
    rows = as_rows(c_pts, c_counts)
    if rows != as_rows(d_pts, d_counts):
        raise SystemExit(f"{label}: crawl ({len(c_pts)} points) and dense ({len(d_pts)} points) "
                         f"disagree; the replication is wrong")
    checks = {"crawl_equals_dense": True}
    # Timings are printed, not written, so the files are reproducible byte for byte.
    timings = {"crawl": t1 - t0, "dense": t2 - t1}

    if check_rust:
        found, secs = rust_ground_truth(points_path, spec["d"], spec["h1"], delta, threshold, n,
                                        derived)
        ours = {tuple(r[:-1]) for r in rows}
        if found != ours:
            raise SystemExit(f"{label}: mosaic_others finds {len(found)} points, we find "
                             f"{len(ours)}; {len(found - ours)} only in Rust, "
                             f"{len(ours - found)} only here")
        checks["mosaic_others"] = {"equal": True, "points": len(found)}
        timings["mosaic_others"] = secs
        if derived is not None:
            # The authors' crawl of all three coordinates (no derived_coordinate in the config):
            # its points that are cells must be ours (the fork's equivalence claim, 5623af8).
            found3, secs3 = rust_ground_truth(points_path, spec["d"], spec["h1"], delta,
                                              threshold, n, None)
            k, plus, minus, off = (derived[x] for x in ("index", "plus", "minus", "offset"))
            cells3 = {p for p in found3 if p[k] == p[plus] + off - p[minus]}
            if cells3 != ours:
                raise SystemExit(f"{label}: the 3-coordinate crawl finds {len(cells3)} cells, "
                                 f"we find {len(ours)}")
            checks["mosaic_others_without_derived"] = {
                "cells_equal": True, "points": len(found3), "cells": len(cells3),
                "non_cells": len(found3) - len(cells3)}
            timings["mosaic_others_without_derived"] = secs3

    decoded = decode([r[:-1] for r in rows], metadata_file)
    cells = []
    for r in rows:
        coords = tuple(r[:-1])
        if coords in decoded:
            cell_label, lat, lon = decoded[coords]
            cells.append({"coords": list(coords), "label": cell_label, "lat": lat, "lon": lon,
                          "grid": list(bc.to_grid(lat, lon)), "count": r[-1]})
    non_cells = len(rows) - len(cells)
    if var["encoding"] == "h3":
        parents = Counter(h3_parent(c["label"], 2) for c in cells)
        checks["h3_res2_parents"] = dict(parents)
        if set(parents) - {H3_A2_PREFIX}:
            raise SystemExit(f"{label}: H3 cells outside {H3_A2_PREFIX}: {dict(parents)}")
    if non_cells:
        raise SystemExit(f"{label}: {non_cells} points are not cells (expected none)")

    return timings, {
        "variant": variant,
        "encoding": var["encoding"],
        "threshold_frac": tau,
        "N": n,
        "T": threshold,
        "delta": delta,
        "metric": "Linf",
        "d": spec["d"],
        "h1": spec["h1"],
        "data_bits": spec["data_bits"],
        "encoding_params": spec["params"],
        "definition": DEFINITION,
        "summary": {
            "points": len(rows),
            "cells": len(cells),
            "non_cells": non_cells,
            "unique_client_points": len(clients.unique),
            "levels": len(levels),
            "prefixes_checked": sum(lv["checked"] for lv in levels),
        },
        "checks": checks,
        # A7: every checked prefix reveals one bit to both servers (protocol.rs:994-1073), so
        # revealed_aggregates_per_level = candidates_per_level; frontier_per_level = kept.
        "candidates_per_level": [lv["checked"] for lv in levels],
        "frontier_per_level": [lv["kept"] for lv in levels],
        "level_dims": [lv["dim"] for lv in levels],
        "points": rows,
        "cells": cells,
    }


# --- Reference (A5) ----------------------------------------------------------------------------


def reference(grid_answer, dataset, subset, source):
    """Q with neighbourhood counts, split into 8-neighbourhood areas with their peak squares.

    Areas are numbered by size (largest first), ties by peak count (higher first), then by the
    peak's grid coordinates; an area's peak is its highest count, ties by the smallest coordinates.
    """
    assert grid_answer["encoding"] == "mosaic_grid"
    count = {tuple(r[:2]): r[2] for r in grid_answer["points"]}
    seen, areas = set(), []
    for start in sorted(count):
        if start in seen:
            continue
        seen.add(start)
        stack, members = [start], []
        while stack:
            sq = stack.pop()
            members.append(sq)
            for da in (-1, 0, 1):
                for db in (-1, 0, 1):
                    nb = (sq[0] + da, sq[1] + db)
                    if nb in count and nb not in seen:
                        seen.add(nb)
                        stack.append(nb)
        peak = min(members, key=lambda s: (-count[s], s))
        areas.append({"members": members, "peak": peak, "peak_count": count[peak]})
    areas.sort(key=lambda a: (-len(a["members"]), -a["peak_count"], a["peak"]))
    area_of = {}
    for i, a in enumerate(areas):
        for sq in a["members"]:
            area_of[sq] = i
    return {
        "dataset": dataset,
        "subset": subset,
        "tau": grid_answer["threshold_frac"],
        "T": grid_answer["T"],
        "delta": grid_answer["delta"],
        "N": grid_answer["N"],
        "source": source,
        "squares": [{"grid": list(sq), "count": count[sq], "area": area_of[sq]}
                    for sq in sorted(count)],
        "areas": [{"id": i, "peak": list(a["peak"]), "peak_count": a["peak_count"],
                   "size": len(a["members"])} for i, a in enumerate(areas)],
    }


# --- CLI ---------------------------------------------------------------------------------------


def encoded_inputs():
    """{(name, encoding): manifest entry} of bench/data/encodings, each file checked."""
    path = bc.ENCODED_DIR / "manifest.json"
    if not path.exists():
        raise SystemExit(f"{bc.relpath(path)} missing: run bench/scripts/convert_inputs.py first")
    with open(path, encoding="utf-8") as f:
        manifest = json.load(f)
    if not manifest.get("checks_passed"):
        raise SystemExit("convert_inputs.py's checks did not pass; rerun it")
    entries = {}
    for e in manifest["files"]:
        pts = bc.load_json_points(bc.ROOT / e["path"])
        if bc.points_sha256(pts) != e["points_sha256"]:
            raise SystemExit(f"{e['path']} changed since convert_inputs.py (points_sha256)")
        entries[(e["name"], e["encoding"])] = e
    return entries


def write_answer(path, header, ans):
    bc.write_json_rows(path, {**header, **ans}, row_keys={"points", "cells"})


def print_counts(table):
    """table: {column label: {variant: answer}}."""
    cols = list(table)
    width = max(14, *(len(c) for c in cols)) + 2
    print(f"{'variant':12}" + "".join(f"{c:>{width}}" for c in cols))
    for variant in bc.GT_VARIANTS:
        cells = []
        for c in cols:
            a = table[c][variant]
            cells.append(f"{a['summary']['points']} (T={a['T']})")
        print(f"{variant:12}" + "".join(f"{x:>{width}}" for x in cells))


def run_smoke(csv_path, tau, check_rust):
    from convert_inputs import run_generator

    csv_path = Path(csv_path).resolve()
    sha = bc.sha256_file(csv_path)
    print(f"{bc.relpath(csv_path)} at threshold {tau}:")
    with tempfile.TemporaryDirectory() as tmp:
        for variant, var in bc.GT_VARIANTS.items():
            enc = var["encoding"]
            out = Path(tmp) / enc / f"{csv_path.stem}.json"
            if not out.exists():
                run_generator(csv_path, out, enc, tmp)
            _, ans = answer(out, bc.metadata_path(out), variant, tau, check_rust)
            header = {"schema_version": bc.SCHEMA_VERSION, "system": bc.SYSTEM,
                      "kind": "ground_truth_smoke", "dataset": None, "subset": "smoke",
                      "source": {"path": bc.relpath(csv_path), "sha256": sha}}
            dest = bc.GROUND_TRUTH_DIR / f"smoke_{csv_path.stem}_{variant}_tau{bc.tau_tag(tau)}.json"
            write_answer(dest, header, ans)
            hist = Counter(r[-1] for r in ans["points"])
            print(f"  {variant:12} N={ans['N']} T={ans['T']} delta={ans['delta']}: "
                  f"{ans['summary']['points']} points ({ans['summary']['cells']} cells, "
                  f"{ans['summary']['non_cells']} non-cells), counts {dict(sorted(hist.items()))}, "
                  f"{ans['summary']['prefixes_checked']} prefixes in {ans['summary']['levels']} "
                  f"levels -> {bc.relpath(dest)}")


def main():
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--csv", help="any ride CSV (smoke test) instead of the subsets")
    parser.add_argument("--threshold", type=float, help="threshold fraction tau for --csv")
    parser.add_argument("--check-rust", action="store_true",
                        help="also compare with Mosaic's own `mosaic_others ground-truth`")
    args = parser.parse_args()

    if args.csv:
        if args.threshold is None:
            parser.error("--csv needs --threshold")
        run_smoke(args.csv, args.threshold, args.check_rust)
        return

    ok, problems, _ = check_subsets(verbose=False)
    if not ok:
        raise SystemExit("verify_subsets fails; run bench/scripts/verify_subsets.py:\n  "
                         + "\n  ".join(problems))
    entries = encoded_inputs()
    manifest = bc.load_manifest()
    dataset = manifest["dataset"]
    table = defaultdict(dict)
    answers = {}
    for f in manifest["files"]:
        subset, name = f["subset"], f"{dataset}_{f['subset']}"
        source = {"path": f["path"], "sha256": f["sha256"]}
        for tau in bc.GROUND_TRUTH_TAUS:
            for variant, var in bc.GT_VARIANTS.items():
                e = entries[(name, var["encoding"])]
                timings, ans = answer(bc.ROOT / e["path"], bc.ROOT / e["metadata"], variant, tau,
                                      args.check_rust)
                header = {"schema_version": bc.SCHEMA_VERSION, "system": bc.SYSTEM,
                          "kind": "ground_truth", "dataset": dataset, "subset": subset,
                          "source": source,
                          "input": {"path": e["path"], "points_sha256": e["points_sha256"]}}
                dest = bc.ground_truth_path(dataset, subset, variant, tau)
                write_answer(dest, header, ans)
                answers[(subset, tau, variant)] = ans
                table[f"{subset} tau {bc.tau_tag(tau)}"][variant] = ans
                s = ans["summary"]
                rust = "".join(f", {k} equal ({v:.1f} s)" for k, v in timings.items()
                               if k.startswith("mosaic_others"))
                print(f"{bc.relpath(dest)}: {s['points']} points, {s['prefixes_checked']} prefixes "
                      f"in {s['levels']} levels (crawl {timings['crawl']:.2f} s, dense "
                      f"{timings['dense']:.2f} s){rust}")

            # Morton cells are the grid squares shifted by (634, 693) (morton.rs:10-12).
            sx, sy = bc.MORTON_GRID_SHIFT
            grid = answers[(subset, tau, "mosaic_grid")]["points"]
            morton = answers[(subset, tau, "morton")]["points"]
            if sorted([a + sx, b + sy, c] for a, b, c in morton) != grid:
                raise SystemExit(f"{subset} tau {tau}: Morton is not the grid shifted by "
                                 f"({sx}, {sy})")

        ref = reference(answers[(subset, bc.REFERENCE_TAU, "mosaic_grid")], dataset, subset, source)
        dest = bc.reference_path(dataset, subset)
        bc.write_json_rows(dest, ref, row_keys={"squares", "areas"})
        print(f"{bc.relpath(dest)}: {len(ref['squares'])} squares in {len(ref['areas'])} areas")

    print("\nQualifying points per variant (all are cells; H3 counts hexagons):")
    print_counts(table)
    print("\nPrefixes checked (= bits revealed to the servers, A7) per variant:")
    cols = list(table)
    print(f"{'variant':12}" + "".join(f"{c:>18}" for c in cols))
    for variant in bc.GT_VARIANTS:
        print(f"{variant:12}" + "".join(
            f"{table[c][variant]['summary']['prefixes_checked']:>18}" for c in cols))
    print("\nReference (A5, grid, tau 0.02, delta 5):")
    for f in manifest["files"]:
        with open(bc.reference_path(dataset, f["subset"]), encoding="utf-8") as fh:
            ref = json.load(fh)
        print(f"  {dataset}_{f['subset']} (N={ref['N']}, T={ref['T']}): {len(ref['squares'])} "
              f"squares, {len(ref['areas'])} areas: " + "; ".join(
                  f"#{a['id']} size {a['size']} peak {tuple(a['peak'])} count {a['peak_count']}"
                  for a in ref["areas"]))

    # The README's figures for the busiest day (FSS configs, T = 431): informational.
    full = "3of3"
    expect = [("mosaic_grid", "points", 916), ("morton", "points", 916),
              ("geohash", "points", 891), ("h3_delta4", "points", 445),
              ("h3_delta4", "levels", 23), ("h3_delta4", "prefixes_checked", 2577)]
    print(f"\nREADME figures, {dataset}_{full} tau 0.02 (README.md:97, :649, :655, :673):")
    for variant, key, want in expect:
        got = answers[(full, 0.02, variant)]["summary"][key]
        print(f"  {'PASS' if got == want else 'DIFF'}  {variant} {key}: {got} (README {want})")
    if not args.check_rust:
        print("\n(Run with --check-rust to compare every answer with mosaic_others.)")


if __name__ == "__main__":
    main()
