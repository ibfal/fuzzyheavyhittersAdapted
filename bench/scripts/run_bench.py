#!/usr/bin/env python3
"""Run the Mosaic benchmark (bench/BENCHMARK_MOSAIC.md: A2, A6, A8, A9, B3, B6, B7).

    .\\.venv\\Scripts\\python.exe bench\\scripts\\run_bench.py --smoke      # smoke runs, ten_locations.csv
    .\\.venv\\Scripts\\python.exe bench\\scripts\\run_bench.py              # the main benchmark (72 runs, tau 0.02)
    .\\.venv\\Scripts\\python.exe bench\\scripts\\run_bench.py --tau 0.005  # the optional matched runs (72)
    .\\.venv\\Scripts\\python.exe bench\\scripts\\run_bench.py --only-encoding h3 --delta 4          # optional H3 delta 4
    .\\.venv\\Scripts\\python.exe bench\\scripts\\run_bench.py --only-encoding h3 --h3-crawl original # optional authors' crawl
    .\\.venv\\Scripts\\python.exe bench\\scripts\\run_bench.py --only-encoding mosaic_grid --only-subset 3of3 --reps 1 --no-warmup --results-dir bench\\.run\\check

Steps: build the release binaries once (mosaic_server, mosaic_client, mosaic_dealer,
mosaic_client_bench); write one Mosaic config per encoding x subset into bench/.run/configs/
(encoding parameters from bench_common.ENCODINGS, T = max(1, floor(tau N)), H3 delta from --delta,
the H3 crawl from --h3-crawl); run the client micro-benchmark once per encoding (A6); then,
repetition loop outermost (A8), for every encoding and subset: start the dealer and wait until its
keys are pre-generated, start both servers on free ports (--port-base, never 8000) and wait until
they listen, start the client simulator, wait for all four processes, check server 0's result line
against the A9 schema and the ground truth, append it to <results-dir>/mosaic.jsonl, and stop
every process (also on errors and Ctrl+C). The channel buffers are set to 64 MiB
(MOSAIC_CHANNEL_BUFFER_BYTES, --channel-buffer).

Logs, configs and a per-run record (wall-clock time, peak memory) go to bench/.run/ (ignored by
Git).
"""

import argparse
import json
import os
import shutil
import socket
import subprocess
import sys
import time
from pathlib import Path

sys.dont_write_bytecode = True
sys.path.insert(0, str(Path(__file__).resolve().parent))
import bench_common as bc  # noqa: E402

RUN_DIR = bc.BENCH / ".run"
RESULTS_DIR = bc.BENCH / "results" / "raw"
EXE = ".exe" if sys.platform == "win32" else ""
TARGET = bc.ROOT / "target" / "release"
BINARIES = ("mosaic_server", "mosaic_client", "mosaic_dealer", "mosaic_client_bench")

# Ports that must never be used (8000: Splunk on the benchmark laptop).
RESERVED_PORTS = {8000}
DEFAULT_PORT_BASE = 18300
# Consecutive runs use different port blocks, so a run never waits for the sockets of the last.
PORT_BLOCKS = 4
DEFAULT_RUN_TIMEOUT = 6 * 3600  # seconds


class RunError(RuntimeError):
    pass


# --- What each run reads -----------------------------------------------------------------------


def subset_specs(args):
    """{subset: {name, csv, rows, dataset, threshold}}; name is the stem of the converted points."""
    if args.smoke:
        rows = len(bc.read_points(bc.SMOKE_CSV))
        return {bc.SMOKE_SUBSET: {"name": bc.SMOKE_CSV.stem, "csv": bc.SMOKE_CSV, "rows": rows,
                                  "dataset": bc.SMOKE_DATASET, "threshold": bc.SMOKE_THRESHOLD}}
    manifest = {f["subset"]: f for f in bc.load_manifest()["files"]}
    specs = {}
    for subset in args.only_subset or bc.SUBSETS:
        entry = manifest[subset]
        csv = bc.ROOT / entry["path"]
        if bc.sha256_file(csv) != entry["sha256"]:
            raise RunError(f"{entry['path']} does not match the SHA-256 in bench/data/manifest.json")
        specs[subset] = {"name": f"{bc.DATASET}_{subset}", "csv": csv, "rows": entry["rows"],
                         "dataset": bc.DATASET, "threshold": args.tau}
    return specs


def run_delta(enc, args):
    """H3 uses --delta (hexagon steps); every other encoding the harness delta (A2)."""
    return args.delta if enc == "h3" else bc.ENCODINGS[enc]["delta"]


def h3_original(enc, args):
    return enc == "h3" and args.h3_crawl == "original"


def encoding_params(enc, args):
    """params.encoding_params: bench_common's, plus for H3 the crawl that ran (prompt 2)."""
    params = json.loads(json.dumps(bc.ENCODINGS[enc]["params"]))
    if enc == "h3":
        if args.h3_crawl == "original":
            params.pop("derived_coordinate", None)
        params["h3_crawl"] = args.h3_crawl
    return params


