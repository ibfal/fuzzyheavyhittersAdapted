# 01: Benchmark harness (Mosaic)

## Summary

This prompt answers B2 from the code and builds the Mosaic harness in `bench/`. No Mosaic code was changed.

- **Subsets:** they match Mastic's manifest byte for byte.
- **Input conversion:** Mosaic's own converter turns the subsets into its JSON points for all four encodings. The full day reproduces `start_busiest_day.json` exactly, 21,581 of 21,581 points.
- **Ground truth:** written for every encoding, subset and τ (0.02 and 0.005), with H3 at δ = 5 and δ = 4. Two independent Python methods agree, and so does Mosaic's own plaintext, `mosaic_others`.
- **Reference:** the full day gives 916 squares in 4 areas, as expected.
- **Spec:** Part A was copied from the updated Mastic spec and is byte-identical. B3 and B5 are filled in.

**One thing needs your attention:** the merged H3 change (commit 5623af8) changes the servers' crawl in `protocol.rs`, not only the encoding. It is correct and gives the same cells; details in Checks, item 2.

## Files

Created:
- `bench/scripts/verify_subsets.py`: checks the subsets against `manifest.json` (SHA-256, rows, LF, nesting); exits 1 with a FAIL banner on any mismatch.
- `bench/scripts/convert_inputs.py`: runs `ride_austin_json_generator` for 4 encodings × 4 CSVs, writes a manifest, and runs checks (a)–(g).
- `bench/scripts/ground_truth.py`: the plaintext answer (crawl + dense, optional `mosaic_others` cross-check), the decoded cells, the reference, and smoke mode.
- `bench/requirements.txt`: numpy, psutil, matplotlib, pytest.
- `bench/README.md`: the exact commands.
- `bench/reports/01-harness.md`: this report.
- `bench/data/encodings/<enc>/{day_1of3,day_2of3,day_3of3,ten_locations}.json` + `.encoding.json` and `bench/data/encodings/manifest.json`: Mosaic's input points (5.0 MB).
- `bench/ground_truth/day_<subset>_<variant>_tau<τ>.json`: 30 files.
- `bench/ground_truth/smoke_ten_locations_<variant>_tau0.1.json`: 5 files.
- `bench/reference/day_{1of3,2of3,3of3}.json`: the A5 reference.

Modified:
- `bench/scripts/bench_common.py`: now based on Mastic's current spec-v3 version (the copy in this repo was older). Only the Mastic-specific parts changed:
  - `SYSTEM`, τ = 0.02, the method;
  - Mosaic's `ENCODINGS` (d, h1, crawl levels, δ, generator flags) and `GT_VARIANTS`;
  - Mosaic's grid formula `to_grid()` and `release_binary()`, instead of Mastic's plaintext encoders;
  - `compare_configs()` for Mosaic's JSON configs;
  - the schema: A9 v3.1 additions, and δ required on Mosaic lines.
  - The environment record (rustc), JSONL append and file helpers are unchanged.
- `bench/BENCHMARK_MOSAIC.md`: Part A replaced by the Mastic Part A, verbatim; B3 and B5 filled. Nothing else changed.
- `.gitignore`: adds `.venv/`, `__pycache__/`, `.pytest_cache/`.

Not touched: `src/`, `configs/`, `bench/scripts/evaluate.py`, `bench/tests/test_evaluate.py`, `bench/data/*.csv`, `bench/data/manifest.json`. Also outside the repo: `.venv/` (a new virtual environment) and `target/release/` (build output).

## How to run

