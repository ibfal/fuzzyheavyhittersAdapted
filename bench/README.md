# Mosaic benchmark harness

`BENCHMARK_MOSAIC.md` is the source of truth. This file holds the exact commands. Run everything from the repository root in PowerShell.

## Setup (once)

```powershell
python -m venv .venv
.\.venv\Scripts\python.exe -m pip install -r bench\requirements.txt
cargo build --release --bin ride_austin_json_generator --bin heavy_hitters_decoder --bin mosaic_others
```

- The release build takes about 2 minutes.
- The scripts use three binaries from `target\release\`:
  - the CSV→JSON converter (`ride_austin_json_generator`);
  - the point decoder (`heavy_hitters_decoder`);
  - Mosaic's plaintext (`mosaic_others`), only for `--check-rust`.

Mosaic's day files are not in this repository (`data/` is gitignored). `convert_inputs.py` reads them from the Mastic repository's `data\mosaic\` folder:
- `sample_busiest_day.csv`;
- `start_busiest_day.json`.

## Commands

```powershell
# 1. The subsets copied from Mastic match bench\data\manifest.json (row counts, SHA-256, LF, nesting).
.\.venv\Scripts\python.exe bench\scripts\verify_subsets.py
.\.venv\Scripts\python.exe bench\scripts\verify_subsets.py --source C:\Users\arabe\IMP\mastic_for_fuzzy_HH\data\mosaic\sample_busiest_day.csv

# 2. Convert the subsets and the smoke-test CSV into Mosaic's JSON points, for all four encodings.
#    It also checks that the full day CSV reproduces start_busiest_day.json exactly.
.\.venv\Scripts\python.exe bench\scripts\convert_inputs.py --source-dir C:\Users\arabe\IMP\mastic_for_fuzzy_HH\data\mosaic

# 3. Ground truth (tau 0.02 and 0.005, every encoding, H3 at delta 5 and 4) and the reference.
.\.venv\Scripts\python.exe bench\scripts\ground_truth.py
.\.venv\Scripts\python.exe bench\scripts\ground_truth.py --check-rust   # also compare with mosaic_others

# 4. Smoke test: the ground truth for any CSV at any threshold.
.\.venv\Scripts\python.exe bench\scripts\ground_truth.py --csv bench\data\ten_locations.csv --threshold 0.1

# 5. Shared helpers: schema-check self-test, environment record, repo configs vs harness parameters.
.\.venv\Scripts\python.exe bench\scripts\bench_common.py --self-test
.\.venv\Scripts\python.exe bench\scripts\bench_common.py
```

Every script exits with code 1 and a `FAIL` message when a check fails.

## What is where

| Path | Contents |
|---|---|
| `data\day_*.csv`, `data\manifest.json`, `data\ten_locations.csv` | Copied from the Mastic harness. Never edited here. |
| `data\encodings\<encoding>\<name>.json` (+ `.encoding.json`) | The same rides as Mosaic's input points. Written by `convert_inputs.py`. |
| `data\encodings\manifest.json` | Generator arguments, kept/dropped/clamped counts, `points_sha256`. |
| `ground_truth\day_<subset>_<variant>_tau<tau>.json` | Mosaic's plaintext answer (B4). One file per variant × subset × τ, where the variant is `mosaic_grid`, `geohash`, `h3` (δ = 5), `h3_delta4` or `morton`. Each file has the points with neighbourhood counts, the decoded cells, and the prefixes checked and kept per level. |
| `ground_truth\smoke_<name>_<variant>_tau<tau>.json` | The same for a smoke-test CSV. |
| `reference\day_<subset>.json` | The reference hotspots (A5). It is copied to the Mastic codebase. |
| `reports\NN-<name>.md` | One report per prompt. |

## How the ground truth is computed

`ground_truth.py` replicates Mosaic's plaintext definition (B2.4), with file:line citations in the code. Two independent methods must agree exactly on points and counts:
1. **The crawl the servers run** (`protocol.rs:559-760`). It also gives the bits the servers see per level.
2. **The definition at every point of the domain** (dense neighbourhood sums).

`--check-rust` also compares every answer with Mosaic's own `mosaic_others ground-truth`. For H3 that runs both with and without the `derived_coordinate` crawl (commit 5623af8), and restricted to cells the two must agree.

The files are deterministic: running the scripts again with the same options reproduces them byte for byte. With `--check-rust`, the ground-truth files also record that check.
