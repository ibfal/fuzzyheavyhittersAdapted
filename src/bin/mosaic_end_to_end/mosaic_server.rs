use clap::Parser;
use mosaic::{
    channel::{listen_to, setup_parallel_channels, CommTrackingChannel},
    configs::{cli_config::CliConfig, method_config::MethodConfig, network_config::NetworkConfig},
    fuzzy_match::protocol::MosaicProtocol,
    randomness::prg::PRG,
};
use std::fs;
use scuttlebutt::channel::AbstractChannel;
use serde_json::{json, Value};

/// Load query points from JSON file
fn load_query_points(file_path: &str) -> Result<Vec<Vec<u128>>, String> {
    let content = fs::read_to_string(file_path)
        .map_err(|e| format!("Failed to read query file {}: {}", file_path, e))?;

    serde_json::from_str(&content)
        .map_err(|e| format!("Failed to parse query file {}: {}", file_path, e))
}


struct NetworkServerSetup {
    client_channel: CommTrackingChannel,
    other_server_channels: Vec<CommTrackingChannel>,
    signal_dealer_channels: Vec<CommTrackingChannel>,
}

fn send_ready_signal(client_channel: &mut CommTrackingChannel, server_id: u8) -> Result<(), String> {
    client_channel
        .write_bytes(b"hi")
        .map_err(|e| format!("Server {}: failed to send ready signal: {}", server_id, e))?;
    client_channel
        .flush()
        .map_err(|e| format!("Server {}: failed to flush ready signal: {}", server_id, e))?;
    println!("Server {}: sent ready signal to client", server_id);
    Ok(())
}

fn setup_network_server(
    network_config_path: &str,
    is_server1: bool,
    num_threads: usize,
) -> Result<NetworkServerSetup, String> {
    let server_id = if is_server1 { 1 } else { 0 };
    let network_config = NetworkConfig::from_file(network_config_path)?;

    let server_addr = if is_server1 {
        network_config.server1_addr.clone()
    } else {
        network_config.server0_addr.clone()
    };

    let client_to_server_port = if is_server1 {
        network_config.client_to_server1_port
    } else {
        network_config.client_to_server0_port
    };

    let mut client_channel = listen_to(server_addr.clone(), client_to_server_port)
        .map_err(|e| format!("Failed to listen for client connection: {}", e))?;

    let server0_addr = &network_config.server0_addr;
    let server0_to_server1_port = network_config.server0_to_server1_port;

    println!(
        "Server {}: Setting up {} inter-server channels...",
        server_id, num_threads
    );

    let other_server_channels = if is_server1 {
        setup_parallel_channels(true, num_threads, server0_addr, server0_to_server1_port)
    } else {
        setup_parallel_channels(false, num_threads, server0_addr, server0_to_server1_port)
    }?;

    println!(
        "Server {}: Successfully established {} inter-server channels",
        server_id,
        other_server_channels.len()
    );

    let num_dealer_channels = num_threads;
    println!(
        "Server {}: Setting up {} dealer channels...",
        server_id, num_dealer_channels
    );

    let dealer_to_server_port = if is_server1 {
        network_config.dealer_to_server1_port
    } else {
        network_config.dealer_to_server0_port
    };

    let signal_dealer_channels = setup_parallel_channels(
        false,
        num_dealer_channels,
        &server_addr,
        dealer_to_server_port,
    )?;

    println!(
        "Server {}: Successfully established {} dealer channels",
        server_id,
        signal_dealer_channels.len(),
    );

    send_ready_signal(&mut client_channel, server_id)?;

    Ok(NetworkServerSetup {
        client_channel,
        other_server_channels,
        signal_dealer_channels,
    })
}
fn print_server_summary(
    server_id: u8,
    protocol_time: std::time::Duration,
    other_server_channels: &[CommTrackingChannel],
    signal_dealer_channels: &[CommTrackingChannel],
) {
    let mut total_other_server_bytes_sent = 0;
    let mut total_other_server_bytes_received = 0;
    let mut total_dealer_bytes_sent = 0;
    let mut total_dealer_bytes_received = 0;

    for channel in other_server_channels {
        let (sent, received) = channel.get_communication_stats();
        total_other_server_bytes_sent += sent;
        total_other_server_bytes_received += received;
    }

    for channel in signal_dealer_channels {
        let (sent, received) = channel.get_communication_stats();
        total_dealer_bytes_sent += sent;
        total_dealer_bytes_received += received;
    }

    println!("\n=== Server {} Performance Summary ===", server_id);
    println!("📊 Protocol execution time: {:.2?}", protocol_time);
    println!(
        "📡 Communication with other server ({} channels):",
        other_server_channels.len()
    );
    println!(
        "   Bytes sent: {} bytes ({:.2} KB)",
        total_other_server_bytes_sent,
        total_other_server_bytes_sent as f64 / 1024.0
    );
    println!(
        "   Bytes received: {} bytes ({:.2} KB)",
        total_other_server_bytes_received,
        total_other_server_bytes_received as f64 / 1024.0
    );
    println!(
        "📡 Communication with dealer ({} channels):",
        signal_dealer_channels.len()
    );
    println!(
        "   Bytes sent: {} bytes ({:.2} KB)",
        total_dealer_bytes_sent,
        total_dealer_bytes_sent as f64 / 1024.0
    );
    println!(
        "   Bytes received: {} bytes ({:.2} KB)",
        total_dealer_bytes_received,
        total_dealer_bytes_received as f64 / 1024.0
    );
    println!(
        "📡 Total communication: {} bytes ({:.2} KB)",
        total_other_server_bytes_sent
            + total_other_server_bytes_received
            + total_dealer_bytes_sent
            + total_dealer_bytes_received,
        (total_other_server_bytes_sent
            + total_other_server_bytes_received
            + total_dealer_bytes_sent
            + total_dealer_bytes_received) as f64
            / 1024.0
    );
}
/// Benchmark options (bench/BENCHMARK_MOSAIC.md, A9): `--bench-out`, and for server 0
/// `--bench-meta`.
struct BenchOptions {
    out: String,
    meta: Option<Value>,
}