def data_bits(enc, args):
    spec = bc.ENCODINGS[enc]
    return spec["d"] * spec["h1"] if h3_original(enc, args) else spec["data_bits"]


def truth_path(enc, subset, spec, args):
    variant = bc.gt_variant(enc, run_delta(enc, args))
    if subset == bc.SMOKE_SUBSET:
        return bc.GROUND_TRUTH_DIR / f"smoke_{spec['name']}_{variant}_tau{bc.tau_tag(spec['threshold'])}.json"
    return bc.ground_truth_path(bc.DATASET, subset, variant, spec["threshold"])


def config_name(enc, subset, args, tau):
    """Config file stem; non-default tau, H3 delta and H3 crawl get suffixes, so they never
    replace the configs of the main runs."""
    name = f"{enc}-{subset}"
    if tau != bc.THRESHOLD_FRAC and subset != bc.SMOKE_SUBSET:
        name += f"-tau{bc.tau_tag(tau)}"
    if enc == "h3" and args.delta != bc.ENCODINGS["h3"]["delta"]:
        name += f"-delta{args.delta}"
    if h3_original(enc, args):
        name += "-original"
    return name


def protocol_config(enc, points_path, rows, threshold, args):
    """A Mosaic run config (the format of configs/unknown_dictionary/*/linf_fss_config.json)."""
    spec = bc.ENCODINGS[enc]
    protocol = {
        "delta": run_delta(enc, args),
        "match_threshold": bc.threshold_abs(rows, threshold),
        "enable_sketch": bc.METHOD["enable_sketch"],
        "h1": spec["h1"],
        "h2": bc.PROTOCOL_BASE["h2"],
        "h3": bc.PROTOCOL_BASE["h3"],
        "d": spec["d"],
        "share_method": bc.METHOD["share_method"],
        "dictionary_type": bc.METHOD["dictionary_type"],
        "check_property": bc.METHOD["check_property"],
        "distance_metric": bc.METHOD["distance_metric"],
        "num_clients": rows,
        "sketch_modulus": bc.PROTOCOL_BASE["sketch_modulus"],
    }
    derived = spec["params"].get("derived_coordinate")
    if derived is not None and not h3_original(enc, args):
        protocol["derived_coordinate"] = derived
    if rows >= 2 ** protocol["h3"]:
        raise RunError(f"N = {rows} does not fit the count ring of h3 = {protocol['h3']} bits")
    path = str(Path(points_path).resolve())
    return {"data_file": path, "query_file": path, "protocol": protocol,
            "output": {"verbose": False, "show_intermediate": False, "output_file": None}}


def write_json(path, obj):
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(obj, indent=1) + "\n", encoding="utf-8", newline="\n")


def check_repo_configs():
    """Warn where the repo's day configs differ from what run_bench writes (beyond N, T, delta)."""
    for result in bc.compare_configs():
        diffs = [d for d in result["differences"]
                 if not d.startswith(("match_threshold", "delta", "derived_coordinate"))]
        if diffs:
            print(f"WARNING: {result['file']}: {'; '.join(diffs)}")
    path = bc.ROOT / "configs" / "unknown_dictionary" / "busiest_day" / "linf_fss_config.json"
    protocol = json.loads(path.read_text(encoding="utf-8"))["protocol"]
    diffs = {k: protocol.get(k) for k, v in bc.PROTOCOL_BASE.items() if protocol.get(k) != v}
    if diffs:
        print(f"WARNING: {bc.relpath(path)} has {diffs}; the benchmark uses {bc.PROTOCOL_BASE}")


# --- Ports -------------------------------------------------------------------------------------


def port_layout(base, threads):
    """The ports of one run: server 0 listens on the inter-server ports, each server on its
    dealer ports and its client port (src/configs/network_config.rs, channel.rs)."""
    return {
        "server0_to_server1_port": base,
        "dealer_to_server0_port": base + threads,
        "dealer_to_server1_port": base + 2 * threads,
        "client_to_server0_port": base + 3 * threads,
        "client_to_server1_port": base + 3 * threads + 1,
    }


def all_ports(base, threads):
    return list(range(base, base + 3 * threads + 2))


