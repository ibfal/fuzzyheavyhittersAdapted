#!/usr/bin/env python3
"""Shared helpers for the Mosaic benchmark (bench/BENCHMARK_MOSAIC.md).

Copied from the Mastic harness (mastic_for_fuzzy_HH/bench/scripts/bench_common.py, spec v3) and
adapted where it was Mastic-specific:

- The experiment's parameters in one place (A2) for Mosaic: the four encodings with Mosaic's
  d, h1, crawl levels and delta (B2.7), the per-system thresholds (Mosaic: tau = 0.02), the
  decided method (FSS shares, GC check, GC threshold, sketching on).
- Mosaic's grid formula (A1, A5) and the release binaries, instead of Mastic's plaintext encoders.
- The comparison of the repo's Mosaic configs with these parameters.
- The A9 schema check with the spec v3.1 additions (per-direction server_to_server, nullable
  count / lat / lon, optional `extra`).

Unchanged from Mastic: the environment record (A8), file helpers and the JSONL append helper.

    .\\.venv\\Scripts\\python.exe bench\\scripts\\bench_common.py              # environment, config check
    .\\.venv\\Scripts\\python.exe bench\\scripts\\bench_common.py --self-test  # schema check self-test
"""

import argparse
import copy
import csv
import datetime
import hashlib
import json
import math
import os
import platform
import shutil
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
BENCH = ROOT / "bench"
DATA_DIR = BENCH / "data"
ENCODED_DIR = DATA_DIR / "encodings"  # the subsets as Mosaic's JSON points (convert_inputs.py)
GROUND_TRUTH_DIR = BENCH / "ground_truth"
REFERENCE_DIR = BENCH / "reference"

SYSTEM = "mosaic"
SCHEMA_VERSION = 1
DATASET = "day"
SEED = 20261107
SUBSETS = ("1of3", "2of3", "3of3")
# Threshold fraction tau per system (A2): each system runs at its own native threshold.
# Mastic: in-cell counts, the existing day configs. Mosaic: neighbourhood counts, Mosaic's code and
# its results.json (optionally also run at Mastic's 0.005). T = max(1, floor(tau * N)).
THRESHOLDS = {"mastic": 0.005, "mosaic": 0.02}
THRESHOLD_FRAC = THRESHOLDS[SYSTEM]  # this codebase's tau: the default of every script
# The ground truth covers Mosaic's native tau and the optional matched runs at Mastic's tau (B4).
GROUND_TRUTH_TAUS = (THRESHOLDS["mosaic"], THRESHOLDS["mastic"])
DELTA = 5  # Mosaic's radius in grid cells, L-infinity (data/mosaic/results.json)
# The reference hotspots (A5): Mosaic's qualifying squares at Mosaic's native tau and delta.
REFERENCE_TAU = THRESHOLDS["mosaic"]
REFERENCE_DELTA = DELTA
COORD_COLUMNS = ("start_location_lat", "start_location_long")

# The method every Mosaic run uses (decided in prompt 1): FSS client shares (needed for
# sketching, client.rs:64-66), garbled circuits for the distance check and the threshold
# comparison (configs/method/gc_gc.json), sketch verification of the client keys on.
METHOD = {"share_method": "FSS", "check_method": "GC", "threshold_method": "GC",
          "enable_sketch": True, "distance_metric": "Linf", "check_property": "Equality",
          "dictionary_type": "Unknown"}
METHOD_CONFIG = ROOT / "configs" / "method" / "gc_gc.json"

# H3 coordinates are [i + o, j + o, i - j + o] with o = 2^(h1 - 1) (src/data_toolkit/encoding/h3.rs:9-12).
# The third one is derived from the others, so the crawl skips it and checks it at the full points
# (src/configs/cli_config.rs:51-99, src/fuzzy_match/protocol.rs:768-840).
H3_DERIVED = {"index": 2, "plus": 0, "minus": 1, "offset": 1024}

