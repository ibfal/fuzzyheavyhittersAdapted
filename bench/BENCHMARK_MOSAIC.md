# Benchmarking Mosaic (location heavy hitters)

This file is the source of truth for benchmarking the **Mosaic** codebase. It lives at `<mosaic-folder>/bench/BENCHMARK_MOSAIC.md`. The Mastic codebase has its own file, `BENCHMARK_MASTIC.md`.

The two benchmarks run **separately**, each inside its own codebase. They can only be compared at the end if the **common definitions** (Part A) are identical in both files. Change Part A in both files at the same time, or not at all.

Only two things cross between the codebases:

| What | Direction | Why |
|---|---|---|
| The data subsets (`bench/data/`) | Mastic → Mosaic | both systems must see exactly the same rides |
| The reference hotspots (`bench/reference/`) | Mosaic → Mastic | "accuracy" is measured against Mosaic's plaintext fuzzy definition |

All parameters have defaults. Change one only after agreeing with the professors, and then in **both** files.

---

# Part A: Common definitions (identical in both files)

## A1. Data

**Where it comes from.** The rides are RideAustin trips (Austin, Texas, 2016–2017), as cleaned and sampled by the Mosaic authors in their published artifact (Zenodo record 20768775, `fuzzyheavyhitters.zip`, folder `data/`). Our fork copied them into `mastic_for_fuzzy_HH/data/mosaic/` (see the `README.md` there). That folder holds **data only**; the Mosaic codebase is a separate repository.