```powershell
python -m venv .venv
.\.venv\Scripts\python.exe -m pip install -r bench\requirements.txt
cargo build --release --bin ride_austin_json_generator --bin heavy_hitters_decoder --bin mosaic_others
.\.venv\Scripts\python.exe bench\scripts\verify_subsets.py --source C:\Users\arabe\IMP\mastic_for_fuzzy_HH\data\mosaic\sample_busiest_day.csv
.\.venv\Scripts\python.exe bench\scripts\convert_inputs.py --source-dir C:\Users\arabe\IMP\mastic_for_fuzzy_HH\data\mosaic
.\.venv\Scripts\python.exe bench\scripts\ground_truth.py --check-rust
.\.venv\Scripts\python.exe bench\scripts\ground_truth.py --csv bench\data\ten_locations.csv --threshold 0.1 --check-rust
.\.venv\Scripts\python.exe bench\scripts\bench_common.py --self-test
.\.venv\Scripts\python.exe bench\scripts\bench_common.py
```

Timings on this laptop:
- release build: 2 min 15 s;
- `convert_inputs.py`: 2 s;
- `ground_truth.py`: 13 min with `--check-rust`, mostly the authors' 3-coordinate H3 crawl (up to 160 s per case). Without it, under a minute (estimated from the per-case timings of crawl + dense, ≤ 1.3 s each);
- smoke run: 5 s.

## Checks

### 1. Step 1: answers to B2 (citations at HEAD 5623af8)

| B2 | Answer |
|---|---|
| 1. Language, parties | **Language:** Rust 2018 (`Cargo.toml:6`), with swanky GC/OT (`:20-22`) and h3o (`:19`).<br>**Parties:** four processes, `server0`, `server1`, one client simulator for all N clients, and the dealer.<br>• The dealer is mandatory: the servers always connect to it (`mosaic_server.rs:81-98`) and send it `Shutdown` (`protocol.rs:755-757`).<br>• There is **no collector**: both servers learn the output (`protocol.rs:994-1073`).<br>**Transport:** plain TCP (`channel.rs:9-128`), with `threads` parallel channels between the servers and to the dealer. |
| 2. Offline / online | **Client:** keys are made before connecting (`mosaic_client.rs:93-101`); sketch data after the shares are sent (`:121-134`).<br>**Servers:** receive → verify sketches (`mosaic_server.rs:248-299`) → crawl (`protocol.rs:559-760`).<br>**No server preprocessing:** OT/GC setup is input-independent but runs inline for every candidate (`batch_equality_full.rs:33-37`; `check_phase.rs:478-496`). → **server offline = 0**. |
| 3. Rounds and messages | **Upload:** 1 round (two messages per server, no replies).<br>**Online:**<br>• sketch verification: Beaver exchanges plus one opening (`protocol.rs:242-253`);<br>• then the sequential crawl iterations (`:605-731`). Each runs per thread, one after another: one GC equality test plus one OT conversion per candidate, one GC ≥ T per chunk, then one bit exchange (`:994-1073`);<br>• H3: plus one final `batch_check` (`:827`).<br>**Bytes:** counted at application level (`channel.rs:54-65`). server_to_server is asymmetric (README: server1 sends about 4× what server0 sends). |
| 4. Plaintext definition | Every point x of [0, 2^h1)^d with #{y : max_i \|x_i − y_i\| ≤ δ} ≥ T (`mosaic_others.rs:40-148`, `:77-84`; `util.rs:187-199,279-281`).<br>• All points of the domain are candidates, not only squares with rides.<br>• The comparison is ≥ (`mosaic_others.rs:84`; GC `greater_than_or_equal_threshold.rs:210-276`).<br>• T is absolute (`cli_config.rs:28-29`), = ⌊0.02 N⌋ in the authors' configs.<br>• No wrap-around (`share_phase.rs:58-64`).<br>• H3: cells with ≥ T clients within δ hexagon steps (see item 2). |
| 5. Output | Points without counts (`protocol.rs:742-753`). The server prints only how many there are (`mosaic_server.rs:346-351`). Nothing in `src/` groups squares into areas. |
| 6. Who learns what | Filled into B5. Both servers learn one ≥ T bit per checked prefix, the output, and one validity bit per client. The dealer learns nothing with gc_gc; the client only gets "hi". A network observer sees everything, because TCP is unencrypted. |
| 7. Encodings and "within δ" | Grid, geohash and Morton have the same cells as A2, and so does H3 (still resolution 10, encoder unchanged).<br>**Crawl levels n:** grid 22, geohash 22 (one dummy longitude level), **H3 23** (2 × 11 + 1 derived check), Morton 20. These are now recorded in the A2 note of Part A.<br>**"Within δ" (L∞ on the coordinates):**<br>• grid/Morton: 11×11 squares (1.29 km²);<br>• geohash: 11×11 cells of 76 m × 132 m (1.22 km²);<br>• H3: hexagon grid distance (`encoding/tests.rs:177-184`). δ = 5 → 91 hexagons ≈ 1.37 km²; δ = 4 → 61 ≈ 0.92 km². |
| 8. Thesis numbers | README, AWS LAN, servers on c5a.24xlarge with 32 threads; day, L∞, FSS, gc_gc:<br>• 32.4 s;<br>• all clients together: 192 MB, 2.91 s;<br>• server0 sends 2.74 GB, server1 sends 11.6 GB;<br>• sketching 6.86 s (`README.md:76-163,339-347`).<br>No rounds are reported. The paper's Table 4 is older (`:325`). The paper PDF couldn't be read: there is no PDF renderer here. |
| 9. Input format | JSON integer points (`mosaic_client.rs:6-13`), produced from the CSV by `ride_austin_json_generator`. → `convert_inputs.py`. |