# Encoding parameters (A2, B2.7).
#   d, h1:          coordinates per point and bits per coordinate (the configs' d and h1)
#   data_bits:      crawl levels n (A2 note): d * h1, or (d - 1) * h1 + 1 with a derived coordinate
#   delta:          the radius the runs use (A2: 5 for every encoding; for H3 in hexagon steps)
#   params:         `params.encoding_params` of a result line, as in the Rust `Encoding` enum
#                   (src/data_toolkit/encoding/mod.rs:31-61) plus H3's derived coordinate
#   generator_args: ride_austin_json_generator flags, every parameter explicit
#                   (defaults in src/data_toolkit/encoding/cli.rs:19-61)
#   config_dir:     the repo's own day configs for this encoding (compared in compare_configs)
ENCODINGS = {
    "mosaic_grid": {
        "d": 2, "h1": 11, "data_bits": 22, "delta": DELTA,
        "params": {},
        "generator_args": ["--encoding", "mosaic-grid"],
        "config_dir": "configs/unknown_dictionary",
    },
    "geohash": {
        "d": 2, "h1": 11, "data_bits": 22, "delta": DELTA,
        "params": {"prefix": "9v6", "bits": 21},
        "generator_args": ["--encoding", "geohash", "--geohash-prefix=9v6", "--geohash-bits=21"],
        "config_dir": "configs/encodings/geohash",
    },
    "h3": {
        "d": 3, "h1": 11, "data_bits": 23, "delta": DELTA,
        "params": {"resolution": 10, "origin_lat": 30.267, "origin_lon": -97.743, "axis_bits": 11,
                   "derived_coordinate": H3_DERIVED},
        "generator_args": ["--encoding", "h3", "--h3-resolution=10", "--h3-origin-lat=30.267",
                           "--h3-origin-lon=-97.743", "--h3-axis-bits=11"],
        "config_dir": "configs/encodings/h3",
    },
    "morton": {
        "d": 2, "h1": 10, "data_bits": 20, "delta": DELTA,
        "params": {"min_lat": 29.9005, "max_lat": 30.9245, "min_lon": -98.0505,
                   "max_lon": -97.0265, "axis_bits": 10},
        "generator_args": ["--encoding", "morton", "--morton-min-lat=29.9005",
                           "--morton-max-lat=30.9245", "--morton-min-lon=-98.0505",
                           "--morton-max-lon=-97.0265", "--morton-axis-bits=10"],
        "config_dir": "configs/encodings/morton",
    },
}

# Ground-truth variants (B4): every encoding at its run delta, plus H3 at Mosaic's own default
# delta = 4 (A2: computed in the ground truth, run only if time allows).
GT_VARIANTS = {
    "mosaic_grid": {"encoding": "mosaic_grid", "delta": 5},
    "geohash": {"encoding": "geohash", "delta": 5},
    "h3": {"encoding": "h3", "delta": 5},
    "h3_delta4": {"encoding": "h3", "delta": 4},
    "morton": {"encoding": "morton", "delta": 5},
}

# The default Morton box gives Mosaic's grid cells shifted by this much (morton.rs:10-12).
MORTON_GRID_SHIFT = (634, 693)

# Smoke runs (B7): bench/data/ten_locations.csv (copied from the Mastic repo) at threshold 0.1.
# Their result lines carry this dataset and subset. The schema's subset list holds only the
# benchmark subsets, so run_bench.py checks smoke lines with the subset swapped (as Mastic does).
SMOKE_DATASET, SMOKE_SUBSET = "synthetic", "smoke"
SMOKE_CSV = DATA_DIR / "ten_locations.csv"
SMOKE_THRESHOLD = 0.1

# The H3 crawl (prompt 2 decision): "derived" = the repo's crawl that skips the derived coordinate
# k = i - j + o and checks it at the end (23 levels, the main runs); "original" = the authors'
# crawl of all three coordinates (33 levels; no derived_coordinate in the config).
H3_CRAWLS = ("derived", "original")

# The listening side of every Mosaic channel allocates a reader and a writer buffer of this size
# (src/channel.rs, environment variable MOSAIC_CHANNEL_BUFFER_BYTES; unset: the original 1 GiB).
# The runner sets 64 MiB (prompt 2 decision) and records it in extra.channel_buffer_bytes.
CHANNEL_BUFFER_ENV = "MOSAIC_CHANNEL_BUFFER_BYTES"
CHANNEL_BUFFER_BYTES = 64 * 2**20

# Protocol parameters every run config gets besides the encoding's d, h1, delta and T: the values
# of the repo's day configs (configs/unknown_dictionary/busiest_day/linf_fss_config.json). h3 is
# the bit length of the count ring; 15 bits hold every N of the benchmark (2^15 > 21,581).
PROTOCOL_BASE = {"h2": 1, "h3": 15, "sketch_modulus": 892270022585185806328177}

# The client micro-benchmark (A6): the first K rows of day_3of3.csv.
CLIENT_BENCH_ROWS = 2000


def gt_variant(encoding, delta):
    """The ground-truth variant of a run: the encoding, or h3_delta4 for H3 at delta 4."""
    if encoding == "h3" and delta != ENCODINGS["h3"]["delta"]:
        variant = f"h3_delta{delta}"
        if variant not in GT_VARIANTS:
            raise ValueError(f"no ground truth for H3 at delta {delta}")
        return variant
    return encoding