/// Sum of (bytes sent, bytes received) over channels.
fn channel_bytes(channels: &[CommTrackingChannel]) -> (usize, usize) {
    channels.iter().fold((0, 0), |(s, r), c| {
        let (sent, received) = c.get_communication_stats();
        (s + sent, r + received)
    })
}

/// Run server for both known and unknown dictionary cases
fn run_server(config_path: &str, network_config_path: &str, method_config_path: &str, is_server1: bool, num_threads: usize, bench: Option<BenchOptions>) -> Result<(), String> {
    if num_threads == 0 {
        return Err("--threads must be greater than 0".to_string());
    }

    rayon::ThreadPoolBuilder::new()
        .num_threads(num_threads)
        .build_global()
        .map_err(|e| format!("Failed to configure Rayon thread pool: {}", e))?;

    let cli_config = CliConfig::from_file(config_path)?;
    let method_config = MethodConfig::from_file(method_config_path)?;
    let server_id = if is_server1 { 1 } else { 0 };
    let is_known_dictionary = cli_config.protocol.dictionary_type == "Known";

    println!(
        "Server {} starting {} dictionary protocol with {} parallel threads...",
        server_id,
        if is_known_dictionary {
            "known"
        } else {
            "unknown"
        },
        num_threads
    );

    if bench.is_some() && is_known_dictionary {
        return Err("--bench-out supports the unknown dictionary only".to_string());
    }
    if let Some(b) = &bench {
        // Parts left by an earlier run must not be mistaken for this run's.
        if !is_server1 {
            for party in ["server1", "client"] {
                let _ = fs::remove_file(mosaic::bench::part_path(&b.out, party));
            }
        }
    }

    let NetworkServerSetup {
        mut client_channel,
        mut other_server_channels,
        mut signal_dealer_channels,
    } = setup_network_server(network_config_path, is_server1, num_threads)?;

    // Create protocol configuration
    let protocol_parameters = cli_config.protocol.clone();
    let mut protocol = MosaicProtocol::new(
        protocol_parameters.clone(),
        method_config.clone(),
        is_server1,
        protocol_parameters.clone().enable_sketch,
        protocol_parameters.clone().num_clients,
        protocol_parameters.clone().match_threshold,
    );

    println!("Server {}: Receiving shares from client...", server_id);
    let mut shares = protocol
        .receive_client_shares(&mut client_channel)
        .map_err(|e| format!("Failed to receive client shares: {}", e))?;
    println!(
        "Server {}: Received {} shares from client",
        server_id,
        shares.len()
    );
    let t_shares_stored_ns = mosaic::bench::unix_ns();
    let sketch_data = if protocol_parameters.enable_sketch {
        println!("Server {}: Receiving sketch data from client...", server_id);
        let data = protocol
            .receive_client_sketch_data(&mut client_channel)
            .map_err(|e| format!("Failed to receive client sketch data: {}", e))?;
        println!(
            "Server {}: Received sketch data for {} shares",
            server_id,
            data.len()
        );
        Some(data)
    } else {
        None
    };
    // A3/B3: all reports are stored; the online phase starts here (once both servers are here).
    let t_reports_stored_ns = mosaic::bench::unix_ns();
    let s2s_before_online = channel_bytes(&other_server_channels);
    let client_bytes_received = client_channel.get_communication_stats().1;
    let mut client_validity_bits = 0usize;
    let mut clients_rejected = 0usize;

    if protocol_parameters.enable_sketch {
        println!("Server {}: Verifying client shares using sketches...", server_id);

        let bench_sketch = mosaic::bench::timer();
        let start_sketch = std::time::Instant::now();

        let sketch_data = sketch_data
            .as_ref()
            .ok_or_else(|| "Sketch data missing while sketching is enabled".to_string())?;
        let (malicious_flags, bad_count) = {
            let prg_seed = [0u8; 16]; // Change later, need to exchange seed between servers
            let mut prg = PRG::new(Some(&prg_seed), 0);
            let flags = protocol
                .verify_client_shared_ranges(
                    &shares,
                    sketch_data,
                    &mut prg,
                    &mut other_server_channels,
                )
                .map_err(|e| format!("Failed to verify client shares: {}", e))?;
            let bad_count = flags.iter().filter(|&&flag| flag).count();
            (flags, bad_count)
        };
        let total = malicious_flags.len();
        // A7: one accept/reject bit per client is opened to both servers.
        client_validity_bits = total;
        clients_rejected = bad_count;
        if bad_count > 0 {
            println!(
                "Server {}: Removing {} of {} shares flagged by sketch verification",
                server_id, bad_count, total
            );
        }
        shares = shares
            .into_iter()
            .zip(malicious_flags.iter())
            .filter_map(|(share, flag)| if *flag { None } else { Some(share) })
            .collect();
        if bad_count > 0 {
            protocol = MosaicProtocol::new(
                protocol_parameters.clone(),
                method_config.clone(),
                is_server1,
                protocol_parameters.enable_sketch,
                shares.len(),
                protocol_parameters.match_threshold,
            );
        }

        println!(
            "Server {}: Sketch verification completed in {:.2?}",
            server_id,
            start_sketch.elapsed()
        );
        bench_sketch.stop("sketch_verify");
    }
    let s2s_after_sketch = channel_bytes(&other_server_channels);

    println!("Server {}: Running protocol...", server_id);

    let start_time = std::time::Instant::now();
    let mut heavy_hitter_points: Vec<Vec<u128>> = Vec::new();

    let protocol_time = if is_known_dictionary {
        // Load query points for known dictionary
        println!(
            "Server {}: Loading query points from {}",
            server_id, cli_config.query_file
        );
        let query_points = load_query_points(&cli_config.query_file)?;

        // Run the protocol for known dictionary
        println!(
            "Server {}: Using {} dealer channels and {} server channels for parallel processing",
            server_id,
            signal_dealer_channels.len(),
            other_server_channels.len()
        );

        let results = protocol.run_server_known_dictionary_parallel(
            &shares,
            &query_points,
            &mut signal_dealer_channels,
            &mut other_server_channels,
        )?;
        println!("Server {}: Protocol execution completed", server_id);
        println!(
            "Found {} heavy hitters.",
            results.len(),
        );

        start_time.elapsed()
    } else {
        println!(
            "Server {}: Running unknown dictionary protocol...",
            server_id
        );

        // Run the protocol for unknown dictionary
        let heavy_hitters = protocol.run_server_unknown_dictionary_parallel2(
            &shares,
            &mut signal_dealer_channels,
            &mut other_server_channels,
        )?;
        println!("Server {}: Protocol execution completed", server_id);
        println!(
            "Found {} heavy hitters.",
            heavy_hitters.len(),
        );
        heavy_hitter_points = heavy_hitters;

        start_time.elapsed()
    };
    let t_online_end_ns = mosaic::bench::unix_ns();

    print_server_summary(
        server_id,
        protocol_time,
        &other_server_channels,
        &signal_dealer_channels,
    );

    if let Some(b) = bench {
        let s2s_end = channel_bytes(&other_server_channels);
        let dealer = channel_bytes(&signal_dealer_channels);
        let part = json!({
            "party": format!("server{}", server_id),
            "t_shares_stored_ns": t_shares_stored_ns as u64,
            "t_reports_stored_ns": t_reports_stored_ns as u64,
            "t_online_end_ns": t_online_end_ns as u64,
            "client_bytes_received": client_bytes_received,
            // Application-level bytes on the inter-server channels (A4), per step of the online
            // phase. Nothing is sent on them before the online phase.
            "server_bytes_sent": {
                "before_online": s2s_before_online.0,
                "sketch_verify": s2s_after_sketch.0 - s2s_before_online.0,
                "crawl": s2s_end.0 - s2s_after_sketch.0,
                "online": s2s_end.0 - s2s_before_online.0,
            },
            "server_bytes_received_online": s2s_end.1 - s2s_before_online.1,
            "dealer_bytes": {"sent": dealer.0, "received": dealer.1},
            "client_validity_bits": client_validity_bits,
            "clients_rejected": clients_rejected,
            "threads": num_threads,
            "channel_buffer_bytes": mosaic::channel::channel_buffer_bytes().map_err(|e| e.to_string())?,
            "online_ms_local": mosaic::bench::ms_between(t_reports_stored_ns as u64, t_online_end_ns as u64),
            "recorded": mosaic::bench::snapshot(),
            "output_points": heavy_hitter_points.iter()
                .map(|p| p.iter().map(|&v| v as u64).collect::<Vec<u64>>())
                .collect::<Vec<_>>(),
        });
        if is_server1 {
            mosaic::bench::write_json_atomic(&mosaic::bench::part_path(&b.out, "server1"), &part)?;
        } else {
            let meta = b.meta.ok_or("server 0 needs --bench-meta with --bench-out")?;
            let line = bench_line::build(&cli_config, &meta, &part, &b.out)?;
            mosaic::bench::append_jsonl(&b.out, &line)?;
            println!("Server 0: appended the benchmark result line to {}", b.out);
        }
    }

    Ok(())
}

