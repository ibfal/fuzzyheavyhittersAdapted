//! CLI for Fuzzy Heavy Hitters Protocol
//!
//! This CLI allows running the complete fuzzy heavy hitters protocol locally
//! with both servers in the same process for testing purposes.

use clap::{Arg, Command};
use mosaic::{
    configs::cli_config::{generate_config, CliConfig},
    data_toolkit::crawl,
    util::{
        bits_to_u128_msb, calculate_distance, calculate_optimistic_distance, get_distance_threshold,
    },
};
use serde_json;
use std::fs;
use std::process;
use std::time::Instant;

/// Load query points from JSON file
fn load_query_points(file_path: &str) -> Result<Vec<Vec<u128>>, String> {
    let content = fs::read_to_string(file_path)
        .map_err(|e| format!("Failed to read query file {}: {}", file_path, e))?;

    serde_json::from_str(&content)
        .map_err(|e| format!("Failed to parse query file {}: {}", file_path, e))
}

/// Load client points directly from JSON file
fn load_client_points(file_path: &str) -> Result<Vec<Vec<u128>>, String> {
    let content = fs::read_to_string(file_path)
        .map_err(|e| format!("Failed to read client points file {}: {}", file_path, e))?;

    serde_json::from_str(&content)
        .map_err(|e| format!("Failed to parse client points file {}: {}", file_path, e))
}

/// Prefix-based search for large input spaces
/// Uses a branch-and-bound approach: if a prefix can't possibly be a heavy hitter,
/// don't explore its extensions
fn find_heavy_hitters_prefix_search(
    client_points: &[Vec<u128>],
    delta: u128,
    threshold: usize,
    input_bit_length: usize,
    distance_metric: &str,
) -> Vec<Vec<u128>> {
    let dimensions = client_points[0].len();
    let distance_threshold = get_distance_threshold(delta, distance_metric);
    let mut heavy_hitters = Vec::new();
    let mut prefixes_to_explore = vec![vec![vec![]; dimensions]]; // Start with empty prefixes
    let mut prefixes_checked = 0;

    while !prefixes_to_explore.is_empty() {
        let mut next_prefixes = Vec::new();

        for prefix_set in prefixes_to_explore {
            prefixes_checked += 1;
            if prefixes_checked % 10_000 == 0 {
                println!(
                    "  Checked {} prefixes, found {} heavy hitters so far...",
                    prefixes_checked,
                    heavy_hitters.len()
                );
            }

            // Convert prefix to actual point and check if it's a heavy hitter
            let point: Vec<u128> = prefix_set
                .iter()
                .map(|prefix| bits_to_u128_msb(prefix))
                .collect();

            // Check if this prefix set represents complete points
            let is_complete = prefix_set
                .iter()
                .all(|prefix| prefix.len() == input_bit_length);
            if is_complete {
                let count = client_points
                    .iter()
                    .filter(|client_point| {
                        calculate_distance(&point, client_point, distance_metric)
                            <= distance_threshold
                    })
                    .count();
                if count >= threshold {
                    heavy_hitters.push(point);
                }
            } else {
                let point_max: Vec<u128> = point
                    .iter()
                    .enumerate()
                    .map(|(i, p)| {
                        p << (input_bit_length - prefix_set[i].len())
                            | ((1u128 << (input_bit_length - prefix_set[i].len())) - 1)
                    })
                    .collect();
                let point_min: Vec<u128> = point
                    .iter()
                    .enumerate()
                    .map(|(i, p)| p << (input_bit_length - prefix_set[i].len()))
                    .collect();

                // Get upper bound estimate for this prefix
                let all_dist: Vec<_> = client_points
                    .iter()
                    .map(|client_point| {
                        calculate_optimistic_distance(
                            &point_max,
                            &point_min,
                            client_point,
                            distance_metric,
                        )
                    })
                    .collect();
                println!("Checking prefix: {:?}", prefix_set);
                println!("All dist: {:?}", &all_dist[..5]);
                let upper_bound = all_dist
                    .iter()
                    .filter(|&dist| dist <= &distance_threshold)
                    .count();

                if upper_bound >= threshold {
                    // This prefix might lead to heavy hitters, so extend it
                    // Find the first dimension that can be extended
                    for dim in 0..dimensions {
                        if prefix_set[dim].len() < input_bit_length {
                            // Try extending with both 0 and 1
                            for bit_value in [false, true] {
                                let mut extended_prefix = prefix_set.clone();
                                extended_prefix[dim].push(bit_value);
                                next_prefixes.push(extended_prefix);
                            }
                            break; // Only extend one dimension at a time
                        }
                    }
                }
                // If upper_bound < threshold, we prune this branch
            }
        }

        prefixes_to_explore = next_prefixes;
    }

    println!(
        "Prefix search completed. Checked {} prefixes total.",
        prefixes_checked
    );
    heavy_hitters
}

