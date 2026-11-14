use clap::Parser;
use mosaic::{channel::{connect_to, CommTrackingChannel}, configs::{cli_config::CliConfig, network_config::NetworkConfig}, fuzzy_match::client::Client};
use std::fs;
use scuttlebutt::channel::AbstractChannel;

/// Load client points directly from JSON file
fn load_client_points(file_path: &str) -> Result<Vec<Vec<u128>>, String> {
    let content = fs::read_to_string(file_path)
        .map_err(|e| format!("Failed to read client points file {}: {}", file_path, e))?;

    serde_json::from_str(&content)
        .map_err(|e| format!("Failed to parse client points file {}: {}", file_path, e))
}


fn wait_for_ready_signals(
    channel_server0: &mut CommTrackingChannel,
    channel_server1: &mut CommTrackingChannel,
) -> Result<(), String> {
    let mut ready0 = [0u8; 2];
    let mut ready1 = [0u8; 2];
    channel_server0
        .read_bytes(&mut ready0)
        .map_err(|e| format!("Failed to read ready signal from server 0: {}", e))?;
    channel_server1
        .read_bytes(&mut ready1)
        .map_err(|e| format!("Failed to read ready signal from server 1: {}", e))?;

    if &ready0 != b"hi" {
        return Err(format!("Unexpected ready signal from server 0: {:?}", ready0));
    }
    if &ready1 != b"hi" {
        return Err(format!("Unexpected ready signal from server 1: {:?}", ready1));
    }

    println!("Client received ready signals from both servers");
    Ok(())
}

fn setup_network_client(network_config_path: &str) -> Result<(CommTrackingChannel, CommTrackingChannel), String> {
    let network_config = NetworkConfig::from_file(network_config_path)?;

    println!("Connecting to servers...");
    let channel_server0 = connect_to(
        network_config.server0_addr,
        network_config.client_to_server0_port,
    )
    .map_err(|e| format!("Failed to connect to server 0: {}", e))?;
    let channel_server1 = connect_to(
        network_config.server1_addr,
        network_config.client_to_server1_port,
    )
    .map_err(|e| format!("Failed to connect to server 1: {}", e))?;

    Ok((channel_server0, channel_server1))
}