/// Server 0's A9 result line (bench/BENCHMARK_MOSAIC.md A9, B3) from the four parties' records.
mod bench_line {
    use mosaic::{
        bench::{ms_between, part_path, round_to, wait_for_json},
        configs::cli_config::{CliConfig, DerivedCoordinate},
        data_toolkit::encoding::{metadata_path, read_metadata, split_locations},
    };
    use serde_json::{json, Map, Value};
    use std::time::Duration;

    /// How long server 0 waits for the other parties' records after its own run.
    const PART_TIMEOUT: Duration = Duration::from_secs(600);

    fn u64_at(v: &Value, path: &[&str]) -> Result<u64, String> {
        let mut cur = v;
        for key in path {
            cur = &cur[*key];
        }
        cur.as_u64()
            .ok_or_else(|| format!("bench: {} missing in the record of {}", path.join("."), v["party"]))
    }

    fn rounds(part: &Value, section: &str) -> u64 {
        part["recorded"]["rounds"][section]["critical"].as_u64().unwrap_or(0)
    }

    /// The reported points as `output` entries: the cell's label and centre, or, for points that
    /// are not cells (only the H3 crawl without the derived coordinate finds those), a label
    /// with the coordinates and null lat/lon. Counts are never revealed: null. Also returns the
    /// number of non-cells.
    fn decode_output(points: &[Vec<u128>], metadata_file: &str) -> Result<(Vec<Value>, u64), String> {
        let metadata = read_metadata(metadata_file)
            .map_err(|e| format!("bench: cannot read {}: {}", metadata_file, e))?;
        let encoder = metadata.encoding.encoder();
        let mut sorted = points.to_vec();
        sorted.sort();
        let mut out = Vec::with_capacity(sorted.len());
        let mut non_cells = 0;
        for point in &sorted {
            let decoded: Option<Vec<(String, f64, f64)>> = split_locations(&metadata, point)
                .map(|c| encoder.decode(c).map(|(lat, lon)| (encoder.label(c), lat, lon)))
                .collect();
            match decoded {
                Some(cells) if cells.len() == 1 => {
                    let (label, lat, lon) = &cells[0];
                    // Six decimals, as heavy_hitters_decoder writes them (and the ground truth).
                    let lat: f64 = format!("{:.6}", lat).parse().unwrap_or(*lat);
                    let lon: f64 = format!("{:.6}", lon).parse().unwrap_or(*lon);
                    out.push(json!({"label": label, "lat": lat, "lon": lon, "count": null}));
                }
                Some(cells) => {
                    // Pickup and drop-off points: not used by the benchmark.
                    let label = cells.iter().map(|c| c.0.clone()).collect::<Vec<_>>().join(" -> ");
                    out.push(json!({"label": label, "lat": null, "lon": null, "count": null}));
                }
                None => {
                    non_cells += 1;
                    let coords = point.iter().map(|v| v.to_string()).collect::<Vec<_>>().join(", ");
                    out.push(json!({"label": format!("noncell ({})", coords), "lat": null,
                                    "lon": null, "count": null}));
                }
            }
        }
        Ok((out, non_cells))
    }