/// Run ground truth (non-secure plaintext) protocol for verification
fn run_ground_truth(config_path: &str) -> Result<(), String> {
    let start_time = Instant::now();
    println!("Running Ground Truth (Plaintext) Protocol...");

    let cli_config = CliConfig::from_file(config_path)?;
    let is_known_dictionary = cli_config.protocol.dictionary_type == "Known";

    // Load client data
    println!("Loading client data from {}", cli_config.data_file);
    let client_points = load_client_points(&cli_config.data_file)?;
    println!("Loaded {} client points", client_points.len());

    if is_known_dictionary {
        println!("\n=== Running Known Dictionary Ground Truth ===");

        // Load query points
        println!("Loading query points from {}", cli_config.query_file);
        let query_points = load_query_points(&cli_config.query_file)?;
        println!("Loaded {} query points", query_points.len());

        let distance_threshold = get_distance_threshold(
            cli_config.protocol.delta,
            &cli_config.protocol.distance_metric,
        );
        println!(
            "Distance threshold for delta {}: {}",
            cli_config.protocol.delta, distance_threshold
        );

        // Calculate ground truth results for each query point
        let mut heavy_hitters = Vec::new();
        println!("\nCalculating ground truth for each query point...");
        for query_point in query_points.iter() {
            let count = client_points
                .iter()
                .filter(|client_point| {
                    calculate_distance(
                        query_point,
                        client_point,
                        &cli_config.protocol.distance_metric,
                    ) <= distance_threshold
                })
                .count();
            if count >= cli_config.protocol.match_threshold as usize {
                heavy_hitters.push(query_point);
            }
        }

        // Display summary
        let total_heavy_hitters = heavy_hitters.len();
        println!("\n=== Ground Truth Results Summary ===");
        println!("Protocol: Known Dictionary");
        println!("Distance metric: {}", cli_config.protocol.distance_metric);
        println!("Delta (distance threshold): {}", cli_config.protocol.delta);
        println!(
            "Threshold (minimum count): {}",
            cli_config.protocol.match_threshold
        );
        println!("Total query points: {}", query_points.len());
        println!(
            "Heavy hitters found: {} ({:.1}%)",
            total_heavy_hitters,
            (total_heavy_hitters as f64 / query_points.len() as f64) * 100.0
        );

        // Save results if output file specified
        if let Some(output_file) = &cli_config.output.output_file {
            let results_data = serde_json::json!({
                "protocol_type": "known_dictionary_ground_truth",
                "config": {
                    "distance_metric": cli_config.protocol.distance_metric,
                    "delta": cli_config.protocol.delta.to_string(),
                    "match_threshold": cli_config.protocol.match_threshold.to_string(),
                    "dimensions": cli_config.protocol.d.to_string(),
                },
                "results": query_points.iter().zip(heavy_hitters.iter()).enumerate().map(|(i, (query, result))| {
                    serde_json::json!({
                        "query_id": (i + 1).to_string(),
                        "query_point": query.iter().map(|&x| x.to_string()).collect::<Vec<_>>(),
                        "is_heavy_hitter": result
                    })
                }).collect::<Vec<_>>(),
                "summary": {
                    "total_queries": query_points.len().to_string(),
                    "heavy_hitters": total_heavy_hitters.to_string(),
                    "percentage": (total_heavy_hitters as f64 / query_points.len() as f64 * 100.0).to_string()
                }
            });

            fs::write(
                output_file,
                serde_json::to_string_pretty(&results_data)
                    .map_err(|e| format!("Failed to serialize results: {}", e))?,
            )
            .map_err(|e| format!("Failed to write results to {}: {}", output_file, e))?;
            println!("Results saved to {}", output_file);
        }
    } else {
        println!("\n=== Running Unknown Dictionary Ground Truth ===");

        // Find all fuzzy heavy hitters using prefix-based search. With a derived coordinate
        // (H3), search like the servers: skip it and check it at the full points.
        let ground_truth_heavy_hitters = match &cli_config.protocol.derived_coordinate {
            None => find_heavy_hitters_prefix_search(
                &client_points,
                cli_config.protocol.delta,
                cli_config.protocol.match_threshold as usize,
                cli_config.protocol.h1,
                &cli_config.protocol.distance_metric,
            ),
            Some(derived) => {
                let (d, h1) = (cli_config.protocol.d, cli_config.protocol.h1);
                let levels = crawl::crawl_with_derived(
                    &client_points,
                    d,
                    h1,
                    crawl::Metric::parse(&cli_config.protocol.distance_metric)?,
                    cli_config.protocol.delta,
                    cli_config.protocol.match_threshold as usize,
                    Some(derived),
                );
                println!(
                    "Crawl skipping coordinate {}: {} prefixes checked",
                    derived.index,
                    levels.iter().map(|l| l.candidates.len()).sum::<usize>()
                );
                crawl::heavy_hitters(&levels, crawl::total_levels(d, h1, Some(derived)))
                    .iter()
                    .map(|c| c.prefix.iter().map(|&(v, _)| v).collect())
                    .collect()
            }
        };

        // Display results
        println!("\n=== Ground Truth Results Summary ===");
        println!("Protocol: Unknown Dictionary");
        println!("Distance metric: {}", cli_config.protocol.distance_metric);
        println!("Delta (distance threshold): {}", cli_config.protocol.delta);
        println!(
            "Match threshold (minimum count): {}",
            cli_config.protocol.match_threshold
        );
        println!("Input bit length: {}", cli_config.protocol.h1);
        println!("Dimensions: {}", cli_config.protocol.d);
        println!("Total client points: {}", client_points.len());
        println!("Heavy hitters found: {}", ground_truth_heavy_hitters.len());

        if cli_config.output.verbose {
            println!("\nHeavy hitter values:");
            for (i, heavy_hitter) in ground_truth_heavy_hitters.iter().enumerate() {
                println!("  {}: {:?}", i + 1, heavy_hitter);
            }
        }

        // Save results if output file specified
        if let Some(output_file) = &cli_config.output.output_file {
            let results_data = serde_json::json!({
                "protocol_type": "unknown_dictionary_ground_truth",
                "config": {
                    "distance_metric": cli_config.protocol.distance_metric,
                    "delta": cli_config.protocol.delta.to_string(),
                    "match_threshold": cli_config.protocol.match_threshold.to_string(),
                    "dimensions": cli_config.protocol.d.to_string(),
                    "input_bit_length": cli_config.protocol.h1.to_string()
                },
                "heavy_hitters": ground_truth_heavy_hitters.iter().map(|hh| {
                    hh.iter().map(|&x| x.to_string()).collect::<Vec<_>>()
                }).collect::<Vec<_>>(),
                "summary": {
                    "total_client_points": client_points.len().to_string(),
                    "heavy_hitters_count": ground_truth_heavy_hitters.len().to_string(),
                }
            });

            fs::write(
                output_file,
                serde_json::to_string_pretty(&results_data)
                    .map_err(|e| format!("Failed to serialize results: {}", e))?,
            )
            .map_err(|e| format!("Failed to write results to {}: {}", output_file, e))?;
            println!("Results saved to {}", output_file);
        }
    }

    let total_time = start_time.elapsed();
    println!("\n=== Ground Truth Performance Summary ===");
    println!("📊 Total computation time: {:.2?}", total_time);

    Ok(())
}