BUILD_COMMAND = ("cargo build --release --bin ride_austin_json_generator "
                 "--bin heavy_hitters_decoder --bin mosaic_others")


def threshold_abs(n, tau):
    """T = max(1, floor(tau * n)), the driver's rule. tau is always explicit: the two systems run
    at different thresholds (A2)."""
    return max(1, int(tau * n))


def tau_tag(tau):
    """tau as it appears in file names: 0.02 -> "0.02", 0.005 -> "0.005"."""
    return f"{tau:g}"


def ground_truth_path(dataset, subset, variant, tau):
    """bench/ground_truth/<dataset>_<subset>_<variant>_tau<tau>.json (B4)."""
    return GROUND_TRUTH_DIR / f"{dataset}_{subset}_{variant}_tau{tau_tag(tau)}.json"


def reference_path(dataset, subset):
    """bench/reference/<dataset>_<subset>.json (A5)."""
    return REFERENCE_DIR / f"{dataset}_{subset}.json"


def encoded_path(encoding, name):
    """bench/data/encodings/<encoding>/<name>.json, written by convert_inputs.py. The generator
    writes the encoding metadata next to it as <name>.encoding.json (encoding/mod.rs:259-268)."""
    return ENCODED_DIR / encoding / f"{name}.json"


def metadata_path(points_path):
    points_path = Path(points_path)
    return points_path.with_name(f"{points_path.stem}.encoding.json")


# --- Mosaic's grid (A1, A5) --------------------------------------------------------------------
# src/data_toolkit/mod.rs:10-42, replicated operation by operation in IEEE double precision.

_AUSTIN_MIN_LAT = 30.267 - 1.0   # AUSTIN_CENTER.0 - BUFFER_DEGREES (mod.rs:10-12), in f64
_AUSTIN_MIN_LON = -97.743 - 1.0  # AUSTIN_CENTER.1 - BUFFER_DEGREES (mod.rs:10-14), in f64
_DECIMAL_SCALE = 1000.0          # DECIMAL_SCALE as f64 (mod.rs:19)
_GRID_SIZE = 2000                # LAT_GRID_SIZE = LON_GRID_SIZE (mod.rs:21-22)
_U32_MAX = 2**32 - 1


def _rust_round(x):
    """f64::round: halfway cases away from zero (Python's round() rounds them to even)."""
    if not math.isfinite(x):
        return x
    a = abs(x)
    f = math.floor(a)
    r = f + 1 if a - f >= 0.5 else f  # a - f is exact for |x| < 2^52
    return math.copysign(r, x)


def _as_u32(x):
    """Rust `f64 as u32`: saturating; NaN and negatives give 0."""
    if math.isnan(x) or x <= 0:
        return 0
    if x >= _U32_MAX:
        return _U32_MAX
    return int(x)


def to_grid(lat, lon):
    """(lat_grid, lon_grid) of a location: geo_to_grid (mod.rs:27-42), clamping included."""
    lat_grid = _as_u32(_rust_round((lat - _AUSTIN_MIN_LAT) * _DECIMAL_SCALE))  # mod.rs:28
    lon_grid = _as_u32(_rust_round((lon - _AUSTIN_MIN_LON) * _DECIMAL_SCALE))  # mod.rs:29
    return (lat_grid if lat_grid < _GRID_SIZE else _GRID_SIZE - 1,             # mod.rs:31-35
            lon_grid if lon_grid < _GRID_SIZE else _GRID_SIZE - 1)             # mod.rs:36-40


def release_binary(name):
    """target/release/<name>(.exe), or exit with the build command."""
    exe = ROOT / "target" / "release" / (name + (".exe" if sys.platform == "win32" else ""))
    if not exe.exists():
        raise SystemExit(f"{relpath(exe)} not found. Build it first:\n  {BUILD_COMMAND}")
    return exe


# --- The repo's configs vs the harness parameters ----------------------------------------------