    /// `meta` (from the runner) gives the header: dataset, subset, rep, warmup, timestamp, env,
    /// encoding, params.threshold_frac, params.encoding_params, and optionally extra and
    /// encoding_metadata. Everything else comes from this run's config and the parties' records.
    pub fn build(cli: &CliConfig, meta: &Value, s0: &Value, out: &str) -> Result<Value, String> {
        let s1 = wait_for_json(&part_path(out, "server1"), PART_TIMEOUT)?;
        let client = wait_for_json(&part_path(out, "client"), PART_TIMEOUT)?;
        let p = &cli.protocol;

        // Header: the run's own config wins; the meta must agree where it repeats a value.
        let derived = p.derived_coordinate;
        let data_bits = DerivedCoordinate::crawl_dims(derived.as_ref(), p.d).len() * p.h1
            + usize::from(derived.is_some());
        let mut params = meta["params"].as_object().cloned().ok_or("bench meta: params missing")?;
        if params.get("threshold_frac").and_then(Value::as_f64).is_none() {
            return Err("bench meta: params.threshold_frac missing".to_string());
        }
        if !params.get("encoding_params").map_or(false, Value::is_object) {
            return Err("bench meta: params.encoding_params missing".to_string());
        }
        for (key, value) in [
            ("data_bits", json!(data_bits)),
            ("threshold_abs", json!(p.match_threshold as u64)),
            ("delta", json!(p.delta as u64)),
        ] {
            if let Some(given) = params.get(key) {
                if !given.is_null() && *given != value {
                    return Err(format!("bench meta: params.{} = {} but the run uses {}", key, given, value));
                }
            }
            params.insert(key.to_string(), value);
        }
        if let Some(n) = meta.get("n_clients") {
            if n.as_u64() != Some(p.num_clients as u64) {
                return Err(format!("bench meta: n_clients = {} but the run has {}", n, p.num_clients));
            }
        }

        // A3/B3 timings, across processes on one machine (wall-clock nanoseconds).
        let stored = u64_at(s0, &["t_reports_stored_ns"])?.max(u64_at(&s1, &["t_reports_stored_ns"])?);
        let end = u64_at(s0, &["t_online_end_ns"])?.max(u64_at(&s1, &["t_online_end_ns"])?);
        let first_byte = u64_at(&client, &["t_first_byte_ns"])?;
        let sketch_gen_ms = u64_at(&client, &["sketch_gen_ns"])? as f64 / 1e6;
        let upload_ms = ms_between(first_byte, stored) - sketch_gen_ms;
        let online_ms = ms_between(stored, end);
        let steps: Vec<Value> = s0["recorded"]["steps"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .map(|s| json!({"name": s["name"], "ms": round_to(s["ms"].as_f64().unwrap_or(0.0), 3)}))
            .collect();

        // A4 rounds. Both servers count the same turns; take the larger if they ever differ.
        let section = |name: &str| rounds(s0, name).max(rounds(&s1, name));
        let (r_sketch, r_crawl, r_derived) =
            (section("sketch_verify"), section("crawl"), section("derived_check"));

        // A4 bytes.
        let sent = |part: &Value, key: &str| u64_at(part, &["server_bytes_sent", key]);
        let client_sent = client["bytes_sent"].as_array().ok_or("bench: client bytes_sent missing")?;
        let dealer_bytes: u64 = [s0, &s1]
            .iter()
            .map(|p| {
                p["dealer_bytes"]["sent"].as_u64().unwrap_or(0)
                    + p["dealer_bytes"]["received"].as_u64().unwrap_or(0)
            })
            .sum();

        // A7 leakage, from server 0's record; server 1 must have seen the same.
        let rec = &s0["recorded"];
        let candidates = rec["candidates_per_level"].clone();
        let frontier = rec["frontier_per_level"].clone();
        if s1["recorded"]["candidates_per_level"] != candidates
            || s1["recorded"]["frontier_per_level"] != frontier
        {
            return Err("bench: the two servers recorded different prefixes per level".to_string());
        }
        let total: u64 = candidates.as_array().map_or(0, |a| a.iter().filter_map(Value::as_u64).sum());

        // Output, decoded to cells. Both servers must hold the same points.
        if s1["output_points"] != s0["output_points"] {
            return Err("bench: the two servers output different points".to_string());
        }
        let points: Vec<Vec<u128>> = serde_json::from_value::<Vec<Vec<u64>>>(s0["output_points"].clone())
            .map_err(|e| format!("bench: output points: {}", e))?
            .into_iter()
            .map(|p| p.into_iter().map(u128::from).collect())
            .collect();
        let metadata_file = match meta.get("encoding_metadata").and_then(Value::as_str) {
            Some(path) => path.to_string(),
            None => metadata_path(&cli.data_file).to_string_lossy().into_owned(),
        };
        let (output, non_cells) = decode_output(&points, &metadata_file)?;

        let mut extra: Map<String, Value> =
            meta.get("extra").and_then(Value::as_object).cloned().unwrap_or_default();
        extra.insert("channel_buffer_bytes".into(), s0["channel_buffer_bytes"].clone());
        extra.insert("dealer_bytes".into(), json!(dealer_bytes));
        extra.insert("rounds_sketch_verify".into(), json!(r_sketch));
        extra.insert("rounds_crawl".into(), json!(r_crawl + r_derived));
        if derived.is_some() {
            extra.insert("rounds_derived_check".into(), json!(r_derived));
        }
        extra.insert("crawl_iterations".into(), rec["counters"]["crawl_iterations"].clone());
        extra.insert(
            "bytes_sketch_verify".into(),
            json!([sent(s0, "sketch_verify")?, sent(&s1, "sketch_verify")?]),
        );
        extra.insert("client_sketch_gen_ms".into(), json!(round_to(sketch_gen_ms, 3)));
        extra.insert("clients_rejected".into(), s0["clients_rejected"].clone());
        if meta["encoding"] == "h3" {
            extra.insert("h3_noncell_outputs".into(), json!(non_cells));
        } else if non_cells > 0 {
            return Err(format!("bench: {} outputs are not cells", non_cells));
        }

        Ok(json!({
            "schema_version": 1,
            "system": "mosaic",
            "encoding": meta["encoding"],
            "dataset": meta["dataset"],
            "subset": meta["subset"],
            "n_clients": p.num_clients,
            "rep": meta["rep"],
            "warmup": meta["warmup"],
            "timestamp": meta["timestamp"],
            "params": params,
            "env": meta["env"],
            "phases": {
                // B3: nothing is precomputed; the OT/GC set-up runs inline for every candidate.
                "server_offline_ms": 0,
                "upload_ms": round_to(upload_ms, 3),
                "online_ms": round_to(online_ms, 3),
                "online_steps": steps,
            },
            "rounds": {"upload": client["rounds_upload"], "online": r_sketch + r_crawl + r_derived, "extra": 0},
            "bytes": {
                "upload": {"client_to_server": [client_sent[0], client_sent[1]]},
                "online": {
                    // No collector: both servers end with the output (B3).
                    "collector_to_server": [0, 0],
                    "server_to_collector": [0, 0],
                    "server_to_server": [sent(s0, "online")?, sent(&s1, "online")?],
                },
            },
            "leakage": {
                "revealed_aggregates_total": total,
                "revealed_aggregates_per_level": candidates,
                "revealed_kind": "bit",
                "client_validity_bits": s0["client_validity_bits"],
                "frontier_per_level": frontier,
            },
            "output": output,
            "extra": extra,
        }))
    }
}

#[derive(Parser, Debug)]
#[command(version, about, long_about = None)]
struct Args {
    #[arg(short, long)]
    side: u8,
    #[arg(short, long)]
    config: String,
    #[arg(short, long)]
    network_config: String,
    #[arg(short, long)]
    method_config: String,
    #[arg(short, long, default_value_t = 1)]
    threads: usize,
    /// Benchmark (bench/BENCHMARK_MOSAIC.md): server 0 appends the run's A9 result line to this
    /// JSONL file; server 1 writes its measurements to `<file>.server1.part.json`.
    #[arg(long)]
    bench_out: Option<String>,
    /// Benchmark, server 0: the line's header fields as JSON, given as `@file`, a file path, or
    /// inline JSON (PowerShell strips the quotes of inline JSON: use a file there).
    #[arg(long)]
    bench_meta: Option<String>,
}

fn main() {
    let args = Args::parse();
    let side = args.side;
    let config_path = args.config;
    let network_config_path = args.network_config;
    let method_config_path = args.method_config;
    let num_threads = args.threads;
    let bench = match (args.bench_out, args.bench_meta) {
        (Some(out), meta) => match meta.as_deref().map(mosaic::bench::read_meta).transpose() {
            Ok(meta) => {
                mosaic::bench::enable();
                Some(BenchOptions { out, meta })
            }
            Err(e) => {
                eprintln!("Error running server: {}", e);
                return;
            }
        },
        (None, Some(_)) => {
            eprintln!("Error running server: --bench-meta needs --bench-out");
            return;
        }
        (None, None) => None,
    };

    let result = if side == 0 {
        run_server(&config_path, &network_config_path, &method_config_path, false, num_threads, bench)
    } else if side == 1 {
        run_server(&config_path, &network_config_path, &method_config_path, true, num_threads, bench)
    } else {
        Err("Side must be 0 or 1".to_string())
    };

    if let Err(e) = result {
        eprintln!("Error running server: {}", e);
    }
}