fn main() {
    let matches = Command::new("Fuzzy Heavy Hitters CLI")
        .version("1.0")
        .about("CLI for running the fuzzy heavy hitters protocol in distributed or local mode")
        .subcommand(
            Command::new("generate-config")
                .about("Generate a sample configuration file")
                .arg(
                    Arg::new("output")
                        .short('o')
                        .long("output")
                        .value_name("FILE")
                        .help("Output configuration file path")
                        .default_value("fhh_config.json"),
                ),
        )
        .subcommand(
            Command::new("ground-truth")
                .about("Run ground truth (non-secure plaintext) protocol for verification")
                .arg(
                    Arg::new("config")
                        .short('c')
                        .long("config")
                        .value_name("FILE")
                        .help("Configuration file path")
                        .required(true),
                ),
        )
        .get_matches();

    let result = match matches.subcommand() {
        Some(("generate-config", sub_matches)) => {
            let output_path = sub_matches
                .get_one::<String>("output")
                .expect("output has a default value");
            generate_config(output_path.as_str())
        }
        Some(("ground-truth", sub_matches)) => {
            let config_path = sub_matches
                .get_one::<String>("config")
                .expect("config is required");
            run_ground_truth(config_path.as_str())
        }
        _ => {
            eprintln!("No subcommand specified. Use --help for usage information.");
            eprintln!("Available commands:");
            eprintln!("  generate-config - Generate sample configuration");
            eprintln!("  ground-truth  - Run ground truth (non-secure) protocol");
            process::exit(1);
        }
    };

    if let Err(e) = result {
        eprintln!("Error: {}", e);
        process::exit(1);
    }
}