def compare_configs():
    """Differences between the repo's day L-infinity FSS configs and ENCODINGS / METHOD / tau.

    The benchmark runs write their own configs per subset (prompt 2); this only shows where the
    repo's configs (configs/unknown_dictionary, configs/encodings/<enc>) differ from the harness.
    """
    results = []
    for name, spec in ENCODINGS.items():
        path = ROOT / spec["config_dir"] / "busiest_day" / "linf_fss_config.json"
        diffs = []
        if not path.exists():
            diffs.append("file missing")
        else:
            protocol = json.loads(path.read_text(encoding="utf-8"))["protocol"]
            n = protocol.get("num_clients")
            checks = [
                ("d", protocol.get("d"), spec["d"]),
                ("h1", protocol.get("h1"), spec["h1"]),
                ("delta", protocol.get("delta"), spec["delta"]),
                ("match_threshold", protocol.get("match_threshold"),
                 threshold_abs(n, THRESHOLD_FRAC) if isinstance(n, int) else None),
                ("derived_coordinate", protocol.get("derived_coordinate"),
                 spec["params"].get("derived_coordinate")),
                *[(key, protocol.get(key), METHOD[key])
                  for key in ("share_method", "enable_sketch", "distance_metric", "check_property",
                              "dictionary_type")],
            ]
            diffs = [f"{key} = {got!r}, harness uses {want!r}" for key, got, want in checks
                     if got != want]
        results.append({"encoding": name, "file": relpath(path), "differences": diffs})
    method = json.loads(METHOD_CONFIG.read_text(encoding="utf-8")) if METHOD_CONFIG.exists() else {}
    want = {"check_method": METHOD["check_method"], "threshold_method": METHOD["threshold_method"]}
    results.append({"encoding": "(method)", "file": relpath(METHOD_CONFIG),
                    "differences": [] if method == want else [f"{method!r}, harness uses {want!r}"]})
    return results


# --- Files -------------------------------------------------------------------------------------


def relpath(path):
    """Path relative to the repo, with forward slashes (for manifests and reports)."""
    path = Path(path).resolve()
    try:
        return path.relative_to(ROOT).as_posix()
    except ValueError:
        return path.as_posix()


def sha256_file(path):
    h = hashlib.sha256()
    with open(path, "rb") as f:
        for block in iter(lambda: f.read(1 << 20), b""):
            h.update(block)
    return h.hexdigest()


def points_sha256(points):
    """SHA-256 of a point list as compact JSON: independent of whitespace and line endings."""
    text = json.dumps([[int(v) for v in p] for p in points], separators=(",", ":"))
    return hashlib.sha256(text.encode("ascii")).hexdigest()


def read_points(path):
    """(lat, lon) of every row of a ride CSV."""
    points = []
    with open(path, newline="", encoding="utf-8") as f:
        for line, row in enumerate(csv.DictReader(f), start=2):
            try:
                points.append((float(row[COORD_COLUMNS[0]]), float(row[COORD_COLUMNS[1]])))
            except (KeyError, TypeError, ValueError) as e:
                raise ValueError(f"{path}:{line}: bad coordinates ({e})") from None
    return points


def load_json_points(path):
    """A Mosaic points file: a JSON list of integer coordinate lists (mosaic_client.rs:6-13)."""
    with open(path, encoding="utf-8") as f:
        return [[int(v) for v in p] for p in json.load(f)]


def load_manifest():
    with open(DATA_DIR / "manifest.json", encoding="utf-8") as f:
        return json.load(f)


def write_json(path, obj, indent=1):
    """Write JSON with LF line endings."""
    path = Path(path)
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(obj, indent=indent) + "\n", encoding="utf-8", newline="\n")


def write_json_rows(path, obj, row_keys):
    """Write a JSON object with LF line endings, one element per line for the lists in row_keys
    and every other value on one line (keeps large point lists readable and diffable)."""
    keys = list(obj)
    lines = ["{"]
    for i, key in enumerate(keys):
        comma = "," if i < len(keys) - 1 else ""
        value = obj[key]
        if key in row_keys and isinstance(value, list) and value:
            lines.append(f" {json.dumps(key)}: [")
            for j, row in enumerate(value):
                lines.append("  " + json.dumps(row) + ("," if j < len(value) - 1 else ""))
            lines.append(f" ]{comma}")
        else:
            lines.append(f" {json.dumps(key)}: {json.dumps(value)}{comma}")
    lines.append("}")
    path = Path(path)
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text("\n".join(lines) + "\n", encoding="utf-8", newline="\n")


def timestamp():
    """Local time with UTC offset, e.g. 2026-11-10T14:03:11+01:00."""
    return datetime.datetime.now().astimezone().isoformat(timespec="seconds")


# --- Environment (A8) --------------------------------------------------------------------------


def _cpu_name():
    if sys.platform == "win32":
        try:
            import winreg

            key = r"HARDWARE\DESCRIPTION\System\CentralProcessor\0"
            with winreg.OpenKey(winreg.HKEY_LOCAL_MACHINE, key) as k:
                return winreg.QueryValueEx(k, "ProcessorNameString")[0].strip()
        except OSError:
            pass
    elif sys.platform.startswith("linux"):
        try:
            with open("/proc/cpuinfo", encoding="utf-8") as f:
                for line in f:
                    if line.startswith("model name"):
                        return line.split(":", 1)[1].strip()
        except OSError:
            pass
    elif sys.platform == "darwin":
        try:
            return subprocess.run(["sysctl", "-n", "machdep.cpu.brand_string"],
                                  capture_output=True, text=True, check=True).stdout.strip()
        except (OSError, subprocess.CalledProcessError):
            pass
    return platform.processor() or "unknown"


