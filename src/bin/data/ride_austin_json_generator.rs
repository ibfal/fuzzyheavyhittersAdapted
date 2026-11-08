use clap::Parser;
use mosaic::data_toolkit::encoding::{
    cli::EncodingArgs, encode_csv, print_summary, write_metadata, PointKind,
};
use serde_json;
use std::error::Error;
use std::fs::File;
use std::io::BufWriter;
use std::path::Path;

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
    #[arg(short, long)]
    input: String,
    #[arg(short, long)]
    output: String,
    /// 0: pickup only, 1: pickup and drop-off.
    #[arg(short, long)]
    output_type: u8,
    #[command(flatten)]
    encoding: EncodingArgs,
}

fn main() -> Result<(), Box<dyn Error>> {
    let args = Args::parse();
    let input_file = &args.input;
    let output_file = &args.output;

    println!("Starting RideAustin client points JSON generation...");

    // Check if input file exists
    if !Path::new(input_file).exists() {
        eprintln!("Error: Input file '{}' does not exist.", input_file);
        eprintln!("Please make sure you have the ride data CSV file at this location.");
        return Ok(());
    }

    // Read and encode CSV data
    println!("Reading and converting CSV data from {}...", input_file);
    let encoding = args.encoding.to_encoding();
    let point_kind = PointKind::from_output_type(args.output_type)?;
    let dataset = encode_csv(input_file, point_kind, &encoding)?;
    print_summary(&dataset.metadata);
    let client_points = dataset.points;

    // Write to JSON file in the same format as data/synthetic/client_points.json
    println!("Writing client points JSON to {}...", output_file);
    let file = File::create(output_file)?;
    let writer = BufWriter::new(file);
    serde_json::to_writer_pretty(writer, &client_points)?;
    let metadata_file = write_metadata(output_file, &dataset.metadata)?;

    println!("Successfully generated client points JSON!");
    println!("Total points: {}", client_points.len());
    println!("Output file: {}", output_file);
    println!("Encoding metadata: {}", metadata_file.display());

    // Print some sample statistics
    if !client_points.is_empty() {
        println!("\nSample points:");
        for (i, point) in client_points.iter().take(5).enumerate() {
            println!("Point {}: {:?}", i + 1, point);
        }
    }

    Ok(())
}