fn print_client_summary(
    share_time: std::time::Duration,
    send_time: std::time::Duration,
    total_time: std::time::Duration,
    bytes_sent_0: usize,
    bytes_sent_1: usize,
) {
    let total_bytes_sent = bytes_sent_0 + bytes_sent_1;

    println!("Client shares sent successfully");
    println!("\n=== Client Performance Summary ===");
    println!("📊 Share generation time: {:.2?}", share_time);
    println!("📊 Share transmission time: {:.2?}", send_time);
    println!("📊 Total client time: {:.2?}", total_time);
    println!("📡 Bytes sent to server 0: {} bytes", bytes_sent_0);
    println!("📡 Bytes sent to server 1: {} bytes", bytes_sent_1);
    println!(
        "📡 Total bytes sent: {} bytes ({:.2} KB)",
        total_bytes_sent,
        total_bytes_sent as f64 / 1024.0
    );
}
/// Run as client - generates shares and sends them to servers
fn run_client(config_path: &str, network_config_path: &str, bench_out: Option<&str>) -> Result<(), String> {
    println!("Starting Client...");
    let cli_config = CliConfig::from_file(config_path)?;

    // Load client data directly from client_points.json
    println!("Loading client data from {}", cli_config.data_file);
    let client_points = load_client_points(&cli_config.data_file)?;

    println!("Loaded {} client points", client_points.len());

    // Generate client shares
    let share_create_start = std::time::Instant::now();
    let share_config = cli_config.to_share_config()?; // Client uses share config
    let enable_sketch = cli_config.protocol.enable_sketch;
    let num_clients = cli_config.protocol.num_clients;
    let client = Client::new(share_config, enable_sketch, num_clients);
    let (shares_server0, shares_server1) = client
        .generate_client_shares(&client_points)
        .map_err(|e| e.to_string())?;
    let share_creation_time = share_create_start.elapsed();

    let (mut channel_server0, mut channel_server1) = setup_network_client(network_config_path)?;
    wait_for_ready_signals(&mut channel_server0, &mut channel_server1)?;

    // Send shares to both servers
    println!("Sending shares to servers...");
    let share_send_start = std::time::Instant::now();
    client
        .send_client_shares(
            shares_server0,
            shares_server1,
            &mut channel_server0,
            &mut channel_server1,
        )
        .map_err(|e| e.to_string())?;
    let share_send_time = share_send_start.elapsed();

    println!("Client sending PC-FSS keys took {:.2?}", share_send_time);

    let sketch_start = std::time::Instant::now();
    let mut sketch_gen_time = std::time::Duration::ZERO;
    if enable_sketch {
        let (sketches0, sketches1) = client
            .generate_client_sketch_data()
            .map_err(|e| e.to_string())?;
        sketch_gen_time = sketch_start.elapsed();

        println!("Client generating sketch data took {:.2?}", sketch_start.elapsed());

        client
            .send_sketch_data(sketches0, sketches1, &mut channel_server0, &mut channel_server1)
            .map_err(|e| e.to_string())?;

        println!("Client sending sketch data took {:.2?}", sketch_start.elapsed());
    }
    let t_sketch_sent_ns = mosaic::bench::unix_ns();

    let total_time = share_creation_time + share_send_time;

    // Calculate communication metrics
    let (bytes_sent_0, _) = channel_server0.get_communication_stats();
    let (bytes_sent_1, _) = channel_server1.get_communication_stats();

    if let Some(out) = bench_out {
        // Benchmark: the client's view of the upload, for server 0's result line.
        let snapshot = mosaic::bench::snapshot();
        let t_first_byte_ns = snapshot["marks"]["client_first_byte"]
            .as_u64()
            .ok_or("bench: the client recorded no first byte")?;
        let part = serde_json::json!({
            "party": "client",
            "n_clients": num_clients,
            "t_first_byte_ns": t_first_byte_ns,
            "t_sketch_sent_ns": t_sketch_sent_ns as u64,
            // Generated between the two sends; A3/B3: not part of the upload.
            "sketch_gen_ns": sketch_gen_time.as_nanos() as u64,
            "share_gen_ms": share_creation_time.as_secs_f64() * 1e3,
            // Application-level bytes written to each server (A4): shares + sketch data.
            "bytes_sent": [bytes_sent_0, bytes_sent_1],
            // One round: the client sends both messages to both servers without waiting for a
            // reply (the "hi" it waits for before is the servers' ready signal, set-up).
            "rounds_upload": 1,
        });
        mosaic::bench::write_json_atomic(&mosaic::bench::part_path(out, "client"), &part)?;
    }

    print_client_summary(
        share_creation_time,
        share_send_time,
        total_time,
        bytes_sent_0,
        bytes_sent_1,
    );

    Ok(())
}

#[derive(Parser, Debug)]
#[command(version, about, long_about = None)]
struct Args {
    #[arg(short, long)]
    config: String,
    #[arg(short, long)]
    network_config: String,
    /// Benchmark (bench/BENCHMARK_MOSAIC.md): the result file of the run. The client writes its
    /// measurements to `<file>.client.part.json` for server 0's result line.
    #[arg(long)]
    bench_out: Option<String>,
}

fn main() {
    let args = Args::parse();
    let config_path = &args.config;
    let network_config_path = &args.network_config;
    if args.bench_out.is_some() {
        mosaic::bench::enable();
    }
    let result = run_client(config_path, network_config_path, args.bench_out.as_deref());
    if let Err(e) = result {
        eprintln!("Error running client: {}", e);
    }
}