def _run(*cmd):
    """stdout of a command, or None if it cannot run."""
    try:
        return subprocess.run(cmd, cwd=ROOT, capture_output=True, text=True,
                              check=True).stdout.strip()
    except (OSError, subprocess.CalledProcessError):
        return None


def _rustc_version():
    exe = "rustc.exe" if sys.platform == "win32" else "rustc"
    for candidate in (shutil.which("rustc"), str(Path.home() / ".cargo" / "bin" / exe)):
        if candidate and Path(candidate).exists():
            version = _run(candidate, "--version")
            if version:
                return version
    return None


def server_threads(cores_physical):
    """Threads per server process: floor(physical cores / 2), at least 1 (A8)."""
    return max(1, cores_physical // 2)


def collect_env():
    """The `env` object of a result line (A8, A9). Mosaic is Rust too: compiler = rustc."""
    import psutil

    physical = psutil.cpu_count(logical=False) or os.cpu_count() or 1
    status = _run("git", "status", "--porcelain", "--untracked-files=no")
    return {
        "cpu": _cpu_name(),
        "cores_physical": physical,
        "cores_logical": psutil.cpu_count(logical=True) or physical,
        "ram_gb": round(psutil.virtual_memory().total / 2**30, 1),
        "os": platform.platform(),
        "threads": server_threads(physical),
        "compiler": _rustc_version(),
        "commit": _run("git", "rev-parse", "HEAD"),
        # Tracked changes only: generated, untracked bench output does not count.
        "dirty": None if status is None else bool(status),
    }


# --- Result schema (A9) ------------------------------------------------------------------------


class _Errors(list):
    def add(self, path, msg):
        self.append(f"{path}: {msg}")


def _type(kind, nullable=False):
    def check(errors, value, path):
        if value is None:
            if not nullable:
                errors.add(path, "must not be null")
            return
        is_number = isinstance(value, (int, float)) and not isinstance(value, bool)
        ok = {
            "int": isinstance(value, int) and not isinstance(value, bool),
            "num": is_number and math.isfinite(value),
            "str": isinstance(value, str),
            "bool": isinstance(value, bool),
            "dict": isinstance(value, dict),
        }[kind]
        if not ok:
            errors.add(path, f"expected {kind}, got {type(value).__name__} {value!r}")

    return check


def _enum(*values):
    def check(errors, value, path):
        if value not in values:
            errors.add(path, f"expected one of {list(values)}, got {value!r}")

    return check


def _iso_time(errors, value, path):
    try:
        datetime.datetime.fromisoformat(value)
    except (TypeError, ValueError):
        errors.add(path, f"expected an ISO 8601 timestamp, got {value!r}")


def _pair(errors, value, path):
    """Per-server values: [server0, server1], or null if the field does not apply."""
    if value is None:
        return
    if not isinstance(value, list) or len(value) != 2:
        errors.add(path, f"expected [server0, server1], got {value!r}")
        return
    for i, v in enumerate(value):
        _type("num")(errors, v, f"{path}[{i}]")


def _num_or_pair(errors, value, path):
    """A9 v3.1 server_to_server: a single number (Mastic: 0), [sent by server0, sent by server1],
    or null."""
    if isinstance(value, list):
        _pair(errors, value, path)
    else:
        _type("num", True)(errors, value, path)


def _list(item):
    def check(errors, value, path):
        if not isinstance(value, list):
            errors.add(path, f"expected a list, got {type(value).__name__}")
            return
        for i, v in enumerate(value):
            item(errors, v, f"{path}[{i}]")

    return check


def _object(fields, optional=None):
    """An object with exactly the keys of `fields`, plus any of the keys of `optional`."""
    optional = optional or {}

    def check(errors, value, path):
        if not isinstance(value, dict):
            errors.add(path, f"expected an object, got {type(value).__name__}")
            return
        for key in fields.keys() - value.keys():
            errors.add(f"{path}.{key}", "missing")
        for key in value.keys() - fields.keys() - optional.keys():
            errors.add(f"{path}.{key}", "unexpected field")
        for key, field in {**fields, **optional}.items():
            if key in value:
                field(errors, value[key], f"{path}.{key}")

    return check


_INT, _NUM, _STR, _BOOL = _type("int"), _type("num"), _type("str"), _type("bool")
_INT_OR_NULL, _NUM_OR_NULL = _type("int", True), _type("num", True)

_HEADER = {
    "schema_version": _enum(SCHEMA_VERSION),
    "system": _enum("mastic", "mosaic"),
    "encoding": _enum(*ENCODINGS),
    "dataset": _STR,
    "subset": _enum(*SUBSETS),
    "n_clients": _INT,
    "rep": _INT,
    "warmup": _BOOL,
    "timestamp": _iso_time,
    "params": _object({
        "data_bits": _INT_OR_NULL,
        "threshold_frac": _NUM,
        "threshold_abs": _INT,
        "delta": _INT_OR_NULL,
        "encoding_params": _type("dict"),
    }),
    "env": _object({
        "cpu": _STR,
        "cores_physical": _INT,
        "cores_logical": _INT,
        "ram_gb": _NUM,
        "os": _STR,
        "threads": _INT,
        "compiler": _type("str", True),
        "commit": _type("str", True),
        "dirty": _type("bool", True),
    }),
}

# A9 v3.1: `extra` is an optional object of system-specific values on either kind of line.
_OPTIONAL = {"extra": _type("dict")}

_RUN = _object({
    **_HEADER,
    "phases": _object({
        "server_offline_ms": _NUM_OR_NULL,
        "upload_ms": _NUM_OR_NULL,
        "online_ms": _NUM_OR_NULL,
        "online_steps": _list(_object({"name": _STR, "ms": _NUM_OR_NULL})),
    }),
    "rounds": _object({"upload": _INT_OR_NULL, "online": _INT_OR_NULL, "extra": _INT_OR_NULL}),
    "bytes": _object({
        "upload": _object({"client_to_server": _pair}),
        "online": _object({
            "collector_to_server": _pair,
            "server_to_collector": _pair,
            "server_to_server": _num_or_pair,
        }),
    }),
    "leakage": _object({
        "revealed_aggregates_total": _INT_OR_NULL,
        "revealed_aggregates_per_level": _list(_INT),
        "revealed_kind": _type("str", True),
        "client_validity_bits": _INT_OR_NULL,
        "frontier_per_level": _list(_INT),
    }),
    # A9 v3.1: count null when the system reveals no counts (Mosaic); lat/lon null for outputs
    # that are not real cells.
    "output": _list(_object({
        "label": _type("str", True),
        "lat": _NUM_OR_NULL,
        "lon": _NUM_OR_NULL,
        "count": _INT_OR_NULL,
    })),
}, _OPTIONAL)

_CLIENT = _object({
    **_HEADER,
    "client": _object({
        "k": _INT,
        "keygen_ms_mean": _NUM_OR_NULL,
        "keygen_ms_median": _NUM_OR_NULL,
        "keygen_ms_p95": _NUM_OR_NULL,
        "report_bytes_per_server": _pair,
        "upload_bytes_per_client": _NUM_OR_NULL,
    }),
}, _OPTIONAL)


def _mosaic_rules(errors, obj):
    """Mosaic lines record delta in params on every line (decision 2 of prompt 1)."""
    if isinstance(obj, dict) and obj.get("system") == "mosaic":
        delta = (obj.get("params") or {}).get("delta") if isinstance(obj.get("params"), dict) else None
        if not (isinstance(delta, int) and not isinstance(delta, bool)):
            errors.add("$.params.delta", f"Mosaic lines must record delta as an int, got {delta!r}")


def validate_result(obj, kind="run"):
    """Problems with a result line ("run": results/raw/<system>.jsonl, "client":
    results/raw/<system>_client.jsonl). An empty list means the line is valid."""
    errors = _Errors()
    {"run": _RUN, "client": _CLIENT}[kind](errors, obj, "$")
    _mosaic_rules(errors, obj)
    return sorted(errors)


def append_jsonl(path, obj, kind="run"):
    """Check a result line against A9, then append it to a JSONL file."""
    errors = validate_result(obj, kind)
    if errors:
        raise ValueError("result line does not match the A9 schema:\n  " + "\n  ".join(errors))
    path = Path(path)
    path.parent.mkdir(parents=True, exist_ok=True)
    with open(path, "a", encoding="utf-8", newline="\n") as f:
        f.write(json.dumps(obj, separators=(",", ":")) + "\n")


# --- Self-check (A5) ---------------------------------------------------------------------------


def self_check(output, truth):
    """A5 self-check of one run's output against the plaintext answer truth["cells"] (same system,
    encoding, subset and tau). Outputs are matched to the answer's cells by label.

    Copied unchanged from the Mastic harness (bench_common.self_check), so runner and evaluation
    use one definition.

    - precision / recall: share of reported labels in the answer / of the answer's labels reported;
    - exact: every count equals the answer's. None when the run reveals no counts (every output
      count null, A9 spec v3.1): then only the sets of reported locations are compared;
    - centres: equal where both sides have coordinates. An output without coordinates (lat/lon
      null, not a real cell, A9 spec v3.1) must have null coordinates in the answer too;
    - problems: labels that are null or appear twice, counts that are only partly null, and
      coordinates on one side only.
    ok = precision 1, recall 1, exact not False, centres equal and no problems.
    """
    problems = []
    labels = [c["label"] for c in output]
    if any(label is None for label in labels) or len(set(labels)) != len(labels):
        problems.append("output labels must be unique and non-null")
    want = {c["label"]: c for c in truth["cells"]}
    got = {c["label"]: c for c in output}
    matched = want.keys() & got.keys()
    precision = len(matched) / len(got) if got else float(not want)
    recall = len(matched) / len(want) if want else 1.0
    counts = [c["count"] for c in output]
    if output and all(v is None for v in counts):
        exact = None  # the system reveals no counts: locations only
    else:
        if any(v is None for v in counts):
            problems.append("some output counts are null and others are not")
        exact = matched == want.keys() == got.keys() and all(
            got[k]["count"] == want[k]["count"] for k in matched)

    def located(c):
        return c.get("lat") is not None and c.get("lon") is not None

    if any(located(got[k]) != located(want[k]) for k in matched):
        problems.append("an output has coordinates where the plaintext answer has none, or the "
                        "reverse")
    both = [k for k in matched if located(got[k]) and located(want[k])]
    centre = max((max(abs(got[k]["lat"] - want[k]["lat"]), abs(got[k]["lon"] - want[k]["lon"]))
                  for k in both), default=0.0)
    return {"precision": precision, "recall": recall, "exact": exact,
            "cells": len(got), "truth_cells": len(want), "max_centre_diff_deg": centre,
            "problems": problems,
            "ok": precision == 1.0 and recall == 1.0 and exact is not False and centre < 1e-6
            and not problems}


# --- Self-test and CLI -------------------------------------------------------------------------

# The examples of A9.
EXAMPLE_RUN = {
    "schema_version": 1,
    "system": "mastic",
    "encoding": "geohash",
    "dataset": "day",
    "subset": "1of3",
    "n_clients": 7194,
    "rep": 2,
    "warmup": False,
    "timestamp": "2026-11-10T14:03:11+01:00",
    "params": {"data_bits": 21, "threshold_frac": 0.005, "threshold_abs": 35, "delta": None,
               "encoding_params": {"prefix": "9v6", "bits": 21}},
    "env": {"cpu": "...", "cores_physical": 8, "cores_logical": 16, "ram_gb": 16, "os": "...",
            "threads": 4, "compiler": "rustc 1.xx", "commit": "abc123", "dirty": False},
    "phases": {"server_offline_ms": 0, "upload_ms": 812.4, "online_ms": 4210.7,
               "online_steps": [{"name": "flp", "ms": 640.2}, {"name": "level_0", "ms": 151.3}]},
    "rounds": {"upload": 1, "online": 47, "extra": 0},
    "bytes": {"upload": {"client_to_server": [0, 0]},
              "online": {"collector_to_server": [0, 0], "server_to_collector": [0, 0],
                         "server_to_server": 0}},
    "leakage": {"revealed_aggregates_total": 694, "revealed_aggregates_per_level": [2, 4],
                "revealed_kind": "count", "client_validity_bits": 7194,
                "frontier_per_level": [1, 2]},
    "output": [{"label": "9v6s0j9", "lat": 30.265, "lon": -97.732, "count": 120}],
}
EXAMPLE_CLIENT = {
    **{k: v for k, v in EXAMPLE_RUN.items() if k in _HEADER},
    "client": {"k": 2000, "keygen_ms_mean": 0.41, "keygen_ms_median": 0.39,
               "keygen_ms_p95": 0.52, "report_bytes_per_server": [2210, 2210],
               "upload_bytes_per_client": 4420},
}
# A Mosaic line with the v3.1 additions (illustrative values).
EXAMPLE_MOSAIC_RUN = {
    **copy.deepcopy(EXAMPLE_RUN),
    "system": "mosaic",
    "encoding": "h3",
    "params": {"data_bits": 23, "threshold_frac": 0.02, "threshold_abs": 143, "delta": 5,
               "encoding_params": copy.deepcopy(ENCODINGS["h3"]["params"])},
    "phases": {"server_offline_ms": 0, "upload_ms": 812.4, "online_ms": 42107.0,
               "online_steps": [{"name": "sketch_verify", "ms": 6860.2},
                                {"name": "expand", "ms": 9000.0},
                                {"name": "check_threshold", "ms": 25000.0},
                                {"name": "reveal", "ms": 900.0},
                                {"name": "derived_check", "ms": 346.8}]},
    "bytes": {"upload": {"client_to_server": [96000000, 96000000]},
              "online": {"collector_to_server": [0, 0], "server_to_collector": [0, 0],
                         "server_to_server": [2740000000, 11600000000]}},
    "leakage": {"revealed_aggregates_total": 2577, "revealed_aggregates_per_level": [2, 4],
                "revealed_kind": "bit", "client_validity_bits": 7194,
                "frontier_per_level": [2, 4]},
    "output": [{"label": "8a489e3ffffffff", "lat": 30.27, "lon": -97.75, "count": None},
               {"label": None, "lat": None, "lon": None, "count": None}],
    "extra": {"h3_noncell_outputs": 1, "dealer_bytes": [27, 27]},
}


def self_test():
    """The A9 examples and the Mosaic example must pass; each broken copy must fail."""

    def broken(base, change):
        obj = copy.deepcopy(base)
        change(obj)
        return obj

    cases = [
        ("A9 run example", EXAMPLE_RUN, "run", True),
        ("A9 client example", EXAMPLE_CLIENT, "client", True),
        ("Mosaic run (v3.1 additions)", EXAMPLE_MOSAIC_RUN, "run", True),
        ("Mosaic run without extra", broken(EXAMPLE_MOSAIC_RUN, lambda o: o.pop("extra")), "run",
         True),
        ("missing rounds", broken(EXAMPLE_RUN, lambda o: o.pop("rounds")), "run", False),
        ("per-server list of 1", broken(
            EXAMPLE_RUN, lambda o: o["bytes"]["online"].update(collector_to_server=[5])),
         "run", False),
        ("server_to_server list of 3", broken(
            EXAMPLE_MOSAIC_RUN, lambda o: o["bytes"]["online"].update(server_to_server=[1, 2, 3])),
         "run", False),
        ("warmup as a string", broken(EXAMPLE_RUN, lambda o: o.update(warmup="no")), "run", False),
        ("unexpected field", broken(EXAMPLE_RUN, lambda o: o.update(extra_field=1)), "run", False),
        ("extra not an object", broken(EXAMPLE_MOSAIC_RUN, lambda o: o.update(extra=[1])), "run",
         False),
        ("fractional count", broken(EXAMPLE_RUN, lambda o: o["output"][0].update(count=1.5)),
         "run", False),
        ("unknown encoding", broken(EXAMPLE_RUN, lambda o: o.update(encoding="s2")), "run", False),
        ("Mosaic line without delta", broken(
            EXAMPLE_MOSAIC_RUN, lambda o: o["params"].update(delta=None)), "run", False),
        ("client without client", broken(EXAMPLE_CLIENT, lambda o: o.pop("client")), "client",
         False),
    ]
    ok = True
    for name, obj, kind, should_pass in cases:
        errors = validate_result(obj, kind)
        passed = not errors
        good = passed == should_pass
        ok &= good
        detail = "valid" if passed else f"rejected: {errors[0]}"
        print(f"  {'PASS' if good else 'FAIL'}  {name}: {detail}")
    return ok


def main():
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--self-test", action="store_true", help="test the schema check")
    args = parser.parse_args()

    if args.self_test:
        print("Schema check self-test:")
        raise SystemExit(0 if self_test() else 1)

    print("Environment (A8):")
    print(json.dumps(collect_env(), indent=1))
    print(f"\nHarness parameters: dataset {DATASET}, seed {SEED}, threshold tau per system "
          f"{THRESHOLDS} (this codebase: {THRESHOLD_FRAC}), delta {DELTA}; reference at tau "
          f"{REFERENCE_TAU}, delta {REFERENCE_DELTA}")
    print(f"Method: {METHOD}")
    for name, spec in ENCODINGS.items():
        print(f"  {name:12} d = {spec['d']}, h1 = {spec['h1']:2}, n = {spec['data_bits']:2}, "
              f"delta = {spec['delta']}  {spec['params']}")
    print("\nRepo day configs (linf_fss) against the harness parameters:")
    for result in compare_configs():
        status = "equal" if not result["differences"] else "; ".join(result["differences"])
        print(f"  {result['file']}: {status}")


if __name__ == "__main__":
    main()