**"Already known" facts (B2):**

| Fact | Result | Status |
|---|---|---|
| Full day, T = 431 | 916 squares in 4 areas: sizes 770 / 128 / 17 / 1; downtown peak (999, 1002) with 3,427 rides | pass |
| Mastic's 6 downtown cells | 1,192–3,238 rides within δ (2,074; 1,192; 2,269; 2,030; 3,238; 2,021), all in Q | pass |
| Mastic's 2 airport cells | (935, 1076) has 409, (936, 1076) has 396; neither is in Q | pass |

### 2. The merged H3 change (`git diff f63ee7d..5623af8`)

**What it changes:** the protocol, for H3 only.
- `protocol.rs` (+167 lines) gets a new optional config field, `derived_coordinate` (`cli_config.rs:51-99`). With it, the servers' crawl branches only on i and j (`protocol.rs:584,630-631`).
- Each surviving (i, j) point then gets k = i − j + 1024 and one extra `batch_check` with all three coordinates (`protocol.rs:733-740,768-840`). That adds one more opened bit per point.
- `mosaic_others` and `crawl.rs` follow the config.

**What it doesn't change:** `git diff --quiet` passes for the encoder, `client.rs`, the share/check/threshold/sketch phases, the dealer, the GC/OT code, `channel.rs`, `util.rs`, the server/client/dealer binaries, the generator and the decoder.
- Without the field, every code path is the old one, so grid, geohash and Morton are unaffected.
- The other merged commit (`eec5c8e`) only gates unused SIMD helpers on x86_64.

**Is the output the same?** Yes. On our data, for every subset × τ × H3 δ (12 cases), the authors' 3-coordinate crawl restricted to cells equals our answer.
- The 3-coordinate crawl also outputs 4,573–48,349 non-cell triples per case. The new crawl outputs none.
- For the full day at δ = 4 it reproduces the README exactly: 445 cells + 4,573 non-cells (33 levels) vs 445 cells, 23 levels and 2,577 checked prefixes (new).

**Decision recorded (plan approval):** H3 runs use `derived_coordinate`. B5.5 notes that this is the fork's protocol change, not the authors'.

### 3. Harness checks

