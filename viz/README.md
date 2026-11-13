# Crawl viewer

Four synced maps, one per encoding (Mosaic grid, Morton, H3, geohash), with a slider that steps
through the levels of the unknown-dictionary crawl. Each level adds one bit of one coordinate
(the coordinates take turns), so a prefix stands for a box of cells. At each level every
candidate prefix is drawn as its region: filled if at least the threshold of clients are within
`delta` of the region, so the crawl keeps and splits it, and dashed if it was pruned. This is
what the servers learn during the protocol. Unlike Mastic, a count is the number of clients
*near* a region, not inside it, so neighbouring regions share clients.

The levels differ per encoding: `d * h1` is 22 for the grid and geohash and 20 for Morton. H3
has three coordinates, but its config names the third (`i - j`) as a `derived_coordinate`: the
crawl only takes bits of `i` and `j` (22 levels) and a 23rd level checks every kept `(i, j)` at
its one cell. A pane that has finished shows its final level. Runs saved before this change
crawled all 33 levels of H3 and also kept triples whose third coordinate is not `i - j`; those
are counted in the pane's note but not drawn.

## Save runs

```sh
viz/run_crawls.sh busiest_day              # the four encodings, Linf, L1 and L2
viz/run_crawls.sh busiest_week linf        # only some metrics
```

This needs `data/sample_<dataset>.csv` (from Zenodo) and the configs in `configs/`. Missing
encoded datasets are generated first. For every encoding and metric it runs `crawl_exporter` on
the FSS config of that dataset and writes `viz/results/<dataset>-<metric>-<encoding>.json`, then
runs `viz/build_viz.py`, which rebuilds `viz/results/manifest.json` and
`viz/dist/mosaic-crawl.html`. The crawl is computed in plaintext, as in
`mosaic_others ground-truth`, so no servers are needed. To save a single config, for example a
Morton box of your own:

```sh
cargo run --release --bin crawl_exporter -- \
  --config configs/encodings/morton/busiest_day/linf_fss_config.json \
  --output viz/results/busiest_day-linf-morton-mybox.json
python3 viz/build_viz.py
```

Each pane's menu can show any saved run of the selected dataset and metric.

## View

- While developing: `python3 -m http.server --directory viz 8080`, then open
  http://localhost:8080. The page reads `results/` on every load, so a new run shows up on
  refresh.
- To share: send `viz/dist/mosaic-crawl.html`. It has every run inlined and opens straight from
  disk (about 13 MB for the busiest day).

Arrow keys step through the levels, Home and End jump to the first and last, space plays. The
map tiles need an internet connection.
