#!/usr/bin/env bash
set -euo pipefail

ROLE="${1:-}"
if [[ -z "$ROLE" ]]; then
  echo "Usage: $0 {server0|server1|client|dealer}" >&2
  exit 1
fi

mkdir -p results

# Optional: ENCODING=<name> runs the configs in configs/encodings/<name> instead of
# configs/unknown_dictionary, and prefixes the log names with the encoding. Unset, or
# ENCODING=mosaic_grid, runs the original grid configs as before.
ENCODING="${ENCODING:-}"
if [[ -n "$ENCODING" && "$ENCODING" != "mosaic_grid" ]]; then
  CONFIG_ROOT="configs/encodings/${ENCODING}"
  TAG="${ENCODING}_"
  if [[ ! -d "$CONFIG_ROOT" ]]; then
    echo "No configs for encoding '$ENCODING' in $CONFIG_ROOT" >&2
    exit 1
  fi
else
  CONFIG_ROOT="configs/unknown_dictionary"
  TAG=""
fi

BUSIESTS=(day)
SHARES=(fss okvs)
METHODS=(gc_gc gc_fss fss_gc fss_fss)
METRICS=(linf l1 l2)

case "$ROLE" in
  server0|server1|client|dealer)
    ;;
  *)
    echo "Invalid role: $ROLE" >&2
    echo "Usage: $0 {server0|server1|client|dealer}" >&2
    exit 1
    ;;
esac

for BUSIEST in "${BUSIESTS[@]}"; do
  for SHARE in "${SHARES[@]}"; do
    for METHOD in "${METHODS[@]}"; do
      for METRIC in "${METRICS[@]}"; do
        echo "Running ROLE=${ROLE}, ENCODING=${ENCODING:-original}, BUSIEST=${BUSIEST}, SHARE=${SHARE}, METHOD=${METHOD}, METRIC=${METRIC}"

        case "$ROLE" in
          server0)
            cargo run --release --bin mosaic_server -- \
              --side 0 \
              --config "${CONFIG_ROOT}/busiest_${BUSIEST}/${METRIC}_${SHARE}_config.json" \
              --network-config configs/network_config.json \
              --method-config "configs/method/${METHOD}.json" \
              --threads 32 > "results/${TAG}server0_busiest_${BUSIEST}_${METRIC}_${SHARE}_${METHOD}.log" 2>&1
            ;;
          server1)
            cargo run --release --bin mosaic_server -- \
              --side 1 \
              --config "${CONFIG_ROOT}/busiest_${BUSIEST}/${METRIC}_${SHARE}_config.json" \
              --network-config configs/network_config.json \
              --method-config "configs/method/${METHOD}.json" \
              --threads 32 > "results/${TAG}server1_busiest_${BUSIEST}_${METRIC}_${SHARE}_${METHOD}.log" 2>&1
            ;;
          client)
            cargo run --release --bin mosaic_client -- \
              --config "${CONFIG_ROOT}/busiest_${BUSIEST}/${METRIC}_${SHARE}_config.json" \
              --network-config configs/network_config.json > "results/${TAG}client_busiest_${BUSIEST}_${METRIC}_${SHARE}_${METHOD}.log" 2>&1
            ;;
          dealer)
            cargo run --release --bin mosaic_dealer -- \
              --config "${CONFIG_ROOT}/busiest_${BUSIEST}/${METRIC}_${SHARE}_config.json" \
              --network-config configs/network_config.json \
              --threads 32 > "results/${TAG}dealer_busiest_${BUSIEST}_${METRIC}_${SHARE}_${METHOD}.log" 2>&1
            ;;
        esac
      done
    done
  done
done

BUSIESTS=(week month)
SHARES=(fss okvs)
METHODS=(gc_gc)
METRICS=(linf l1 l2)

case "$ROLE" in
  server0|server1|client|dealer)
    ;;
  *)
    echo "Invalid role: $ROLE" >&2
    echo "Usage: $0 {server0|server1|client|dealer}" >&2
    exit 1
    ;;
esac

for BUSIEST in "${BUSIESTS[@]}"; do
  for SHARE in "${SHARES[@]}"; do
    for METHOD in "${METHODS[@]}"; do
      for METRIC in "${METRICS[@]}"; do
        echo "Running ROLE=${ROLE}, ENCODING=${ENCODING:-original}, BUSIEST=${BUSIEST}, SHARE=${SHARE}, METHOD=${METHOD}, METRIC=${METRIC}"

        case "$ROLE" in
          server0)
            cargo run --release --bin mosaic_server -- \
              --side 0 \
              --config "${CONFIG_ROOT}/busiest_${BUSIEST}/${METRIC}_${SHARE}_config.json" \
              --network-config configs/network_config.json \
              --method-config "configs/method/${METHOD}.json" \
              --threads 32 > "results/${TAG}server0_busiest_${BUSIEST}_${METRIC}_${SHARE}_${METHOD}.log" 2>&1
            ;;
          server1)
            cargo run --release --bin mosaic_server -- \
              --side 1 \
              --config "${CONFIG_ROOT}/busiest_${BUSIEST}/${METRIC}_${SHARE}_config.json" \
              --network-config configs/network_config.json \
              --method-config "configs/method/${METHOD}.json" \
              --threads 32 > "results/${TAG}server1_busiest_${BUSIEST}_${METRIC}_${SHARE}_${METHOD}.log" 2>&1
            ;;
          client)
            cargo run --release --bin mosaic_client -- \
              --config "${CONFIG_ROOT}/busiest_${BUSIEST}/${METRIC}_${SHARE}_config.json" \
              --network-config configs/network_config.json > "results/${TAG}client_busiest_${BUSIEST}_${METRIC}_${SHARE}_${METHOD}.log" 2>&1
            ;;
          dealer)
            cargo run --release --bin mosaic_dealer -- \
              --config "${CONFIG_ROOT}/busiest_${BUSIEST}/${METRIC}_${SHARE}_config.json" \
              --network-config configs/network_config.json \
              --threads 32 > "results/${TAG}dealer_busiest_${BUSIEST}_${METRIC}_${SHARE}_${METHOD}.log" 2>&1
            ;;
        esac
      done
    done
  done
done