def block_size(threads):
    return -(-(3 * threads + 2) // 10) * 10  # rounded up to 10


def port_busy(port):
    """True if something accepts connections on the port or holds it."""
    with socket.socket() as s:
        s.settimeout(0.3)
        if s.connect_ex(("127.0.0.1", port)) == 0:
            return True
    with socket.socket() as s:
        if hasattr(socket, "SO_EXCLUSIVEADDRUSE"):
            s.setsockopt(socket.SOL_SOCKET, socket.SO_EXCLUSIVEADDRUSE, 1)
        try:
            s.bind(("127.0.0.1", port))
        except OSError:
            return True
    return False


def wait_ports_free(ports, timeout=30.0):
    deadline = time.monotonic() + timeout
    while True:
        busy = [p for p in ports if port_busy(p)]
        if not busy:
            return
        if time.monotonic() > deadline:
            raise RunError(f"port(s) {busy} are in use; pick other ports with --port-base")
        time.sleep(0.25)


def network_config(base, threads):
    return {"network": {"server0_addr": "127.0.0.1", "server1_addr": "127.0.0.1",
                        **port_layout(base, threads)}}


# --- Processes ---------------------------------------------------------------------------------


def find_cargo():
    for candidate in (shutil.which("cargo"), str(Path.home() / ".cargo" / "bin" / f"cargo{EXE}")):
        if candidate and Path(candidate).exists():
            return candidate
    raise RunError("cargo not found")


def build():
    cmd = [find_cargo(), "build", "--release"] + [x for b in BINARIES for x in ("--bin", b)]
    print("Building:", " ".join(cmd[1:]), flush=True)
    t0 = time.perf_counter()
    proc = subprocess.run(cmd, cwd=bc.ROOT, capture_output=True, text=True)
    if proc.returncode != 0:
        raise RunError(f"cargo build failed:\n{proc.stdout}{proc.stderr}")
    print(f"  {(proc.stderr.strip().splitlines() or ['done'])[-1].strip()} "
          f"({time.perf_counter() - t0:.0f} s)")


def binary(name):
    exe = TARGET / f"{name}{EXE}"
    if not exe.exists():
        raise RunError(f"{bc.relpath(exe)} not found; run without --no-build")
    return str(exe)


def tail(path, lines=25):
    try:
        text = Path(path).read_text(encoding="utf-8", errors="replace").splitlines()
    except OSError:
        return "(no log)"
    return "\n".join("    " + line for line in text[-lines:])


def log_failure(path):
    """The error line of a Mosaic log, if any. The binaries print errors and exit with code 0
    (mosaic_server.rs main), so the logs are checked as well as the exit codes."""
    try:
        text = Path(path).read_text(encoding="utf-8", errors="replace")
    except OSError:
        return None
    for line in text.splitlines():
        if line.startswith(("Error running", "Error:")) or "panicked at" in line:
            return line.strip()
    return None


def peak_mb(proc):
    """Peak working set (Windows) or current RSS, in MiB; None once the process is gone."""
    import psutil

    try:
        info = psutil.Process(proc.pid).memory_info()
    except (psutil.NoSuchProcess, psutil.AccessDenied):
        return None
    return getattr(info, "peak_wset", info.rss) / 2**20


def stop(procs):
    for p in procs:
        if p.poll() is None:
            p.terminate()
    for p in procs:
        try:
            p.wait(timeout=10)
        except subprocess.TimeoutExpired:
            p.kill()
            p.wait()


class Parties:
    """The processes of one run, their logs and their peak memory."""

    def __init__(self, logs_dir, name, env):
        self.logs_dir, self.name, self.env = logs_dir, name, env
        self.procs, self.logs, self.handles, self.peak = {}, {}, [], {}

    def start(self, party, cmd):
        log = self.logs_dir / f"{self.name}.{party}.log"
        h = open(log, "wb")
        self.handles.append(h)
        self.logs[party] = log
        self.procs[party] = subprocess.Popen(cmd, cwd=bc.ROOT, env=self.env, stdout=h,
                                             stderr=subprocess.STDOUT)
        self.peak[party] = 0.0

    def poll(self):
        """Update peak memory; raise if a party failed. True when every party has exited."""
        for party, p in self.procs.items():
            mb = peak_mb(p)
            if mb is not None:
                self.peak[party] = max(self.peak[party], mb)
        done = True
        for party, p in self.procs.items():
            code = p.poll()
            if code is None:
                done = False
                continue
            failure = log_failure(self.logs[party])
            if code != 0 or failure:
                raise RunError(f"{party} failed (exit code {code}): {failure or ''}\n"
                               f"{tail(self.logs[party])}")
        return done

    def wait_for(self, what, check, timeout):
        deadline = time.monotonic() + timeout
        while not check():
            self.poll()
            if time.monotonic() > deadline:
                raise RunError(f"timed out after {timeout:.0f} s waiting for {what}")
            time.sleep(0.05)

    def log_has(self, party, text):
        try:
            return text in self.logs[party].read_text(encoding="utf-8", errors="replace")
        except OSError:
            return False

    def close(self):
        stop(list(self.procs.values()))
        for h in self.handles:
            h.close()


# --- One run -----------------------------------------------------------------------------------


def part_path(out, party):
    return Path(f"{out}.{party}.part.json")


def read_line(path):
    lines = Path(path).read_text(encoding="utf-8").splitlines() if Path(path).exists() else []
    if len(lines) != 1:
        raise RunError(f"expected one result line in {path}, found {len(lines)}")
    return json.loads(lines[0])


def schema_errors(line, kind="run"):
    """bench_common.validate_result; smoke lines carry subset "smoke", which the schema's subset
    list (the benchmark subsets) does not include, so that one field is checked here instead."""
    if line.get("subset") == bc.SMOKE_SUBSET:
        return bc.validate_result({**line, "subset": bc.SUBSETS[0]}, kind)
    return bc.validate_result(line, kind)


def append_line(path, line, kind="run"):
    """bench_common.append_jsonl, which checks the schema first; smoke lines were checked by
    schema_errors and are written directly."""
    if line.get("subset") != bc.SMOKE_SUBSET:
        bc.append_jsonl(path, line, kind)
        return
    if schema_errors(line, kind):
        raise RunError("smoke line does not match the A9 schema")
    path.parent.mkdir(parents=True, exist_ok=True)
    with open(path, "a", encoding="utf-8", newline="\n") as f:
        f.write(json.dumps(line, separators=(",", ":")) + "\n")


def expected_original_crawl(points_path, delta, threshold, cache):
    """The authors' 3-coordinate H3 crawl in plaintext (ground_truth.crawl without the derived
    coordinate): its points and prefixes per level, cached in bench/.run/expected/."""
    if cache.exists():
        return json.loads(cache.read_text(encoding="utf-8"))
    import ground_truth as gt

    spec = bc.ENCODINGS["h3"]
    clients = gt.Clients(bc.load_json_points(points_path), spec["d"], spec["h1"])
    pts, _, levels = gt.crawl(clients, delta, threshold, None)
    result = {"points": sorted(list(map(int, p)) for p in pts),
              "candidates_per_level": [lv["checked"] for lv in levels],
              "frontier_per_level": [lv["kept"] for lv in levels]}
    write_json(cache, result)
    return result


def check_line(line, truth, spec, enc, args, points_path, name):
    """Header, self-check (A5) and per-level counts (A7) against the plaintext answer."""
    errors = schema_errors(line)
    if errors:
        raise RunError("result line does not match the A9 schema:\n  " + "\n  ".join(errors))
    if line["n_clients"] != spec["rows"] or line["n_clients"] != truth["N"]:
        raise RunError(f"n_clients {line['n_clients']}, rows {spec['rows']}, truth N {truth['N']}")
    if line["params"]["threshold_frac"] != spec["threshold"]:
        raise RunError(f"params.threshold_frac {line['params']['threshold_frac']} but the run's "
                       f"tau is {spec['threshold']}")
    if line["params"]["threshold_abs"] != truth["T"] or line["params"]["delta"] != truth["delta"]:
        raise RunError(f"T {line['params']['threshold_abs']} / delta {line['params']['delta']}, "
                       f"ground truth T {truth['T']} / delta {truth['delta']}")
    leak = line["leakage"]
    if not h3_original(enc, args):
        check = bc.self_check(line["output"], truth)
        want = (truth["candidates_per_level"], truth["frontier_per_level"])
    else:
        # The authors' crawl also outputs points that are not cells (lat/lon null): its cells
        # must be the ground truth's, and all its points those of the plaintext 3-coordinate crawl.
        located = [c for c in line["output"] if c["lat"] is not None]
        check = bc.self_check(located, truth)
        exp = expected_original_crawl(points_path, truth["delta"], truth["T"],
                                      RUN_DIR / "expected" / f"{name}.json")
        coords = {c["label"]: c["coords"] for c in truth["cells"]}
        got = sorted(coords[c["label"]] if c["label"] in coords else
                     [int(v) for v in c["label"][len("noncell ("):-1].split(", ")]
                     for c in line["output"])
        check["all_points_equal_plaintext_crawl"] = got == exp["points"]
        check["noncells"] = line["extra"].get("h3_noncell_outputs")
        check["ok"] = check["ok"] and check["all_points_equal_plaintext_crawl"]
        want = (exp["candidates_per_level"], exp["frontier_per_level"])
    check["levels_equal_ground_truth"] = (leak["revealed_aggregates_per_level"] == want[0]
                                          and leak["frontier_per_level"] == want[1])
    check["ok"] = check["ok"] and check["levels_equal_ground_truth"]
    return check


def write_meta(path, header):
    path.write_text(json.dumps(header), encoding="utf-8")


def run_header(enc, spec, subset, rep, warmup, env, args, points_path, n_clients, threshold_abs):
    return {
        "encoding": enc, "dataset": spec["dataset"], "subset": subset, "rep": rep,
        "warmup": warmup, "timestamp": bc.timestamp(), "env": env, "n_clients": n_clients,
        "params": {"data_bits": data_bits(enc, args), "threshold_frac": spec["threshold"],
                   "threshold_abs": threshold_abs, "delta": run_delta(enc, args),
                   "encoding_params": encoding_params(enc, args)},
        "encoding_metadata": str(bc.metadata_path(points_path)),
    }


def run_once(args, run, spec, config, env, results_file, truth, block):
    enc, subset, rep, warmup = run["encoding"], run["subset"], run["rep"], run["warmup"]
    name = f"{config.stem}-rep{rep}"
    logs_dir = RUN_DIR / "logs"
    logs_dir.mkdir(parents=True, exist_ok=True)
    out = RUN_DIR / "line.jsonl"
    meta_path = RUN_DIR / "meta.json"
    for p in [out] + [part_path(out, x) for x in ("dealer", "server1", "client")]:
        p.unlink(missing_ok=True)

    base = args.port_base + block * block_size(args.threads)
    wait_ports_free(all_ports(base, args.threads))
    net = RUN_DIR / "configs" / f"network-{base}.json"
    write_json(net, network_config(base, args.threads))
    ports = port_layout(base, args.threads)
    points_path = Path(json.loads(config.read_text(encoding="utf-8"))["data_file"])
    write_meta(meta_path, run_header(enc, spec, subset, rep, warmup, env, args, points_path,
                                     spec["rows"], truth["T"]))

    proc_env = {k: v for k, v in os.environ.items() if k != "RAYON_NUM_THREADS"}
    proc_env[bc.CHANNEL_BUFFER_ENV] = str(args.channel_buffer)
    parties = Parties(logs_dir, name, proc_env)
    common = ["--config", str(config), "--network-config", str(net)]
    t_start = time.perf_counter()
    try:
        # 1. The dealer pre-generates its keys before it connects; wait for that (B3 decision).
        parties.start("dealer", [binary("mosaic_dealer"), *common, "--threads", str(args.threads),
                                 "--bench-out", str(out)])
        parties.wait_for("the dealer's key pre-generation",
                         lambda: part_path(out, "dealer").exists(), timeout=600)
        # 2. Both servers; ready when they listen on their client ports.
        for side in (0, 1):
            cmd = [binary("mosaic_server"), "--side", str(side), *common,
                   "--method-config", str(bc.METHOD_CONFIG), "--threads", str(args.threads),
                   "--bench-out", str(out)]
            if side == 0:
                cmd += ["--bench-meta", f"@{meta_path}"]
            parties.start(f"server{side}", cmd)
        for side in (0, 1):
            port = ports[f"client_to_server{side}_port"]
            parties.wait_for(f"server {side} to listen on port {port}",
                             lambda s=side, p=port: parties.log_has(f"server{s}",
                                                                    f"Listening on 127.0.0.1:{p}"),
                             timeout=60)
        # 3. The client simulator (all N clients); then wait for everyone to finish.
        t_client = time.perf_counter()
        parties.start("client", [binary("mosaic_client"), *common, "--bench-out", str(out)])
        deadline = time.monotonic() + args.run_timeout
        while not parties.poll():
            if time.monotonic() > deadline:
                raise RunError(f"run did not finish within {args.run_timeout} s")
            time.sleep(0.25)
        protocol_wall = time.perf_counter() - t_client
    finally:
        parties.close()
    total = time.perf_counter() - t_start

    line = read_line(out)
    check = check_line(line, truth, spec, enc, args, points_path, config.stem)
    dealer = json.loads(part_path(out, "dealer").read_text(encoding="utf-8"))
    record = {
        "run": name, "encoding": enc, "subset": subset, "rep": rep, "warmup": warmup,
        "threshold_frac": spec["threshold"], "delta": line["params"]["delta"],
        "h3_crawl": args.h3_crawl if enc == "h3" else None,
        "timestamp": line["timestamp"], "n_clients": line["n_clients"], "threads": args.threads,
        "channel_buffer_bytes": args.channel_buffer,
        "protocol_wall_s": round(protocol_wall, 3), "run_total_s": round(total, 3),
        "online_ms": line["phases"]["online_ms"], "upload_ms": line["phases"]["upload_ms"],
        "online_steps": line["phases"]["online_steps"], "rounds": line["rounds"],
        "bytes": line["bytes"], "dealer_pregen_ms": round(dealer["pregen_ms"], 1),
        "peak_mb": {k: round(v, 1) for k, v in parties.peak.items()},
        "self_check": check, "results_file": bc.relpath(results_file),
    }
    with open(RUN_DIR / "runner_log.jsonl", "a", encoding="utf-8", newline="\n") as f:
        f.write(json.dumps(record) + "\n")
    if not check["ok"]:
        failed = RUN_DIR / "failed" / f"{name}.jsonl"
        failed.parent.mkdir(parents=True, exist_ok=True)
        failed.write_text(json.dumps(line) + "\n", encoding="utf-8")
        raise RunError(f"self-check failed {check}; line kept in {bc.relpath(failed)}")
    append_line(results_file, line)
    return record


def run_client_bench(enc, config, env, args, results_dir):
    """A6 micro-benchmark: the first 2,000 rows of day_3of3.csv, one thread."""
    out = RUN_DIR / "client_line.jsonl"
    meta = RUN_DIR / "client_meta.json"
    out.unlink(missing_ok=True)
    csv = bc.DATA_DIR / f"{bc.DATASET}_3of3.csv"
    points = bc.encoded_path(enc, f"{bc.DATASET}_3of3")
    header = run_header(enc, {"dataset": bc.DATASET, "threshold": args.tau}, "3of3", 0, False,
                        {**env, "threads": 1}, args, points, bc.CLIENT_BENCH_ROWS, 0)
    header.pop("encoding_metadata")
    # The threshold does not apply to report generation (as in Mastic's line: threshold_abs 0).
    write_meta(meta, {"schema_version": bc.SCHEMA_VERSION, "system": bc.SYSTEM, **header})
    cmd = [binary("mosaic_client_bench"), "--config", str(config), "--csv", str(csv),
           "--metadata", str(bc.metadata_path(points)), "--points", str(points),
           "--clients", str(bc.CLIENT_BENCH_ROWS), "--out", str(out), "--bench-meta", f"@{meta}"]
    proc = subprocess.run(cmd, cwd=bc.ROOT, env={**os.environ, "RAYON_NUM_THREADS": "1"},
                          capture_output=True, text=True)
    print("\n".join("  " + x for x in proc.stdout.splitlines() if not x.startswith("Appended")))
    if proc.returncode != 0:
        raise RunError(f"mosaic_client_bench failed for {enc}:\n{proc.stdout}{proc.stderr}")
    line = read_line(out)
    errors = schema_errors(line, "client")
    if errors:
        raise RunError("client line does not match the A9 schema:\n  " + "\n  ".join(errors))
    bc.append_jsonl(results_dir / f"{bc.SYSTEM}_client.jsonl", line, "client")
    return line


# --- Main --------------------------------------------------------------------------------------


def fmt_time(seconds):
    if seconds is None:
        return "?"
    seconds = int(round(seconds))
    h, rem = divmod(seconds, 3600)
    m, s = divmod(rem, 60)
    return f"{h}h{m:02d}m{s:02d}s" if h else f"{m}m{s:02d}s"


def estimate_left(plan, i, durations, costs):
    """Seconds left before run i: each configuration's last run time, or, for configurations not
    run yet, the observed time per unit of cost (clients x prefixes checked in the ground truth:
    Mosaic's work is one GC + OT over all N clients per checked prefix) scaled to it."""
    if not durations:
        return None
    key = lambda r: (r["encoding"], r["subset"])
    rate = sum(durations.values()) / sum(costs[k] for k in durations)
    return sum(durations.get(key(r), rate * costs[key(r)]) for r in plan[i:])


def parse_args():
    p = argparse.ArgumentParser(description=__doc__.splitlines()[0],
                                formatter_class=argparse.RawDescriptionHelpFormatter)
    p.add_argument("--reps", type=int, default=5, help="measured repetitions (default 5)")
    p.add_argument("--reps-override", action="append", default=[], metavar="ENC:SUBSET=N",
                   help="measured repetitions of one configuration instead of --reps, e.g. "
                        "h3:3of3=3 (B6: 3 for a day_3of3 run that takes more than about 10 "
                        "minutes); repeatable. The repetition loop stays outermost")
    p.add_argument("--no-warmup", action="store_true",
                   help="skip the warm-up repetition (rep 0, discarded by the evaluation)")
    p.add_argument("--smoke", action="store_true",
                   help="bench/data/ten_locations.csv, threshold 0.1, 1 rep, no warm-up")
    p.add_argument("--tau", type=float,
                   help=f"threshold fraction tau (default {bc.THRESHOLD_FRAC}, Mosaic's tau, A2; "
                        f"0.005 = the optional matched runs); recorded in params.threshold_frac")
    p.add_argument("--only-encoding", action="append", choices=list(bc.ENCODINGS),
                   help="run only this encoding (repeatable)")
    p.add_argument("--only-subset", action="append", choices=list(bc.SUBSETS),
                   help="run only this subset (repeatable)")
    p.add_argument("--delta", type=int, default=bc.ENCODINGS["h3"]["delta"],
                   help=f"H3 radius in hexagon steps (default {bc.ENCODINGS['h3']['delta']}; 4 = the "
                        f"optional runs). The other encodings always use delta "
                        f"{bc.DELTA} (A2)")
    p.add_argument("--h3-crawl", choices=bc.H3_CRAWLS, default="derived",
                   help="H3 crawl: derived = the repo's 23-level crawl (default, main runs); "
                        "original = the authors' 33-level crawl (no derived_coordinate)")
    p.add_argument("--threads", type=int, help="threads (and channels) per server and dealer "
                   "(default: floor(physical cores / 2), A8)")
    p.add_argument("--port-base", type=int, default=DEFAULT_PORT_BASE,
                   help=f"first port of the runs' port blocks (default {DEFAULT_PORT_BASE}); each "
                        f"run uses 3 x threads + 2 consecutive ports, never 8000")
    p.add_argument("--channel-buffer", type=int, default=bc.CHANNEL_BUFFER_BYTES,
                   help=f"channel buffer bytes ({bc.CHANNEL_BUFFER_ENV}; default "
                        f"{bc.CHANNEL_BUFFER_BYTES} = 64 MiB; Mosaic's own default is 1 GiB)")
    p.add_argument("--run-timeout", type=int, default=DEFAULT_RUN_TIMEOUT,
                   help=f"seconds before a run is stopped (default {DEFAULT_RUN_TIMEOUT})")
    p.add_argument("--results-dir", type=Path, help="where mosaic.jsonl and mosaic_client.jsonl "
                   "go (default bench/results/raw, or bench/.run/smoke with --smoke)")
    p.add_argument("--skip-keygen", action="store_true",
                   help="do not run the client micro-benchmark (mosaic_client_bench)")
    p.add_argument("--no-build", action="store_true", help="use the existing release binaries")
    p.add_argument("--dry-run", action="store_true", help="write the configs, print the plan, stop")
    args = p.parse_args()
    if args.smoke:
        args.reps, args.no_warmup = 1, True
        if args.only_subset:
            p.error("--only-subset does not apply to --smoke")
        if args.tau is not None:
            p.error(f"--tau does not apply to --smoke (threshold {bc.SMOKE_THRESHOLD})")
    if args.tau is None:
        args.tau = bc.THRESHOLD_FRAC
    elif not 0 < args.tau <= 1:
        p.error(f"--tau {args.tau}: must be in (0, 1]")
    if args.reps < 0 or (args.reps == 0 and args.no_warmup):
        p.error("nothing to run")
    overrides = {}
    for item in args.reps_override:
        try:
            config, n = item.split("=")
            enc, subset = config.split(":")
            n = int(n)
        except ValueError:
            p.error(f"--reps-override {item!r}: expected ENC:SUBSET=N, e.g. h3:3of3=3")
        if enc not in bc.ENCODINGS or subset not in bc.SUBSETS or n < 0:
            p.error(f"--reps-override {item!r}: unknown encoding or subset, or N < 0")
        overrides[enc, subset] = n
    args.reps_override = overrides
    try:
        bc.gt_variant("h3", args.delta)
    except ValueError as e:
        p.error(f"--delta {args.delta}: {e}")
    if args.channel_buffer <= 0:
        p.error("--channel-buffer must be positive")
    return args


def main():
    args = parse_args()
    env = bc.collect_env()
    args.threads = args.threads or env["threads"]
    env["threads"] = args.threads
    span = PORT_BLOCKS * block_size(args.threads)
    if any(args.port_base <= p < args.port_base + span for p in RESERVED_PORTS) \
            or not 1024 <= args.port_base <= 65535 - span:
        print(f"ERROR: --port-base {args.port_base}: ports {args.port_base}-"
              f"{args.port_base + span - 1} are reserved or invalid", file=sys.stderr)
        return 2
    results_dir = args.results_dir or (RUN_DIR / "smoke" if args.smoke else RESULTS_DIR)
    results_dir = results_dir if results_dir.is_absolute() else (bc.ROOT / results_dir)
    results_file = results_dir / f"{bc.SYSTEM}.jsonl"
    encodings = args.only_encoding or list(bc.ENCODINGS)
    try:
        specs = subset_specs(args)
        truths, configs, costs = {}, {}, {}
        for enc in encodings:
            for subset, spec in specs.items():
                tpath = truth_path(enc, subset, spec, args)
                if not tpath.exists():
                    raise RunError(f"{bc.relpath(tpath)} is missing: run ground_truth.py first")
                truths[enc, subset] = json.loads(tpath.read_text(encoding="utf-8"))
                points = bc.encoded_path(enc, spec["name"])
                cfg = protocol_config(enc, points, spec["rows"], spec["threshold"], args)
                path = RUN_DIR / "configs" / f"{config_name(enc, subset, args, spec['threshold'])}.json"
                write_json(path, cfg)
                configs[enc, subset] = path
                costs[enc, subset] = spec["rows"] * truths[enc, subset]["summary"]["prefixes_checked"]
            # The client micro-benchmark always reads day_3of3 (A6).
            points = bc.encoded_path(enc, f"{bc.DATASET}_3of3")
            n = len(bc.load_json_points(points))
            path = RUN_DIR / "configs" / f"{config_name(enc, 'client-bench', args, args.tau)}.json"
            write_json(path, protocol_config(enc, points, n, args.tau, args))
            configs[enc, "client-bench"] = path
    except RunError as e:
        print(f"ERROR: {e}", file=sys.stderr)
        return 1
    check_repo_configs()

    # Measured repetitions per configuration (--reps, or --reps-override for that one).
    n_reps = {(enc, subset): args.reps_override.get((enc, subset), args.reps)
              for enc in encodings for subset in specs}
    reps = ([] if args.no_warmup else [0]) + list(range(1, max(n_reps.values()) + 1))
    plan = [{"encoding": enc, "subset": subset, "rep": rep, "warmup": rep == 0}
            for rep in reps for enc in encodings for subset in specs
            if rep <= n_reps[enc, subset]]
    if args.reps_override:
        print("Repetitions overridden (B6): " + ", ".join(
            f"{enc} {subset}: {n} measured" for (enc, subset), n in args.reps_override.items()))
    taus = sorted({spec["threshold"] for spec in specs.values()})
    print(f"Mosaic benchmark: {len(plan)} runs ({len(encodings)} encodings x {len(specs)} subsets "
          f"x {len(reps)} reps{'' if args.no_warmup else ', rep 0 = warm-up'}), "
          f"tau = {'/'.join(map(str, taus))} (T = "
          f"{' / '.join(str(bc.threshold_abs(s['rows'], s['threshold'])) for s in specs.values())}), "
          f"delta {bc.DELTA}" + (f" (H3: {args.delta}, {args.h3_crawl} crawl)" if "h3" in encodings else "")
          + f", {args.threads} threads per server, channel buffers {args.channel_buffer} B, "
          f"ports {args.port_base}-{args.port_base + span - 1}")
    print(f"  results: {bc.relpath(results_file)}; configs and logs: {bc.relpath(RUN_DIR)}")
    print(f"  commit {env['commit']}{' (dirty)' if env['dirty'] else ''}, {env['cpu']}")
    if args.dry_run:
        for i, r in enumerate(plan, 1):
            print(f"  {i:3}. {r['encoding']:12} {r['subset']:5} rep {r['rep']}"
                  f"{' (warm-up)' if r['warmup'] else ''}  N = {specs[r['subset']]['rows']}  "
                  f"{bc.relpath(configs[r['encoding'], r['subset']])}")
        return 0

    t0 = time.perf_counter()
    try:
        if not args.no_build:
            build()
        if not args.skip_keygen:
            print("\nClient micro-benchmark (A6):", flush=True)
            for enc in encodings:
                run_client_bench(enc, configs[enc, "client-bench"], env, args, results_dir)
        print(flush=True)
        durations = {}
        for i, run in enumerate(plan):
            spec = specs[run["subset"]]
            left = estimate_left(plan, i, durations, costs)
            print(f"[run {i + 1} of {len(plan)}] {run['encoding']} {run['subset']} rep {run['rep']}"
                  f"{' (warm-up)' if run['warmup'] else ''}, N = {spec['rows']}; "
                  f"elapsed {fmt_time(time.perf_counter() - t0)}, left ~{fmt_time(left)}",
                  flush=True)
            truth = truths[run["encoding"], run["subset"]]
            rec = run_once(args, run, spec, configs[run["encoding"], run["subset"]], env,
                           results_file, truth, i % PORT_BLOCKS)
            durations[run["encoding"], run["subset"]] = rec["run_total_s"]
            sc, pk, b = rec["self_check"], rec["peak_mb"], rec["bytes"]["online"]["server_to_server"]
            print(f"    online {rec['online_ms'] / 1e3:.2f} s, upload {rec['upload_ms'] / 1e3:.2f} s, "
                  f"run {rec['run_total_s']:.1f} s; rounds {rec['rounds']['online']} online / "
                  f"{rec['rounds']['upload']} upload; server->server {b[0] / 2**20:.1f} / "
                  f"{b[1] / 2**20:.1f} MiB; peak memory {pk.get('server0', 0):.0f} / "
                  f"{pk.get('server1', 0):.0f} / {pk.get('client', 0):.0f} / "
                  f"{pk.get('dealer', 0):.0f} MiB (server 0 / 1 / client / dealer); self-check "
                  f"P {sc['precision']:.3f} R {sc['recall']:.3f} ({sc['cells']} cells), levels "
                  f"{'= ground truth' if sc['levels_equal_ground_truth'] else 'DIFFER'}",
                  flush=True)
    except KeyboardInterrupt:
        print("\nInterrupted; all Mosaic processes stopped.", file=sys.stderr)
        return 130
    except RunError as e:
        print(f"\nERROR: {e}", file=sys.stderr)
        return 1
    print(f"\nDone: {len(plan)} runs in {fmt_time(time.perf_counter() - t0)}; "
          f"results in {bc.relpath(results_file)}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