| Check | Result | Status |
|---|---|---|
| Branch | `benchmarking`. I made no git writes. During the prompt HEAD moved from 5623af8 to `479c75b` ("H3 encoding update"): your commit of the harness while `ground_truth.py` was still running (see What to commit) | pass |
| `verify_subsets.py` | 7,194 / 14,387 / 21,581 rows; SHA-256 match the manifest; LF only; finite coordinates; nested (1of3 ⊂ 2of3 ⊂ 3of3 as leading rows) | pass |
| `verify_subsets.py --source` | source hash matches `sha256_lf`; `day_3of3` is a permutation of the source rows | pass |
| Fails loudly | a tampered manifest (in memory) gives 2 FAIL lines, a FAIL banner and exit 1 | pass |
| Part A | copied from the Mastic spec (commits 8558b33, e424011): **byte-identical**, 15,374 bytes, SHA-256 `19281179…` in both files (CRLF on disk); also identical after LF normalisation | pass |
| Schema self-test | both A9 examples and the Mosaic v3.1 example are valid; all 10 broken copies are rejected (incl. a 3-element `server_to_server`, `extra` that isn't an object, a Mosaic line without δ) | pass |
| Environment record | i7-1255U, 10 physical / 12 logical cores, 15.8 GB, Windows 11, rustc 1.99.1, **threads 5**, commit 5623af8, `dirty: true` (this prompt's uncommitted edits) | pass |
| Repo configs vs harness | grid, geohash, Morton and `gc_gc.json` equal; **H3 configs have δ = 4** (harness: 5; runs will write their own configs) | as expected |
| Release build (3 binaries) | 2 min 15 s, no errors | pass |
| (a) generator(full day CSV) = `start_busiest_day.json` | 21,581 of 21,581 points equal in file order | pass |
| (b) Python grid formula(full day CSV) = `start_busiest_day.json` | 21,581 of 21,581 | pass |
| (c) Python formula = generator `mosaic_grid` | 7,194 / 14,387 / 21,581 / 165 of the same | pass |
| (d) `day_3of3` grid = `start_busiest_day.json` as a multiset | equal | pass |
| (e) Morton = grid − (634, 693) | every point of every CSV | pass |
| (f) dropped / clamped rides | 0 / 0 for all 4 encodings × 4 CSVs, so N = rows | pass |
| (g) H3 points satisfy k = i − j + 1024 | every point | pass |
| Crawl = dense (points and counts) | all 30 day answers and 5 smoke answers | pass |
| `mosaic_others` = ours | all 35 answers; H3 also without `derived_coordinate`, restricted to cells (14 answers incl. smoke) | pass |
| Morton = grid shifted, in the ground truth | every subset and τ | pass |
| Decoder | every point decodes to a cell (0 non-cells); every H3 cell's res-2 parent is `82489ffffffffff` (A2's prefix) | pass |
| Reproducible | re-running `convert_inputs.py` gives 33 of 33 identical files; re-running the smoke gives identical hashes | pass |
| PowerShell | the README commands run in PowerShell with exit code 0; no `__pycache__` written | pass |

### 4. Smoke ground truth: `ten_locations.csv`, threshold 0.1 (N = 165, T = 16)

| Variant | Points | Counts | Prefixes checked (levels) |
|---|---|---|---|
| `mosaic_grid` | 484 = 4 × 121 | 50 / 40 / 30 / 20, 121 squares each | 1,334 (22) |
| `geohash` | 484 | the same | 1,384 (22) |
| `h3` (δ 5) | **359** | 50: 91, 40: 86, 30: 86, 20: 91, **70: 5** | 1,821 (23) |
| `h3_delta4` | 244 = 4 × 61 | 61 each | 1,318 (23) |
| `morton` | 484 | the same as the grid | 1,254 (20) |

In H3 at δ = 5, the Capitol and UT Tower hexagon neighbourhoods share 5 cells, each with 40 + 30 = 70 rides: 364 − 5 = 359, as predicted (≤ 364). There are 0 non-cells everywhere. The authors' 3-coordinate crawl finds 5,289 points for H3 δ 5: our 359 cells plus 4,930 non-cells.

### 5. Qualifying points per encoding (all are cells; H3 counts hexagons)

| Variant | 1of3 τ .02 (T 143) | 1of3 τ .005 (T 35) | 2of3 τ .02 (T 287) | 2of3 τ .005 (T 71) | 3of3 τ .02 (T 431) | 3of3 τ .005 (T 107) |
|---|---|---|---|---|---|---|
| `mosaic_grid` | 944 | 5,008 | 931 | 4,901 | **916** | 4,870 |
| `geohash` | 925 | 5,176 | 895 | 5,026 | 891 | 4,960 |
| `h3` (δ 5) | 734 | 3,640 | 730 | 3,592 | 721 | 3,539 |
| `h3_delta4` | 452 | 2,541 | 449 | 2,444 | 445 | 2,436 |
| `morton` | 944 | 5,008 | 931 | 4,901 | 916 | 4,870 |

Prefixes checked, i.e. bits revealed to both servers (A7):

| Variant | 1of3 .02 | 1of3 .005 | 2of3 .02 | 2of3 .005 | 3of3 .02 | 3of3 .005 |
|---|---|---|---|---|---|---|
| `mosaic_grid` | 2,666 | 12,452 | 2,676 | 12,336 | 2,630 | 12,164 |
| `geohash` | 2,684 | 12,814 | 2,640 | 12,698 | 2,612 | 12,484 |
| `h3` | 3,783 | 15,371 | 3,832 | 15,416 | 3,826 | 15,117 |
| `h3_delta4` | 2,612 | 11,944 | 2,610 | 11,844 | 2,577 | 11,597 |
| `morton` | 2,716 | 12,294 | 2,678 | 12,184 | 2,650 | 12,028 |

The per-level numbers are in each file (`candidates_per_level`, `frontier_per_level`). For example, the grid on the full day at τ 0.02 keeps 2, 4, 4, …, 278, 503, 916.

**README cross-check (full day, T = 431):** grid 916, Morton 916, geohash 891, H3 δ4 445 cells in 23 levels with 2,577 prefixes. All six match (`README.md:97,649,655,673`).

### 6. Reference (A5): grid, τ 0.02, δ 5

| Subset | N, T | Squares | Areas: size, peak (lat_grid, lon_grid), peak count |
|---|---|---|---|
| `day_1of3` | 7,194, 143 | 944 | 3 areas:<br>• 774, (999, 1002), 1,127<br>• 133, (1020, 998), 253<br>• 37, (985, 979), 151 |
| `day_2of3` | 14,387, 287 | 931 | 5 areas:<br>• 767, (998, 1002), 2,266<br>• 131, (1020, 998), 497<br>• 30, (992, 982), 307<br>• 2, (987, 979), 290<br>• 1, (985, 979), 288 |
| `day_3of3` | 21,581, 431 | **916** | **4 areas**:<br>• 770, (999, 1002), 3,427<br>• 128, (1019, 998), 727<br>• 17, (992, 982), 452<br>• 1, (987, 979), 432 |

The full day matches the expected 916 squares in 4 areas (B4, B7): **pass**.

### 7. B7 checklist

| Item | Status |
|---|---|
| `verify_subsets.py` passes | done |
| Encoding parameters equal A2; mismatches resolved | done. Same cells; n per encoding is in the Part A note. δ = 5 for all; H3 also δ = 4 in the ground truth. |
| Input conversion reproduces `start_busiest_day.json` | done |
| Reference: 916 squares in 4 areas | done |
| Smoke run equals the plaintext for every encoding | ground-truth side done; Mosaic runs in prompt 2 |
| Schema check | ready (self-test passes) |
| Rounds/bytes vs thesis, self-check, timing | prompt 2 |

## Deviations and open questions

1. **H3 runs measure the fork's crawl (decided at plan approval).** Commit 5623af8 changes `protocol.rs` for configs with `derived_coordinate`.
   - The output is identical to the authors' crawl restricted to cells, on all 12 cases.
   - The revealed bits differ: fewer prefixes, (i, j) boxes, one final check level.
   - B5.5 records it as the fork's change. If you want the authors' 33-level crawl instead, prompt 2 can omit the field; non-cell outputs then come back, and decision 3 applies.
2. **Mastic's `bench_common.py` doesn't accept the v3.1 additions yet:** an `extra` field, a list for `server_to_server`, a null `count`/`lat`/`lon`. The Mosaic copy does. Give Mastic's schema check the same change before Mosaic raw lines are read there. Only summaries cross over today, so nothing breaks now.
3. **Reference file format.** It has A5's keys plus `N` and `source` (path and SHA-256 of the subset). Tie rules, which A5 doesn't fix:
   - a peak is the highest count, ties broken by the smallest grid coordinates;
   - area ids go by size (largest first), then peak count.
   - Neither tie rule was needed on this data.
4. **Areas per subset are 3 / 5 / 4.** Small areas near the threshold appear and disappear with N. The three main areas (downtown, UT/north, south-west) are in every subset, so `areas_found` / `reference_areas` will vary slightly by subset.
5. **Ground-truth file names.** These follow B4, plus `h3_delta4` for the δ = 4 variant and `smoke_<csv>_<variant>_tau<τ>.json` for smoke tests. The files contain the decoded cells too (label, centre, grid square), so prompt 2's self-check and prompt 3's accuracy can use them directly.
6. **Converted inputs in git:** `bench/data/encodings/` (5.0 MB) is listed in the `git add` below, as B1 places them in `bench/data/`. The RideAustin terms apply to them as to the subsets. They can be regenerated exactly with `convert_inputs.py` if you'd rather not commit them.
7. **Line endings.** `core.autocrlf=true`, so both specs are CRLF on disk and LF in git. Part A is identical either way.
   - Generated JSON is written with LF, and Git will warn when converting it.
   - `points_sha256` in the encodings manifest is line-ending-independent.
8. **`evaluate.py` / `test_evaluate.py` are untouched (decision 8)** and currently don't run here: they import `run_bench` and expect the old reference keys. You will re-copy them from Mastic.
9. **For prompt 2** (already decided, not done here):
   - make the 1 GiB channel buffer configurable through an environment variable (default unchanged);
   - leave the client's sketch-data generation out of the upload time;
   - start the dealer first and wait for its key pre-generation;
   - log dealer bytes in `extra`;
   - server_to_server = [sent by server0, sent by server1];
   - 5 threads per server;
   - there are no per-level times (the crawl is LIFO), only per-step times and per-level counts.
   - Online includes sketch verification, so it isn't directly comparable with the README's 32.4 s, which likely excludes the 6.86 s of sketching.
10. **B2.8:** the Mosaic paper PDF (in your OneDrive) couldn't be rendered here, so the figures come from the authors' README. Paper section citations for leakage are due in the `LEAKAGE_mosaic.md` prompt.

## What to commit

Most of this prompt's work is already in your commit `479c75b` (14:45). It was made while `ground_truth.py --check-rust` was still on `day_3of3`. The 67 committed data files are the final outputs: identical to the working tree, without timings, and every one parses. What came later is still uncommitted:
- the last three ground-truth files;
- the full-day reference;
- this report;
- a one-line fix in `ground_truth.py` (the README line number it prints, 671 → 673).

```
git add bench/scripts/ground_truth.py bench/ground_truth/day_3of3_h3_tau0.005.json bench/ground_truth/day_3of3_h3_delta4_tau0.005.json bench/ground_truth/day_3of3_morton_tau0.005.json bench/reference/day_3of3.json bench/reports/01-harness.md
```

Commit message: `Complete the Mosaic harness outputs: full-day reference and last tau 0.005 ground truth; add report 01`

(For reference, the whole prompt's file list was: `.gitignore`, `bench/BENCHMARK_MOSAIC.md`, `bench/README.md`, `bench/requirements.txt`, `bench/scripts/{bench_common,verify_subsets,convert_inputs,ground_truth}.py`, `bench/data/encodings/`, `bench/ground_truth/`, `bench/reference/`, `bench/reports/01-harness.md`.)