| File in `data/mosaic/` | Contents | Use |
|---|---|---|
| `sample_busiest_day.csv` | 21,581 rides | **the benchmark data** |
| `sample_busiest_week.csv` | 59,040 rides | not used (Mosaic's own `results.json` run used this file with threshold 2%) |
| `sample_busiest_month.csv` | 115,174 rides (actually 14 days, 2017-03-31 to 2017-04-13) | not used |
| `start_busiest_day.json` | the day's pickups on Mosaic's grid as `[lat_grid, lon_grid]` | sanity check of the grid conversion |
| `results.json` | Mosaic's own experiment results | source for δ and Mosaic's parameters |

Only the columns `start_location_lat` and `start_location_long` are used. Coordinates are rounded to 0.001°. Mosaic's grid: `lat_grid = round((lat − 29.267)·1000)`, `lon_grid = round((lon + 98.743)·1000)`, 11 bits per axis. The day file is also the one referenced by `src/configs/ride-austin-day-*.toml` (`[dataset].path`).

The RideAustin terms apply to the subsets too (the data README says to check them before redistributing). Treat `bench/data/` like `data/mosaic/`.

**Subsets.** The benchmark measures each system at one third, two thirds and all of the clients:

1. Load `data/mosaic/sample_busiest_day.csv` from the Mastic repo.
2. Drop rows with missing or non-numeric coordinates (expected: none). Record the number dropped.
3. Shuffle all rows with `random.Random(20261107).shuffle`.
4. Take the first round(N/3), round(2N/3) and N rows of the shuffled order: **7,194 / 14,387 / 21,581** rides. The subsets are nested (`1of3` ⊂ `2of3` ⊂ `3of3`), and the spatial distribution is the same in all three, so only N changes.
5. Write `bench/data/day_1of3.csv`, `day_2of3.csv`, `day_3of3.csv` with the original header and columns.
6. Write `bench/data/manifest.json`: per file the path, row count, SHA-256, seed, and the source file with its SHA-256.

The subsets are generated **once, in the Mastic codebase**, and copied to the Mosaic codebase, which checks the SHA-256 values against the manifest.

Never subset with the driver's `-n`: it keeps the earliest rides, and the files are sorted by time. Always pass `-n` equal to the subset's row count.

## A2. Experiment matrix

| Factor | Values |
|---|---|
| Encoding | `mosaic_grid`, `geohash`, `h3`, `morton` |
| Subset | `day_1of3`, `day_2of3`, `day_3of3` |
| Threshold τ | per system, see below |
| Repetitions | 1 warm-up (discarded) + 5 measured |
| Malicious clients | none |

One threshold setting = 4 encodings × 3 subsets × 6 runs = **72 runs**.

**The two systems answer different questions**, so each runs at its own native threshold:

- **Mastic** counts the rides *inside* a cell: "which cells have at least T pickups in the cell itself?"
- **Mosaic** counts the rides *near* a square: "which squares have at least T pickups within δ squares in every direction (L∞)?"

Neighbourhood counts are much larger than in-cell counts, so the same τ means very different things in the two systems.

| System | τ | T for the three day subsets | Runs |
|---|---|---|---|
| Mastic | **0.005** (the existing day configs) | 35 / 71 / 107 | 72 (**done**) |
| Mosaic, native | **0.02** (Mosaic's code and its published `results.json`: 1180 = 2% of the week sample) | 143 / 287 / 431 | 72 (main) |
| Mosaic, matched to Mastic | 0.005 | 35 / 71 / 107 | 72, **optional**: run only if prompt 2's timing shows it is affordable |

Both use T = max(1, ⌊τ·N⌋). Mastic is not run at τ = 0.02: no single grid square contains 431 rides (the busiest has 214), so it would output nothing.

| Parameter | Value (same in both systems) |
|---|---|
| Weight | constant: every ride is one vote |
| Mosaic radius δ | **5** grid squares, L∞ (from Mosaic's `results.json`), i.e. an 11 × 11-square neighbourhood, about 1.2 km × 1.1 km |
| `mosaic_grid` | origin (29.267, −98.743), 0.001° cells, 11 bits per axis, n = 22 |
| `geohash` | prefix `9v6`, 21 bits, n = 21 (area-matched to the grid) |
| `h3` | prefix `82489ffffffffff`, resolution 10, n = 24 (area-matched to the grid) |
| `morton` | lat 29.9005–30.9245, lon −98.0505 to −97.0265, 10 bits per axis, n = 20 |

If an encoding uses different parameters in either codebase, align them before benchmarking.

## A3. Phases

| Metric | Definition |
|---|---|
| **Client report generation** | Time for one client to turn its location into the reports for all servers, on one thread (encoding, key generation, proofs, share splitting). Measured with a micro-benchmark (A6). |
| **Server offline** | Server work that does not depend on client data and could be done before the reports arrive. |
| **Upload** | From the first report sent until the last report is stored on all servers. Reported separately, not part of online. |
| **Online** | From the start of aggregation (all reports stored) until the collector holds the final output. Wall-clock on localhost. |
| **Online steps** | The online phase broken into its natural steps (per level, per protocol step). |

"Offline" in the report means client report generation **and** server offline work, shown as two separate numbers. Per-client time is never added to server time.

## A4. Communication

**Rounds.** A round is one sequential step in which a party sends messages and must wait for the replies before continuing. Messages sent concurrently to several parties in the same step count as one round. Report `upload`, `online` and `extra` rounds separately.

**Bytes.** Application-level size of every message **as actually sent** (i.e. with the encoding the RPC layer really uses), summed per link and per phase. Framing overhead (TCP, RPC headers) is not counted.

| Link | Meaning |
|---|---|
| `client_to_server` | report shares, per server |
| `server_to_server` | direct traffic between the servers |
| `collector_to_server` | requests from the collector, per server |
| `server_to_collector` | responses to the collector, per server |

**Communication time (modelled).** On localhost the network costs almost nothing, so communication time is modelled:

    t_comm(phase) = R_phase × RTT + max_link(B_link,phase) / bandwidth
    t_online_est  = online_ms (measured)  +  t_comm(online)

| Profile | RTT | Bandwidth |
|---|---|---|
| LAN | 0.5 ms | 1 Gbit/s |
| WAN | 50 ms | 100 Mbit/s |
| Client uplink (upload only) | — | 10 Mbit/s |

**Upload in the model.** `rounds.upload` records the upload rounds **as implemented** (the simulator sends batches one after another). The deployment model treats upload per client: each client uploads once, so client upload time = 1 × RTT + report bytes / client uplink. Upload is never added to online time.

## A5. Accuracy

All reported locations are converted to the Mosaic grid (cell centre → grid coordinates) before comparison.

**Self-check (correctness).** Compare each run's output with the plaintext answer of the same system, encoding, subset and τ: precision, recall, and whether every count is exact. Expected 1.0 / 1.0 / exact; anything else is a bug and blocks the benchmark.

**Reference: Mosaic's fuzzy hotspots.** Computed by the Mosaic harness, replicating Mosaic's plaintext definition exactly, on the Mosaic grid with **τ = 0.02 and δ = 5**, for each subset:

- **Q:** the set of qualifying squares (neighbourhood count ≥ T), each with its neighbourhood count. Full day: 916 squares.
- **Areas:** Q split into connected groups of squares that touch, including diagonally (8-neighbourhood). Each area has a peak square, the one with the highest neighbourhood count. Full day: 4 areas, the largest downtown.

File `bench/reference/day_<subset>.json` (the numbers are only an illustration of the format):

```json
{"dataset": "day", "subset": "3of3", "tau": 0.02, "T": 431, "delta": 5,
 "squares": [{"grid": [lat_grid, lon_grid], "count": 1192, "area": 0}],
 "areas":   [{"id": 0, "peak": [lat_grid, lon_grid], "peak_count": 3238, "size": 870}]}
```

**Accuracy against the reference**, for every run of either system (all locations on the Mosaic grid):

| Metric | Definition |
|---|---|
| `area_recall` | share of reference areas with at least one reported location within L∞ ≤ δ of a square of that area |
| `precision` | share of reported locations within L∞ ≤ δ of some square in Q |
| `f1` | 2·P·R / (P + R) |
| `areas_found` / `reference_areas` | number of areas hit / number of areas |
| `outputs` / `reference_squares` | number of reported locations / \|Q\| |

Counts are not compared: an in-cell count and a neighbourhood count measure different things, so a count error would be large by construction.

## A6. Client micro-benchmark

One thread; the first K = **2,000** rows of `day_3of3.csv`; one warm-up pass. Time each client's report generation individually and report the mean, median and p95 (ms); also report the serialized report size per server and the total upload per client (bytes). Run once per encoding.

## A7. Leakage

Measured per run (`leakage` in the result line):

| Field | Meaning |
|---|---|
| `revealed_aggregates_total` | number of aggregate values the collector reconstructs during the run |
| `revealed_aggregates_per_level` | the same, per level or step |
| `revealed_kind` | what one revealed value is: `"count"`, `"bit"`, … |
| `client_validity_bits` | number of per-client accept/reject results revealed |
| `frontier_per_level` | surviving candidates per level |

Written analysis (`bench/LEAKAGE_<system>.md`): one table. Rows are what is revealed. Columns:

- to whom (each server, collector, network observer, clients);
- when (phase, level);
- how much (formula + measured value for `day_3of3` per encoding);
- category: **intended output**, **protocol leakage** accepted in the paper (cite the section), or **implementation-only** leakage (cite `file:line`).

Then a section on how the encoding changes the leakage.

## A8. Environment

- Same laptop for all runs: plugged in, high-performance power mode, heavy applications closed. All parties on localhost.
- Release builds only.
- Threads: each server process uses ⌊physical cores / 2⌋ threads (at least 1). The runner sets this and records it, and it stays fixed for the whole benchmark.
- Loop order: repetition outermost, so configurations interleave and thermal throttling spreads evenly.
- Report the median of the 5 measured runs, plus min and max.
- Record per run: CPU model, physical/logical cores, RAM, OS, compiler version, git commit and dirty flag.

## A9. Result schema

One JSON object per line in `bench/results/raw/<system>.jsonl`, where `system` is `mastic` or `mosaic`. Per-server values are `[server0, server1]`. Fields that don't apply to a system are 0 or `null`, never left out.

```json
{
  "schema_version": 1,
  "system": "mastic",
  "encoding": "geohash",
  "dataset": "day",
  "subset": "1of3",
  "n_clients": 7194,
  "rep": 2,
  "warmup": false,
  "timestamp": "2026-11-10T14:03:11+01:00",
  "params": {"data_bits": 21, "threshold_frac": 0.005, "threshold_abs": 35, "delta": null,
             "encoding_params": {"prefix": "9v6", "bits": 21}},
  "env": {"cpu": "...", "cores_physical": 8, "cores_logical": 16, "ram_gb": 16, "os": "...",
          "threads": 4, "compiler": "rustc 1.xx", "commit": "abc123", "dirty": false},
  "phases": {"server_offline_ms": 0, "upload_ms": 812.4, "online_ms": 4210.7,
             "online_steps": [{"name": "flp", "ms": 640.2}, {"name": "level_0", "ms": 151.3}]},
  "rounds": {"upload": 1, "online": 47, "extra": 0},
  "bytes": {"upload": {"client_to_server": [0, 0]},
            "online": {"collector_to_server": [0, 0], "server_to_collector": [0, 0],
                       "server_to_server": 0}},
  "leakage": {"revealed_aggregates_total": 694, "revealed_aggregates_per_level": [2, 4],
              "revealed_kind": "count", "client_validity_bits": 7194,
              "frontier_per_level": [1, 2]},
  "output": [{"label": "9v6s0j9", "lat": 30.265, "lon": -97.732, "count": 120}]
}
```

The client micro-benchmark writes the same header fields plus the following to `bench/results/raw/<system>_client.jsonl`:

```json
"client": {"k": 2000, "keygen_ms_mean": 0.41, "keygen_ms_median": 0.39, "keygen_ms_p95": 0.52,
           "report_bytes_per_server": [2210, 2210], "upload_bytes_per_client": 4420}
```

## A10. Outputs

- `bench/results/summary_<system>.csv`: one row per encoding × subset × τ (`threshold_frac` is a column and part of the row key), with the median, min and max of every metric, modelled LAN/WAN times, client metrics, leakage counters and accuracy. **The columns are identical for both systems**, so the two files can be concatenated.
- `bench/results/tables.md`: timings, communication, client cost, accuracy, leakage.
- `bench/results/plots/` (PNG + PDF, one colour per encoding):
  1. online time vs N (LAN and WAN panels);
  2. bytes vs N;
  3. rounds per encoding;
  4. client time and report size per encoding;
  5. area recall / precision per encoding (and per τ);
  6. revealed aggregates vs N.
- **Comparison (at the end):** copy the other system's `summary_<system>.csv` into `bench/results/` and run `evaluate.py --combine <file>`. This produces the side-by-side tables and plots with one marker per system.

---

# Part B: Mosaic specifics

## B1. Folder layout

```
<mosaic-folder>/                     (branch `benchmarking`)
├── bench/
│   ├── BENCHMARK_MOSAIC.md          ← this file
│   ├── README.md                    ← exact commands (written by Claude Code)
│   ├── data/                        ← subsets + manifest.json, COPIED from the Mastic harness
│   │                                   (+ any converted input files Mosaic needs, e.g. grid JSON)
│   ├── ground_truth/                ← Mosaic's plaintext answer per encoding, subset and τ
│   ├── reference/                   ← reference hotspots (A5), produced here, copied to Mastic
│   ├── scripts/
│   │   ├── bench_common.py          ← schema check, environment info, JSONL helpers
│   │   ├── verify_subsets.py        ← checks bench/data against manifest.json
│   │   ├── convert_inputs.py        ← only if Mosaic needs another input format (B2.9)
│   │   ├── ground_truth.py
│   │   ├── run_bench.py
│   │   └── evaluate.py
│   ├── tests/test_evaluate.py
│   ├── requirements.txt             ← Python packages for the bench scripts
│   ├── reports/                     ← one report per Claude Code prompt (C0)
│   ├── results/raw/  results/plots/  summary_mosaic.csv  tables.md
│   └── LEAKAGE_mosaic.md
└── (instrumented Mosaic code + client micro-benchmark)
```

## B2. What is known, and what Claude Code has to find out first

**Already known** (from an earlier analysis of this repo; prompt 1 verifies it in the code):

- Mosaic outputs every grid **square** whose neighbourhood (all squares within δ = 5 in every direction, L∞) contains at least T pickups.
- On the full day sample with T = 431 (τ = 0.02), that is **916 squares**, which form **4 connected areas**; the largest is downtown.
- Of Mastic's 8 heavy cells, the 6 downtown ones have 1,192–3,238 rides within δ and are in Mosaic's result. The 2 airport cells have 409 and 396, just below 431, and are not.

**Prompt 1 asks Claude Code to work out the following**, and the answers go into B3–B5 of this file:

1. The language, build steps, and which parties exist in a run (clients, servers, collector), and how they communicate.
2. How the code splits the work into offline and online, and how that maps to A3. In particular: does the server do input-independent preprocessing (e.g. for garbled circuits or OT)? If so, it is **server offline**, measured separately from client report generation.
3. Every communication round per phase and where messages are sent, so rounds and bytes can be counted per A4. Unlike Mastic, Mosaic's servers probably talk to each other directly, so `server_to_server` is likely non-zero.
4. Mosaic's exact plaintext definition of a fuzzy hotspot and where the code computes it: candidate squares (all squares, or only squares with a ride?), how the neighbourhood count is formed, the comparison (≥ or >), and how the threshold is set (a fraction of N or an absolute number such as 431). This definition becomes the reference in A5.
5. What the output looks like (squares with or without counts), and whether the code itself groups squares into areas.
6. What each party learns during a run (needed for B5).
7. How the four encodings are configured, whether their parameters equal A2, and **what "within δ" means for each encoding** (for the grid and Morton it is presumably L∞ on grid indices; for geohash and H3 it must be stated).
8. Whether the Mosaic thesis reports runtimes, communication or rounds for these samples, and on what hardware. These are useful sanity checks.
9. **The input format.** Does the code read the CSV (lat/lon) or Mosaic's grid points (`start_busiest_*.json`, `[lat_grid, lon_grid]`)? The benchmark must use the subsets copied from Mastic. If Mosaic needs another format, `convert_inputs.py` produces it from those subsets with Mosaic's grid formula, and checks that converting the full day CSV reproduces the repo's own `start_busiest_day.json` exactly (the Mastic harness already found 21,581 of 21,581 points equal).

## B3. How the phases map onto the Mosaic code

*(Filled in after prompt 1.)*

| Metric | Mosaic code |
|---|---|
| Client report generation | ⟨…⟩ |
| Server offline | ⟨… or 0⟩ |
| Upload | ⟨…⟩ |
| Online | ⟨…⟩ |
| Online steps | ⟨…⟩ |

## B4. Accuracy in Mosaic

- **Self-check:** compare each run with Mosaic's own plaintext answer (`bench/ground_truth/day_<subset>_<encoding>_tau<τ>.json`), computed by replicating the definition found in B2.4 exactly, for each τ that is run. Cite the file and lines it follows.
- **Reference (A5):** the same plaintext definition on the Mosaic grid with τ = 0.02 and δ = 5, plus the areas (8-neighbourhood connected groups) and their peak squares, written to `bench/reference/day_<subset>.json` in the A5 format. Copy this folder to the Mastic codebase.
- For `mosaic_grid` at τ = 0.02, the self-check answer and the reference are the same squares, so its area recall and precision are 1.0 by construction. The other encodings and τ = 0.005 are measured against the same reference, so every run of both systems is compared with one common answer.
- **Sanity check:** for the full day, the reference must contain 916 squares in 4 areas (B2).

## B5. Leakage in Mosaic (starting list for `LEAKAGE_mosaic.md`)

To be filled from the code and the thesis. Check at least:

1. **Intended output:** the hotspots, and whether their counts are revealed or only that they are above the threshold.
2. **Protocol leakage:**
   - what is opened during the search (counts per candidate or prefix, or only above/below-threshold bits from the garbled-circuit comparison);
   - how many values are opened per level;
   - public parameters (N, T, δ, grid size).
3. **Revealed for verification:** any per-client validity results.
4. **Security model:** semi-honest servers vs. malicious clients. State it next to the leakage, because it differs from Mastic.
5. **Implementation only:** deployment simplifications (e.g. all parties in one process, unencrypted channels, a single process playing several roles). Cite `file:line`.
6. **Encoding effect:** what regions or points the opened values describe for each encoding. Use the measured number of revealed values per run.

## B6. Cost on a laptop, and the optional τ = 0.005 runs

- Not known yet. Garbled circuits and OT can be much heavier than Mastic's VIDPF evaluations (Mastic: about 6 s per full-day run).
- The Mosaic authors published these samples with their artifact, so the day sample should be feasible. Check the thesis for their reported runtimes and hardware.
- Prompt 2 measures one `day_3of3` run per encoding at τ = 0.02, and one `mosaic_grid` `day_3of3` run at τ = 0.005 (time and peak memory).
  - **Main runs (τ = 0.02):** if a run takes more than about 10 minutes, reduce to 3 measured repetitions for `day_3of3`, run overnight, and record the change.
  - **Matched runs (τ = 0.005):** with a lower threshold, far more squares qualify, so the search prunes less and takes longer. Run them only if the full set fits in about one night; otherwise run only `mosaic_grid` at τ = 0.005 (18 runs) as a single data point, and record the decision.
  - Don't change the subsets: they must stay identical to Mastic's.

## B7. Validation checklist (before the full run)

- [ ] `verify_subsets.py` passes: the SHA-256 of every subset matches Mastic's `manifest.json`.
- [ ] Encoding parameters equal A2; mismatches resolved.
- [ ] Input conversion (if needed): converting the full day CSV reproduces `start_busiest_day.json` exactly.
- [ ] Reference for the full day: 916 squares in 4 areas.
- [ ] Smoke run on `bench/data/ten_locations.csv` (copied from the Mastic repo's `data/synthetic/`): output equals Mosaic's plaintext answer for every encoding.
- [ ] Every result line passes the schema check.
- [ ] Round and byte counts are plausible against the thesis (if it reports any).
- [ ] Self-check is 1.0 / 1.0 / exact for every run.
- [ ] One `day_3of3` run per encoding has been timed.

---

# Part C: Running it

## C0. Rules for Claude Code (apply to every prompt)

1. **No git operations that change anything.** Do not run `git commit`, `git push`, `git checkout`, `git switch`, `git branch`, `git reset`, `git stash`, `git merge`, `git rebase`, `git restore` or `git clean`. Read-only git (`status`, `log`, `diff`, `show`, `rev-parse`) is fine. Leave all changes uncommitted. I commit and push myself.
2. **Stay on the current branch** (`benchmarking`). If it looks wrong, stop and tell me.
3. **This file is the source of truth.** Ask me before deviating from it. If something in the code doesn't fit it, explain the mismatch and propose a resolution instead of silently choosing.
4. **Don't change protocol behaviour.** Measurement code only. List every existing file you modify and why.
5. **Don't start the full benchmark.** Run only the smoke runs and single timing runs each prompt asks for.
6. **Windows.** Scripts must run in PowerShell (with the repo's Python environment, if it has one). No bash, no shell redirection. Write output files directly from code.
7. **Report at the end of every prompt.** Write `bench/reports/NN-<name>.md` and show the same text in the chat, with these sections:
   - **Summary:** what was done, in plain words; 3–6 sentences.
   - **Files:** created and modified, one line each on why.
   - **How to run:** exact commands.
   - **Checks:** each check from this file, with the numbers and pass/fail.
   - **Deviations and open questions:** anything that differs from this file or needs my decision.
   - **What to commit:** the exact `git add …` command (specific paths, never `git add .`) and a one-line commit message. Leave out virtual environments, caches, build output and temporary files.