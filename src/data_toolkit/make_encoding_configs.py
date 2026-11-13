#!/usr/bin/env python3
"""Write unknown-dictionary configs for an encoded RideAustin dataset.

For every dataset (busiest day / week / month), distance metric and share method, the config in
configs/unknown_dictionary is used as a template and the encoding-dependent fields are replaced:

- data_file, query_file: data/encodings/<encoding>/start_busiest_<dataset>.json
- d, h1:                 from the dataset's .encoding.json (written by ride_austin_json_generator)
- num_clients:           rides kept by the encoding
- match_threshold:       2% of num_clients, rounded down, like the original configs
- delta:                 per encoding and metric, see DELTAS
- h2:                    wide enough that the summed per-coordinate distance cannot wrap
- derived_coordinate:    h3 only: coordinate 2 is i - j, so the crawl skips its bits and checks
                         it once per full point (see DerivedCoordinate in src/configs/cli_config.rs)

Each coordinate's distance FSS outputs |x - y|^p, capped at delta^p + 1, and the d outputs are
added mod 2^h2. For L1 and L2 the sum must stay below 2^h2, so h2 = bits(d * (delta^p + 1)). This
gives the original h2 = 4 (L1) and 6 (L2) for d = 2, delta = 5. Linf only checks each coordinate,
so h2 stays 1. OKVS configs add the same number of extra bits to their template's h2.

Usage (from the repository root, after generating the datasets):
    python3 src/data_toolkit/make_encoding_configs.py morton
    python3 src/data_toolkit/make_encoding_configs.py morton --delta Linf=6
"""

import argparse
import json
import math
from pathlib import Path

DATASETS = ["day", "week", "month"]
METRICS = {"linf": "Linf", "l1": "L1", "l2": "L2"}
SHARES = ["fss", "okvs"]
MATCH_FRACTION = 0.02
P = {"Linf": None, "L1": 1, "L2": 2}

# delta in cells, chosen so the neighbourhood has about the same physical size as Mosaic's
# delta = 5 on its 0.001 degree grid (about 555 m north-south and 480 m east-west). The original
# grid is not listed: it keeps using the configs in configs/unknown_dictionary.
DELTAS = {
    # The default Morton box has exactly Mosaic's 0.001 degree cells.
    "morton": {"Linf": 5, "L1": 5, "L2": 5},
    # 76 m x 132 m cells: delta = 5 is about 380 m north-south and 660 m east-west.
    "geohash": {"Linf": 5, "L1": 5, "L2": 5},
    # Resolution 10 hexagons, centers about 140 m apart, as [i, j, i - j]. Linf is the number of
    # hexagon steps, L1 twice that (so 8 here is the same neighbourhood as Linf 4), and L2 is
    # sqrt(2) times the center distance in spacings (5 is a radius of 3.5 spacings, about 500 m).
    "h3": {"Linf": 4, "L1": 8, "L2": 5},
}


def needed_h2(metric, d, delta):
    """Bits for the sum of d capped distances, or None for Linf."""
    p = P[metric]
    if p is None:
        return None
    return math.ceil(math.log2(d * (delta**p + 1) + 1))


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("encoding", choices=sorted(DELTAS))
    parser.add_argument(
        "--metadata-dir",
        help="where the .encoding.json files are (default: data/encodings/<encoding>)",
    )
    parser.add_argument(
        "--delta",
        action="append",
        default=[],
        metavar="METRIC=DELTA",
        help="override delta for one metric, e.g. Linf=6",
    )
    parser.add_argument("--out-dir", default="configs/encodings")
    args = parser.parse_args()

    deltas = dict(DELTAS[args.encoding])
    for override in args.delta:
        metric, value = override.split("=")
        if metric not in P:
            parser.error(f"unknown metric {metric!r}")
        deltas[metric] = int(value)

    data_dir = f"data/encodings/{args.encoding}"
    metadata_dir = Path(args.metadata_dir or data_dir)
    template_dir = Path("configs/unknown_dictionary")

    for dataset in DATASETS:
        metadata = json.loads((metadata_dir / f"start_busiest_{dataset}.encoding.json").read_text())
        d, h1, n = metadata["d"], metadata["h1"], metadata["kept_rides"]
        print(
            f"{args.encoding} busiest_{dataset}: d = {d}, h1 = {h1}, {n} clients "
            f"({metadata['dropped_rides']} dropped, {metadata['clamped_rides']} clamped)"
        )
        for metric_file, metric in METRICS.items():
            delta = deltas[metric]
            fss_template = json.loads(
                (template_dir / f"busiest_{dataset}" / f"{metric_file}_fss_config.json").read_text()
            )["protocol"]
            for share in SHARES:
                template_path = template_dir / f"busiest_{dataset}" / f"{metric_file}_{share}_config.json"
                config = json.loads(template_path.read_text())
                protocol = config["protocol"]
                assert protocol["distance_metric"] == metric, template_path

                needed = needed_h2(metric, d, delta)
                if needed is not None:
                    extra = max(0, needed - fss_template["h2"])
                    protocol["h2"] = needed if share == "fss" else protocol["h2"] + extra
                assert 2 ** protocol["h3"] > n, f"h3 = {protocol['h3']} cannot count {n} clients"

                config["data_file"] = f"{data_dir}/start_busiest_{dataset}.json"
                config["query_file"] = config["data_file"]
                protocol.update(
                    delta=delta,
                    d=d,
                    h1=h1,
                    num_clients=n,
                    match_threshold=math.floor(MATCH_FRACTION * n),
                )
                if args.encoding == "h3":
                    # [i + o, j + o, i - j + o]: the third coordinate is the first minus the second.
                    protocol["derived_coordinate"] = {"index": 2, "plus": 0, "minus": 1, "offset": 2 ** (h1 - 1)}

                out = Path(args.out_dir) / args.encoding / f"busiest_{dataset}" / f"{metric_file}_{share}_config.json"
                out.parent.mkdir(parents=True, exist_ok=True)
                out.write_text(json.dumps(config, indent=2) + "\n")
    print(f"Wrote configs to {Path(args.out_dir) / args.encoding}")


if __name__ == "__main__":
    main()
