#!/usr/bin/env bash
# Save the crawl of every encoding for one dataset, for viz/index.html.
#
#   viz/run_crawls.sh busiest_day                    # all four encodings, Linf, L1 and L2
#   viz/run_crawls.sh busiest_week linf              # only some metrics
#
# For each encoding and metric this runs crawl_exporter on the FSS config of that dataset
# (configs/unknown_dictionary for the original grid, configs/encodings/<encoding> otherwise) and
# writes viz/results/<dataset>-<metric>-<encoding>.json. Missing datasets are generated from
# data/sample_<dataset>.csv first. Then it rebuilds viz/results/manifest.json and the single-file
# viz/dist/mosaic-crawl.html. The crawl is the plaintext one (as in mosaic_others ground-truth),
# so no servers are needed.
set -euo pipefail

cd "$(dirname "$0")/.."

if [[ $# -lt 1 ]]; then
    sed -n '2,12p' "$0" | sed 's/^# \{0,1\}//'
    exit 1
fi
dataset=$1
shift
metrics=("$@")
if [[ ${#metrics[@]} -eq 0 ]]; then
    metrics=(linf l1 l2)
fi
encodings=(mosaic_grid morton h3 geohash)

cargo build --release --bin ride_austin_json_generator --bin crawl_exporter
bin=target/release

for encoding in "${encodings[@]}"; do
    if [[ $encoding == mosaic_grid ]]; then
        data="data/start_${dataset}.json"
    else
        data="data/encodings/${encoding}/start_${dataset}.json"
    fi
    if [[ ! -f $data || ! -f ${data%.json}.encoding.json ]]; then
        echo "Generating $data"
        mkdir -p "$(dirname "$data")"
        "$bin/ride_austin_json_generator" --input "data/sample_${dataset}.csv" --output "$data" \
            --output-type 0 --encoding "${encoding//_/-}"
    fi
done

mkdir -p viz/results
for metric in "${metrics[@]}"; do
    for encoding in "${encodings[@]}"; do
        if [[ $encoding == mosaic_grid ]]; then
            config="configs/unknown_dictionary/${dataset}/${metric}_fss_config.json"
        else
            config="configs/encodings/${encoding}/${dataset}/${metric}_fss_config.json"
        fi
        "$bin/crawl_exporter" --config "$config" \
            --output "viz/results/${dataset}-${metric}-${encoding}.json"
    done
done

python3 viz/build_viz.py
