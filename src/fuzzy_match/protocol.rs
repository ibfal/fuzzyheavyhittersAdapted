//! Fuzzy Heavy Hitters Protocol Implementation
//!
//! This module provides a high-level interface for running the complete fuzzy heavy hitters protocol
//! including share phase, check phase, and threshold phase.

use crate::{
    channel::CommTrackingChannel,
    data_structures::modp::{Modp, BarrettCtx},
    configs::{
        cli_config::{DerivedCoordinate, ProtocolParameters},
        method_config::MethodConfig,
    },
    fuzzy_match::{
        dealer::{DealerSignal, SerializedDpfKeyBatch, SerializedFssKeyBatch},
        share_phase::SharePhase,
        share_phase_types::{DistanceMetric, ShareMethod},
        shared_range::{ShareData, SharedRange},
        sketch_phase::SketchPhase,
        sketch_phase_types::{SketchData, SketchValues, VerifyValues},
        check_phase::CheckPhase,
        check_phase_types::{CheckData, CheckMethod, CheckProperty},
        threshold_phase::ThresholdPhase,
        threshold_phase_types::{ThresholdData, ThresholdMethod},
    },
    randomness::prg::PRG,
    util::{get_distance_threshold, receive_bool_vec, send_bool_vec, u128_to_bits_msb},
};
use rayon::prelude::*;
use scuttlebutt::{AbstractChannel, AesRng};
use std::convert::TryInto;
use anyhow::{anyhow, ensure, Result};

/// Main protocol structure that encapsulates the entire fuzzy heavy hitters protocol
#[derive(Clone)]
pub struct MosaicProtocol {
    share_phase: SharePhase,
    sketch_phase: SketchPhase,
    check_phase: CheckPhase,
    threshold_phase: ThresholdPhase,
    role: bool, 
    enable_sketch: bool,
    num_clients: usize,
    match_threshold: u128,
    /// See [`DerivedCoordinate`]: a coordinate the unknown-dictionary crawl does not branch on.
    derived: Option<DerivedCoordinate>,
}

impl MosaicProtocol {
    /// Create a new protocol instance
    pub fn new(
        protocol_config: ProtocolParameters,
        method_config: MethodConfig,
        role: bool,
        enable_sketch: bool,
        num_clients: usize,
        match_threshold: u128,
    ) -> Self {
        let derived = protocol_config.derived_coordinate;
        if let Some(derived) = &derived {
            derived
                .validate(protocol_config.d)
                .unwrap_or_else(|e| panic!("invalid protocol config: {}", e));
        }
        let share_phase = SharePhase::new(protocol_config.clone());
        let check_phase = CheckPhase::new((protocol_config.clone(), method_config.clone()), role);
        let sketch_phase = SketchPhase::new(protocol_config.clone());
        let threshold_phase = ThresholdPhase::new((protocol_config, method_config), role);
        Self {
            share_phase,
            sketch_phase,
            check_phase,
            threshold_phase,
            role,
            enable_sketch,
            num_clients,
            match_threshold,
            derived,
        }
    }

    pub fn receive_client_shares(
        &self,
        client_channel: &mut CommTrackingChannel,
    ) -> Result<Vec<SharedRange>> {
        // Receive shares using custom serialization
        let mut len_bytes = [0u8; 8];
        client_channel
            .read_bytes(&mut len_bytes)
            .map_err(|e| anyhow!("Failed to read length from client: {}", e))?;
        let len = u64::from_le_bytes(len_bytes) as usize;
        let mut shares_data = vec![0u8; len];
        client_channel
            .read_bytes(&mut shares_data)
            .map_err(|e| anyhow!("Failed to receive shares from client: {}", e))?;

        // Custom deserialization for Vec<SharedRange>
        let bytes = &shares_data[..];
        ensure!(bytes.len() >= 8, "Invalid shares data length");
        let mut offset = 0;
        let shared_range_count = u64::from_le_bytes(bytes[offset..offset + 8].try_into().unwrap()) as usize;
        ensure!(shared_range_count == self.num_clients, "Mismatch in number of client shares received, expected {}, got {}", self.num_clients, shared_range_count);
        offset += 8;

        let modulus = 1u128 << self.h2();
        let mut shares = Vec::with_capacity(shared_range_count);
        for _ in 0..shared_range_count {
            let (share, share_used) = SharedRange::from_bytes(&bytes[offset..], modulus)
                .map_err(|e| anyhow!("Failed to deserialize SharedRange: {}", e))?;
            shares.push(share);
            offset += share_used;
        }

        Ok(shares)
    }

    pub fn receive_client_sketch_data<'a>(
        &'a self,
        client_channel: &mut CommTrackingChannel,
    ) -> Result<Vec<Vec<SketchData<'a>>>> {
        ensure!(self.enable_sketch, "Sketch data reception called but sketching is disabled");

        let mut len_bytes = [0u8; 8];
        client_channel
            .read_bytes(&mut len_bytes)
            .map_err(|e| anyhow!("Failed to read length from client: {}", e))?;
        let len = u64::from_le_bytes(len_bytes) as usize;

        let mut sketch_data_buf = vec![0u8; len];
        client_channel
            .read_bytes(&mut sketch_data_buf)
            .map_err(|e| anyhow!("Failed to read sketch data from client: {}", e))?;

        // Custom deserialization for Vec<Vec<SketchData>>
        let bytes = &sketch_data_buf[..];
        ensure!(bytes.len() >= 8, "Invalid sketch data length");
        let mut offset = 0;
        let sketch_data_count = u64::from_le_bytes(bytes[offset..offset + 8].try_into().unwrap()) as usize;
        ensure!(sketch_data_count == self.num_clients, "Mismatch in number of client sketches received, expected {}, got {}", self.num_clients, sketch_data_count);
        offset += 8;
        let d = usize::from_le_bytes(bytes[offset..offset + 8].try_into().unwrap());
        ensure!(d == self.d(), "Mismatch in sketch data dimensions, expected {}, got {}", self.d(), d);
        offset += 8;

        let barrett_ctx = self.barrett_ctx();

        let mut sketch_data_vec = Vec::with_capacity(sketch_data_count);
        for _ in 0..sketch_data_count {
            let mut sketches_per_client = Vec::with_capacity(d);
            for _ in 0..d {
                let (sketch_data, used_bytes) = SketchData::from_bytes(&barrett_ctx, &bytes[offset..])
                    .map_err(|e| anyhow!("Failed to deserialize SketchData: {}", e))?;
                sketches_per_client.push(sketch_data);
                offset += used_bytes;
            }
            sketch_data_vec.push(sketches_per_client);
        }

        Ok(sketch_data_vec)
    }

    pub fn verify_client_shared_ranges(
        &self,
        client_shares: &[SharedRange],
        sketch_data: &[Vec<SketchData>],
        prg: &mut PRG,
        other_server_channels: &mut [CommTrackingChannel],
    ) -> Result<Vec<bool>> {
        ensure!(self.enable_sketch, "Sketch verification called but sketching is disabled");
        ensure!(client_shares.len() == sketch_data.len(), 
            "Mismatch in client shares and sketch data length, client_shares.len() = {}, sketch_data.len() = {}", 
            client_shares.len(), 
            sketch_data.len()
        );
        ensure!(other_server_channels.len() > 0, "No other server channels provided for sketch verification");

        let num_threads = other_server_channels.len().max(1);
        let chunk_size = (client_shares.len() + num_threads - 1) / num_threads.max(1);
        let mut malicious_flags = vec![false; client_shares.len()];
        let mut sketch_seeds = vec![[0u8; 16]; client_shares.len()];
        prg.random_16byte_block(&mut sketch_seeds);

        malicious_flags
            .par_chunks_mut(chunk_size)
            .zip(client_shares.par_chunks(chunk_size))
            .zip(sketch_data.par_chunks(chunk_size))
            .zip(sketch_seeds.par_chunks(chunk_size))
            .zip(other_server_channels.par_iter_mut())
            .try_for_each(|((((malicious_flags_chunk, range_chunk), sketch_data_chunk), sketch_seeds_chunk), other_server_channel)| -> Result<()> {
                let start = std::time::Instant::now();

                let sketch_helper = self.sketch_phase
                    .get_sketch_helper()
                    .map_err(|e| anyhow!("Failed to build sketch helper: {}", e))?;

                println!("Sketch helper built in {:?}", start.elapsed());

                let sketch_values = range_chunk.iter()
                    .zip(sketch_seeds_chunk.iter())
                    .map(|(shared_range, seed)| {
                        let mut prg_sketch = PRG::new(Some(&seed), 0); 
                        // let start = std::time::Instant::now();
                        let sketch = self.sketch_phase
                            .sketch(shared_range, &sketch_helper, &mut prg_sketch)
                            .map_err(|e| anyhow!("Sketch failed: {}", e));
                        // println!("Sketch for one client done in {:?}", start.elapsed());
                        sketch
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                
                println!("Sketches generated in {:?}", start.elapsed());
                println!("Number of sketches: {}", sketch_values.len());
                println!("Length of one sketch: {}", sketch_values[0].len());

                let sketch_values_flatten: Vec<SketchValues> = sketch_values.iter()
                    .flat_map(|v| v.iter().cloned())
                    .collect();

                println!("Sketch values flattened in {:?}", start.elapsed());
                println!("Number of sketch values: {}", sketch_values_flatten.len());

                let sketch_data_flatten: Vec<SketchData> = sketch_data_chunk.iter()
                    .flat_map(|v| v.iter().cloned())
                    .collect();

                println!("Sketch data flattened in {:?}", start.elapsed());
                println!("Number of sketch data entries: {}", sketch_data_flatten.len());

                let verify_values = self.sketch_phase
                    .batch_verify(&sketch_values_flatten, &sketch_data_flatten, other_server_channel, self.role())
                    .map_err(|e| anyhow!("Batch verify failed: {}", e))?;

                println!("Verify values obtained in {:?}", start.elapsed());

                let verify_values_flattened: Vec<Modp> = verify_values.iter()
                    .flat_map(|v| 
                       flatten_verify_values(v)
                    ).collect();

                println!("Verify values flattened in {:?}", start.elapsed());
                println!("Number of verify values: {}", verify_values_flattened.len());

                let other_verify_values_flattened = if self.role() {
                    self.send_modp_vec(&verify_values_flattened, other_server_channel)
                        .map_err(|e| anyhow!("Failed to send verify values to other server: {}", e))?;
                    self.recv_modp_vec(other_server_channel)
                        .map_err(|e| anyhow!("Failed to receive verify values from other server: {}", e))?
                } else {
                    let other_values = self.recv_modp_vec(other_server_channel)
                        .map_err(|e| anyhow!("Failed to receive verify values from other server: {}", e))?;
                    self.send_modp_vec(&verify_values_flattened, other_server_channel)
                        .map_err(|e| anyhow!("Failed to send verify values to other server: {}", e))?;
                    other_values
                };

                println!("Other server verify values received in {:?}", start.elapsed());

                ensure!(other_verify_values_flattened.len() == verify_values_flattened.len(),
                    "Mismatch in verify values length between servers, local length = {}, remote length = {}",
                    verify_values_flattened.len(),
                    other_verify_values_flattened.len()
                );

                let one_flattened_length = verify_values_flattened.len() / verify_values.len();

                for (i, malicious_flag) in malicious_flags_chunk.iter_mut().enumerate() {
                    for j in 0..one_flattened_length {
                        let local_value = &verify_values_flattened[i * one_flattened_length + j];
                        let remote_value = &other_verify_values_flattened[i * one_flattened_length + j];
                        let total_value = *local_value - *remote_value;
                        if total_value.value() != 0 {
                            *malicious_flag = true;
                            break;
                        }
                    }
                }

                Ok(())
            })?;

        Ok(malicious_flags)
    }

    /// Parallel version of run_server_known_dictionary using multiple channels for both dealer and other server
    pub fn run_server_known_dictionary_parallel(
        &self,
        client_shares: &[SharedRange],
        query_points: &[Vec<u128>],
        signal_dealer_channels: &mut [CommTrackingChannel],
        other_server_channels: &mut [CommTrackingChannel],
    ) -> Result<Vec<bool>, String> {
        // Convert query points from u128 to Vec<Vec<bool>>
        let query_point_sets: Vec<Vec<Vec<bool>>> = query_points
            .iter()
            .map(|query_point| {
                query_point
                    .iter()
                    .map(|&point| u128_to_bits_msb(point, self.h1()))
                    .collect()
            })
            .collect();

        let evals = query_point_sets
            .iter()
            .map(|query_point| {
                client_shares
                    .iter()
                    .map(|range| {
                        (0..self.d())
                            .map(|i| {
                                self.share_phase
                                    .evaluate_at_single_dimension(range, &query_point[i], i)
                                    .unwrap()
                            })
                            .collect::<Vec<u128>>()
                    })
                    .collect::<Vec<Vec<u128>>>()
            })
            .collect::<Vec<Vec<Vec<u128>>>>();

        // Use parallel batch processing with both dealer and server channels
        println!(
            "Using full parallel processing with {} signal dealer channels and {} server channels",
            signal_dealer_channels.len(),
            other_server_channels.len()
        );
        let final_results = self.batch_check(
            &evals,
            signal_dealer_channels,
            other_server_channels,
        )?;

        // Shutdown dealers using all signal dealer channels
        for dealer_channel in signal_dealer_channels.iter_mut() {
            shutdown_dealer(dealer_channel)?;
        }

        Ok(final_results)
    }

    /// Parallel version of run_server_unknown_dictionary using multiple channels
    pub fn run_server_unknown_dictionary_parallel(
        &self,
        client_shares_list: &[SharedRange],
        signal_dealer_channels: &mut [CommTrackingChannel],
        other_server_channels: &mut [CommTrackingChannel],
    ) -> Result<Vec<Vec<u128>>, String> {
        if client_shares_list.is_empty() {
            println!("No client shares available; skipping unknown dictionary protocol.");
            return Ok(Vec::new());
        }
        let max_bit_length = self.h1();
        let dimension = self.d();
        let eval_len = self.h2();
        let eval_modulus = 1u128 << eval_len;
        // Evaluate the empty prefix for each dimension
        let empty_string_data = client_shares_list
            .iter()
            .map(|shared_range| {
                let data = self.share_phase.share_data_init(shared_range).unwrap();
                data.to_bytes(eval_len)
            })
            .collect::<Result<Vec<Vec<u8>>, _>>()
            .map_err(|e| e.to_string())?;
        let mut current_data = vec![empty_string_data];
        let mut current_prefixes: Vec<Vec<Vec<bool>>> = vec![vec![vec![]; dimension]];
        let crawl_dims = DerivedCoordinate::crawl_dims(self.derived.as_ref(), dimension);

        // let num_threads = other_server_channels.len();

        for prefix_length in 1..=max_bit_length {
            for &dim in &crawl_dims {
                let start = std::time::Instant::now();
                // let mut new_data: Vec<Vec<Vec<u8>>> = Vec::new();
                // for (data, prefix) in current_data.iter().zip(current_prefixes.iter()) {
                //     let mut data0 = Vec::with_capacity(client_shares_list.len());
                //     let mut data1 = Vec::with_capacity(client_shares_list.len());
                //     for (idx, shared_range) in client_shares_list.iter().enumerate() {
                //         let (data_dim, _) =
                //             ShareData::from_bytes(&data[idx], eval_len, eval_modulus)
                //                 .map_err(|e| e.to_string())?;

                //         let (eval0, eval1) = self
                //             .share_phase
                //             .expand_prefix(shared_range, &data_dim, &prefix[dim], dim)
                //             .map_err(|e| e.to_string())?;

                //         data0.push(eval0.to_bytes(eval_len).map_err(|e| e.to_string())?);
                //         data1.push(eval1.to_bytes(eval_len).map_err(|e| e.to_string())?);
                //     }

                //     new_data.push(data0);
                //     new_data.push(data1);
                // }
                let new_data_pairs = current_data
                    .par_iter()
                    .zip(current_prefixes.par_iter())
                    .map(|(data, prefix)| {
                        // build data0 and data1 for this prefix
                        let mut data0 = Vec::with_capacity(client_shares_list.len());
                        let mut data1 = Vec::with_capacity(client_shares_list.len());
                        for (idx, shared_range) in client_shares_list.iter().enumerate() {
                            let (data_dim, _) =
                                ShareData::from_bytes(&data[idx], eval_len, eval_modulus)
                                    .map_err(|e| e.to_string())?;

                            let (eval0, eval1) = self
                                .share_phase
                                .expand_prefix(shared_range, &data_dim, &prefix[dim], dim)
                                .map_err(|e| e.to_string())?;

                            data0.push(eval0.to_bytes(eval_len).map_err(|e| e.to_string())?);
                            data1.push(eval1.to_bytes(eval_len).map_err(|e| e.to_string())?);
                        }
                        Ok::<_, String>((data0, data1))

                    })
                    .collect::<Result<Vec<_>, _>>()?;

                let mut new_data = Vec::with_capacity(new_data_pairs.len() * 2);
                for (data0, data1) in new_data_pairs {
                    new_data.push(data0);
                    new_data.push(data1);
                }
                println!("Time to collect new data: {:?}", start.elapsed());
                println!("Number of new data stored: {}", new_data.len());
                println!(
                    "Memory consumption of new_data: {:?} bytes",
                    new_data
                        .iter()
                        .map(|data_bytes| data_bytes.iter().map(|b| b.len()).sum::<usize>())
                        .sum::<usize>()
                );

                crate::util::print_memory_usage();

                let new_eval = new_data
                    .par_iter()
                    .map(|data_bytes| {
                        data_bytes
                            .iter()
                            .map(|bytes| {
                                let (share_data, _) =
                                    ShareData::from_bytes(bytes, eval_len, eval_modulus)
                                        .map_err(|e| e.to_string())?;
                                let evals = match share_data {
                                    ShareData::OKVS { eval } => eval,
                                    ShareData::IntervalFSS { data } => data
                                        .iter()
                                        .map(|eval| eval.result()[0])
                                        .collect::<Vec<u128>>(),
                                    ShareData::DistanceFSS { eval, .. } => eval,
                                };
                                Ok(evals)
                            })
                            .collect::<Result<Vec<Vec<u128>>, String>>()
                    })
                    .collect::<Result<Vec<Vec<Vec<u128>>>, String>>();
                let new_eval = new_eval?;

                println!(
                    "Time to expand prefixes for all clients: {:?}",
                    start.elapsed()
                );

                let mut new_prefixes = Vec::new();
                for prefix in current_prefixes.iter() {
                    let mut new_prefix = prefix.clone();
                    new_prefix[dim].push(false);
                    new_prefixes.push(new_prefix.clone());
                    new_prefix[dim].pop();
                    new_prefix[dim].push(true);
                    new_prefixes.push(new_prefix);
                }

                println!("Time to expand prefixes: {:?}", start.elapsed());

                let exceeds_threshold_results = self.batch_check(
                    &new_eval,
                    signal_dealer_channels,
                    other_server_channels,
                )?;

                println!("Time for batch check: {:?}", start.elapsed());

                let start_copy = std::time::Instant::now();

                crate::util::print_memory_usage();

                // Use std::mem::take to move qualifying entries out without cloning.
                current_data.clear();
                current_prefixes.clear();
                for ((data, prefix), &exceed) in new_data
                    .iter_mut()
                    .zip(new_prefixes.iter_mut())
                    .zip(exceeds_threshold_results.iter())
                {
                    if exceed {
                        current_data.push(std::mem::take(data));
                        current_prefixes.push(std::mem::take(prefix));
                    }
                }
                // new_eval.clear();
                // new_data.clear();
                // new_prefixes.clear();
                drop(new_eval);
                drop(new_data);
                drop(new_prefixes);

                crate::util::print_memory_usage();

                // new_data/new_prefixes now contain empty Vecs for moved entries; they'll be dropped.
                println!(
                    "Time to filter data based on threshold (move-based): {:?}",
                    start_copy.elapsed()
                );

                println!(
                    "Processed dimension {} with prefix length {} in {:?}",
                    dim,
                    prefix_length,
                    start.elapsed()
                );
                // println!("Exceed threshold results: {:?}", exceeds_threshold_results);
            }
        }
        if self.derived.is_some() {
            let leaves = current_prefixes.into_iter().zip(current_data).collect();
            current_prefixes = self.check_derived_coordinate(
                client_shares_list,
                leaves,
                signal_dealer_channels,
                other_server_channels,
            )?;
        }
        let final_heavy_hitters = current_prefixes
            .into_iter()
            .map(|prefix| {
                // Convert each prefix vector (Vec<bool>) back to a byte vector (Vec<u8>)
                prefix
                    .iter()
                    .map(|bits| {
                        bits.iter()
                            .fold(0u128, |acc, &bit| (acc << 1) | if bit { 1 } else { 0 })
                    })
                    .collect::<Vec<u128>>()
            })
            .collect();

        // Shutdown dealers using all signal dealer channels
        for dealer_channel in signal_dealer_channels.iter_mut() {
            shutdown_dealer(dealer_channel)?;
        }

        Ok(final_heavy_hitters)
    }


    /// Chunked-memory variant of run_server_unknown_dictionary_parallel.
    pub fn run_server_unknown_dictionary_parallel2(
        &self,
        client_shares_list: &[SharedRange],
        signal_dealer_channels: &mut [CommTrackingChannel],
        other_server_channels: &mut [CommTrackingChannel],
    ) -> Result<Vec<Vec<u128>>, String> {
        if client_shares_list.is_empty() {
            println!("No client shares available; skipping unknown dictionary protocol.");
            return Ok(Vec::new());
        }

        struct TraversalState {
            data: Vec<Vec<u8>>,
            prefix: Vec<Vec<bool>>,
            prefix_length: usize,
            /// Position in `crawl_dims` of the coordinate that gets the next bit.
            pos: usize,
        }

        let max_bit_length = self.h1();
        let dimension = self.d();
        let eval_len = self.h2();
        let eval_modulus = 1u128 << eval_len;
        let num_threads = other_server_channels.len().max(1);
        let expansion_window = (num_threads * 4).max(1);
        let crawl_dims = DerivedCoordinate::crawl_dims(self.derived.as_ref(), dimension);

        let empty_string_data = client_shares_list
            .iter()
            .map(|shared_range| {
                let data = self.share_phase.share_data_init(shared_range).unwrap();
                data.to_bytes(eval_len)
            })
            .collect::<Result<Vec<Vec<u8>>, _>>()
            .map_err(|e| e.to_string())?;
        let mut current_data = vec![TraversalState {
            data: empty_string_data,
            prefix: vec![vec![]; dimension],
            prefix_length: 0,
            pos: 0,
        }];
        let mut final_prefixes: Vec<Vec<Vec<bool>>> = Vec::new();
        // With a derived coordinate the full points still need their final check, which uses
        // their evaluations.
        let mut final_states: Vec<(Vec<Vec<bool>>, Vec<Vec<u8>>)> = Vec::new();

        while !current_data.is_empty() {
            let start = std::time::Instant::now();
            let split_at = current_data.len().saturating_sub(expansion_window);
            let expand_states = current_data.split_off(split_at);

            let expanded_states = expand_states
                .par_iter()
                .map(|state| {
                    let dim = crawl_dims[state.pos];
                    let mut data0 = Vec::with_capacity(client_shares_list.len());
                    let mut data1 = Vec::with_capacity(client_shares_list.len());
                    for (idx, shared_range) in client_shares_list.iter().enumerate() {
                        let (data_dim, _) =
                            ShareData::from_bytes(&state.data[idx], eval_len, eval_modulus)
                                .map_err(|e| e.to_string())?;

                        let (eval0, eval1) = self
                            .share_phase
                            .expand_prefix(shared_range, &data_dim, &state.prefix[dim], dim)
                            .map_err(|e| e.to_string())?;

                        data0.push(eval0.to_bytes(eval_len).map_err(|e| e.to_string())?);
                        data1.push(eval1.to_bytes(eval_len).map_err(|e| e.to_string())?);
                    }

                    let next_pos = (state.pos + 1) % crawl_dims.len();
                    let next_prefix_length = state.prefix_length + usize::from(next_pos == 0);

                    let mut prefix0 = state.prefix.clone();
                    prefix0[dim].push(false);
                    let mut prefix1 = state.prefix.clone();
                    prefix1[dim].push(true);

                    Ok::<_, String>([
                        TraversalState {
                            data: data0,
                            prefix: prefix0,
                            prefix_length: next_prefix_length,
                            pos: next_pos,
                        },
                        TraversalState {
                            data: data1,
                            prefix: prefix1,
                            prefix_length: next_prefix_length,
                            pos: next_pos,
                        },
                    ])
                })
                .collect::<Result<Vec<_>, _>>()?;

            let mut new_states = Vec::with_capacity(expanded_states.len() * 2);
            for pair in expanded_states {
                new_states.extend(pair);
            }

            println!("Time to collect new data: {:?}", start.elapsed());
            println!("Number of new states stored in chunk: {}", new_states.len());
            println!(
                "Memory consumption of chunk new_data: {:?} bytes",
                new_states
                    .iter()
                    .map(|state| state.data.iter().map(|b| b.len()).sum::<usize>())
                    .sum::<usize>()
            );

            crate::util::print_memory_usage();

            let new_eval = new_states
                .par_iter()
                .map(|state| {
                    state
                        .data
                        .iter()
                        .map(|bytes| {
                            let (share_data, _) = ShareData::from_bytes(bytes, eval_len, eval_modulus)
                                .map_err(|e| e.to_string())?;
                            let evals = match share_data {
                                ShareData::OKVS { eval } => eval,
                                ShareData::IntervalFSS { data } => data
                                    .iter()
                                    .map(|eval| eval.result()[0])
                                    .collect::<Vec<u128>>(),
                                ShareData::DistanceFSS { eval, .. } => eval,
                            };
                            Ok(evals)
                        })
                        .collect::<Result<Vec<Vec<u128>>, String>>()
                })
                .collect::<Result<Vec<Vec<Vec<u128>>>, String>>()?;

            let exceeds_threshold_results = self.batch_check(
                &new_eval,
                signal_dealer_channels,
                other_server_channels,
            )?;

            for (state, &exceed) in new_states.iter_mut().zip(exceeds_threshold_results.iter()) {
                if !exceed {
                    continue;
                }

                if state.prefix_length == max_bit_length {
                    if self.derived.is_some() {
                        final_states.push((
                            std::mem::take(&mut state.prefix),
                            std::mem::take(&mut state.data),
                        ));
                    } else {
                        final_prefixes.push(std::mem::take(&mut state.prefix));
                    }
                } else {
                    current_data.push(TraversalState {
                        data: std::mem::take(&mut state.data),
                        prefix: std::mem::take(&mut state.prefix),
                        prefix_length: state.prefix_length,
                        pos: state.pos,
                    });
                }
            }

            println!(
                "Processed frontier chunk with {} survivors and {} finals in {:?}",
                current_data.len(),
                final_prefixes.len() + final_states.len(),
                start.elapsed()
            );
        }

        if self.derived.is_some() {
            final_prefixes = self.check_derived_coordinate(
                client_shares_list,
                final_states,
                signal_dealer_channels,
                other_server_channels,
            )?;
        }

        let final_heavy_hitters = final_prefixes
            .into_iter()
            .map(|prefix| {
                prefix
                    .iter()
                    .map(|bits| {
                        bits.iter()
                            .fold(0u128, |acc, &bit| (acc << 1) | if bit { 1 } else { 0 })
                    })
                    .collect::<Vec<u128>>()
            })
            .collect();

        for dealer_channel in signal_dealer_channels.iter_mut() {
            shutdown_dealer(dealer_channel)?;
        }

        Ok(final_heavy_hitters)
    }

    /// Last step of a crawl that skipped the derived coordinate. Every leaf is a full point on
    /// the crawled coordinates; set its derived coordinate to the only possible value, evaluate
    /// the clients' keys there, and keep the points that are still heavy hitters with every
    /// coordinate checked. Leaves whose derived value does not fit in `h1` bits are dropped:
    /// no client point has those coordinates. Both servers hold the same leaves, so they drop
    /// the same ones and call `batch_check` together.
    fn check_derived_coordinate(
        &self,
        client_shares_list: &[SharedRange],
        leaves: Vec<(Vec<Vec<bool>>, Vec<Vec<u8>>)>,
        signal_dealer_channels: &mut [CommTrackingChannel],
        other_server_channels: &mut [CommTrackingChannel],
    ) -> Result<Vec<Vec<Vec<bool>>>, String> {
        let derived = match self.derived {
            Some(derived) => derived,
            None => return Ok(leaves.into_iter().map(|(prefix, _)| prefix).collect()),
        };
        let (h1, eval_len) = (self.h1(), self.h2());
        let eval_modulus = 1u128 << eval_len;
        let start = std::time::Instant::now();
        let num_leaves = leaves.len();

        let leaves: Vec<(Vec<Vec<bool>>, Vec<Vec<u8>>)> = leaves
            .into_iter()
            .filter_map(|(mut prefix, data)| {
                let point: Vec<u128> = prefix.iter().map(|bits| bits_to_u128(bits)).collect();
                let value = derived.value(&point, h1)?;
                prefix[derived.index] = u128_to_bits_msb(value, h1);
                Some((prefix, data))
            })
            .collect();
        println!(
            "Derived coordinate {}: checking {} of {} crawled points ({} out of range)",
            derived.index,
            leaves.len(),
            num_leaves,
            num_leaves - leaves.len()
        );
        if leaves.is_empty() {
            return Ok(Vec::new());
        }

        let evals = leaves
            .par_iter()
            .map(|(prefix, data)| {
                data.iter()
                    .zip(client_shares_list)
                    .map(|(bytes, shared_range)| {
                        let (share_data, _) = ShareData::from_bytes(bytes, eval_len, eval_modulus)
                            .map_err(|e| e.to_string())?;
                        let mut evals = share_data_evals(share_data);
                        evals[derived.index] = self
                            .share_phase
                            .evaluate_at_single_dimension(
                                shared_range,
                                &prefix[derived.index],
                                derived.index,
                            )
                            .map_err(|e| e.to_string())?;
                        Ok(evals)
                    })
                    .collect::<Result<Vec<Vec<u128>>, String>>()
            })
            .collect::<Result<Vec<Vec<Vec<u128>>>, String>>()?;

        let heavy = self.batch_check(&evals, signal_dealer_channels, other_server_channels)?;
        let kept: Vec<Vec<Vec<bool>>> = leaves
            .into_iter()
            .zip(heavy)
            .filter(|(_, heavy)| *heavy)
            .map(|((prefix, _), _)| prefix)
            .collect();
        println!(
            "Derived coordinate check: {} heavy hitters in {:?}",
            kept.len(),
            start.elapsed()
        );
        Ok(kept)
    }

    fn batch_check(
        &self,
        current_eval: &[Vec<Vec<u128>>],
        signal_dealer_channels: &mut [CommTrackingChannel],
        other_server_channels: &mut [CommTrackingChannel],
    ) -> Result<Vec<bool>, String> {
        let num_threads = other_server_channels.len();
        println!(
            "Using {} threads for parallel processing {} data chunks",
            num_threads,
            current_eval.len()
        );
        println!(
            "batch_check: channels={}, rayon_current_num_threads={}",
            num_threads,
            rayon::current_num_threads()
        );

        // Calculate distance threshold once
        let distance_threshold = if self.metric() == DistanceMetric::LInfinity {
            get_distance_threshold(self.delta(), "Linf")
        } else {
            match self.metric() {
                DistanceMetric::Lp { p } => {
                    get_distance_threshold(self.delta(), &format!("L{}", p))
                }
                _ => return Err("Unsupported distance metric for threshold".to_string()),
            }
        };

        // Process prefix sets in parallel chunks
        let chunk_size = (current_eval.len() + num_threads - 1) / num_threads;
        let mut server_bits = vec![false; current_eval.len()];

        println!(
            "batch_check: channels={}, rayon_current_num_threads={}",
            num_threads,
            rayon::current_num_threads()
        );

        // Use rayon to process chunks in parallel
        server_bits
            .par_chunks_mut(chunk_size)
            .zip(current_eval.par_chunks(chunk_size))
            .zip(signal_dealer_channels.par_iter_mut())
            .zip(other_server_channels.par_iter_mut())
            .try_for_each(|(((result_chunk, eval_chunk), signal_dealer_channel), other_server_channel)| {
                // Use the corresponding dealer channels for this thread
                let mut local_rng = AesRng::new();
                let mut aggregated_counts = Vec::new();

                let start = std::time::Instant::now();
                // Process each evaluation chunk in this chunk using the parallel channels
                for eval in eval_chunk.iter() {
                    // Handle CheckData - get from dealer if using LpIntervalFSS, otherwise create locally
                    let check_data_list = match self.check_property() {
                        CheckProperty::Equality => self.batch_check_receive_equality_check_data(signal_dealer_channel)
                            .map_err(|e| format!("Failed to receive equality check data: {}", e))?,
                        CheckProperty::MuBounded => {
                            match self.check_method() {
                                CheckMethod::FSS => {
                                    // Request check FSS keys from dealer using the parallel check dealer channel
                                    let batch = request_dealer_check(signal_dealer_channel, 1u128 << self.h3())?;
                                    // println!("Received dealer check keys");

                                    if batch.keys.len() < self.num_clients() {
                                        return Err(format!("Dealer provided {} keys but {} are needed",
                                                         batch.keys.len(), self.num_clients()));
                                    }

                                    if batch.random_values.len() < self.num_clients() {
                                        return Err(format!("Dealer provided {} random values but {} are needed",
                                                         batch.random_values.len(), self.num_clients()));
                                    }

                                    // Create CheckData for each client share using corresponding FSS key
                                    let mut check_data_vec = Vec::new();
                                    for i in 0..self.num_clients() {
                                        check_data_vec.push(CheckData::LpIntervalFSS {
                                            fss_key: batch.keys[i].clone(),
                                            random_value: batch.random_values[i],
                                        });
                                    }
                                    check_data_vec
                                }
                                CheckMethod::GC => {
                                    vec![CheckData::LpGarbledCircuits { mu: distance_threshold }; self.num_clients()]
                                }
                            }
                        }
                    };

                    // Run batched check phase for all client shares with this prefix set
                    // This uses garbled circuits that communicate with the other server via the parallel channel

                    let match_results = self.check_phase.run_batch_fuzzy_match_check(
                        &eval,
                        &check_data_list,
                        other_server_channel,
                        &mut local_rng,
                    ).map_err(|e| format!("Batch check phase failed: {:?}", e))?;

                    let aggregated_result = self.threshold_phase.aggregate_match_results(&match_results).map_err(|e| format!("Failed to aggregate match results: {:?}", e))?;

                    aggregated_counts.push(aggregated_result);

                }

                println!("Time to process all prefixes and aggregate results: {:?}", start.elapsed());

                let start = std::time::Instant::now();
                let threshold_data_list = match self.threshold_method() {
                    ThresholdMethod::GC => {
                        // Use garbled circuits for threshold comparison
                        vec![ThresholdData::GarbledCircuits { t: self.match_threshold() }; aggregated_counts.len()]
                    }
                    ThresholdMethod::FSS => {
                        // Request threshold FSS keys from dealer using the signal dealer channel
                        let threshold_data_vec = (0..aggregated_counts.len()).map(|_| {
                            let batch = request_dealer_threshold(signal_dealer_channel, 2).unwrap();
                            if batch.keys.len() < 1 {
                                panic!("Dealer provided {} keys but {} are needed", batch.keys.len(), 1);
                            }

                            ThresholdData::IntervalFSS {
                                fss_key: batch.keys[0].clone(),
                                random_value: batch.random_values[0],
                            }
                        }).collect::<Vec<ThresholdData>>();
                        threshold_data_vec
                    }
                };

                println!("Time for initiating threshold data: {:?}", start.elapsed());

                // Run threshold phase to check if results exceed threshold
                // This also uses garbled circuits that communicate with the other server
                let start = std::time::Instant::now();
                let results_bool = self.threshold_phase.compare_with_threshold(
                    &aggregated_counts,
                    &threshold_data_list,
                    other_server_channel,
                    &mut local_rng,
                ).map_err(|e| format!("Threshold phase failed: {:?}", e))?;

                result_chunk.copy_from_slice(&results_bool);
                println!("Time for threshold comparison: {:?}", start.elapsed());

                Ok::<(), String>(())
            })
            .map_err(|e| format!("Parallel processing failed: {}", e))?;

        // After parallel processing, exchange the final results in parallel chunks
        let chunk_size = (server_bits.len() + num_threads - 1) / num_threads;
        let mut final_results = vec![false; server_bits.len()];

        // Exchange bits in parallel chunks using the other server channels
        let start = std::time::Instant::now();
        final_results
            .par_chunks_mut(chunk_size)
            .zip(server_bits.par_chunks(chunk_size))
            .zip(other_server_channels.par_iter_mut())
            .enumerate()
            .try_for_each(
                |(chunk_idx, ((result_chunk, bits_chunk), other_server_channel))| {
                    // Use the corresponding other server channel for this chunk
                    let bits_chunk_vec: Vec<bool> = bits_chunk.to_vec();

                    if self.role {
                        // Server 1: receive bits from server 0, then send our bits
                        let server0_bits = receive_bool_vec(other_server_channel).map_err(|e| {
                            format!(
                                "Failed to receive bits from server 0 in chunk {}: {:?}",
                                chunk_idx, e
                            )
                        })?;
                        send_bool_vec(other_server_channel, &bits_chunk_vec).map_err(|e| {
                            format!(
                                "Failed to send bits to server 0 in chunk {}: {:?}",
                                chunk_idx, e
                            )
                        })?;

                        if server0_bits.len() != bits_chunk_vec.len() {
                            return Err(format!(
                                "Mismatch in chunk {} size: expected {}, got {}",
                                chunk_idx,
                                bits_chunk_vec.len(),
                                server0_bits.len()
                            ));
                        }

                        for (i, (bit0, bit1)) in
                            server0_bits.iter().zip(bits_chunk_vec.iter()).enumerate()
                        {
                            result_chunk[i] = bit0 ^ bit1;
                        }
                    } else {
                        // Server 0: send our bits, then receive from server 1
                        send_bool_vec(other_server_channel, &bits_chunk_vec).map_err(|e| {
                            format!(
                                "Failed to send bits to server 1 in chunk {}: {:?}",
                                chunk_idx, e
                            )
                        })?;
                        let server1_bits = receive_bool_vec(other_server_channel).map_err(|e| {
                            format!(
                                "Failed to receive bits from server 1 in chunk {}: {:?}",
                                chunk_idx, e
                            )
                        })?;

                        if server1_bits.len() != bits_chunk_vec.len() {
                            return Err(format!(
                                "Mismatch in chunk {} size: expected {}, got {}",
                                chunk_idx,
                                bits_chunk_vec.len(),
                                server1_bits.len()
                            ));
                        }

                        for (i, (bit0, bit1)) in
                            bits_chunk_vec.iter().zip(server1_bits.iter()).enumerate()
                        {
                            result_chunk[i] = bit0 ^ bit1;
                        }
                    }

                    Ok::<(), String>(())
                },
            )
            .map_err(|e| format!("Parallel bit exchange failed: {}", e))?;
        println!("Time for parallel bit exchange: {:?}", start.elapsed());
        println!("Parallel processing completed successfully");
        Ok(final_results)
    }

    fn batch_check_receive_equality_check_data(
        &self,
        signal_dealer_channel: &mut CommTrackingChannel,
    ) -> Result<Vec<CheckData>> {
        match self.check_method() {
            CheckMethod::FSS => {
                let batch =
                    request_dealer_equality(signal_dealer_channel, 1u128 << self.h3())?;

                ensure!(
                    batch.keys.len() >= self.num_clients(),
                    "Dealer provided {} keys but {} are needed",
                    batch.keys.len(),
                    self.num_clients()
                );

                // Create CheckData for each client share using corresponding FSS key
                let mut check_data_vec = Vec::new();
                for i in 0..self.num_clients() {
                    check_data_vec.push(CheckData::LinfDpf {
                        fss_key: batch.keys[i].clone(),
                        random_value: batch.random_values[i].clone(),
                    });
                }
                Ok(check_data_vec)
            }
            CheckMethod::GC => {
                Ok(vec![CheckData::LinfGarbledCircuits; self.num_clients()])
            }
        }
    }
}

impl MosaicProtocol {
    // Heavily assumes that both parties synchronize on the barrett context
    fn send_modp_vec<'a>(
        &'a self,
        vals: &[Modp<'a>],
        channel: &mut CommTrackingChannel,
    ) -> Result<()> {
        let length: usize = vals.len();
        channel.write_bytes(&length.to_le_bytes())?;
        for (idx, val) in vals.iter().enumerate() {
            ensure!(val.context() == self.barrett_ctx(), "Barrett context mismatch at idx {}", idx);
            channel.write_bytes(&val.value().to_le_bytes())?;
        }
        channel.flush()?;
        Ok(())
    }

    // Heavily assumes that both parties synchronize on the barrett context
    fn recv_modp_vec<'a>(
        &'a self,
        channel: &mut CommTrackingChannel,
    ) -> Result<Vec<Modp<'a>>> {
        let mut length_bytes = vec![0u8; 8];
        channel.read_bytes(&mut length_bytes)?;
        let length = usize::from_le_bytes(length_bytes.try_into().unwrap());
        let mut res: Vec<Modp> = Vec::with_capacity(length);
        for _ in 0..length {
            let mut val_bytes = vec![0u8; 16];
            channel.read_bytes(&mut val_bytes)?;
            let val = u128::from_le_bytes(val_bytes.try_into().unwrap());
            res.push(Modp::new(self.barrett_ctx(), val));
        }
        Ok(res)
    }
}

impl MosaicProtocol {
    pub fn h1(&self) -> usize {
        self.share_phase.h1()
    }

    pub fn h2(&self) -> usize {
        self.share_phase.h2()
    }

    pub fn h3(&self) -> usize {
        self.check_phase.h3()
    }

    pub fn d(&self) -> usize {
        self.share_phase.d()
    }

    pub fn q(&self) -> u128 {
        self.sketch_phase.q()
    }

    pub fn delta(&self) -> u128 {
        self.share_phase.delta()
    }

    pub fn metric(&self) -> DistanceMetric {
        self.share_phase.metric()
    }

    pub fn num_clients(&self) -> usize {
        self.num_clients
    }

    pub fn role(&self) -> bool {
        self.role
    }

    pub fn match_threshold(&self) -> u128 {
        self.match_threshold
    }

    pub fn enable_sketch(&self) -> bool {
        self.enable_sketch
    }

    pub fn barrett_ctx<'a>(&'a self) -> &'a BarrettCtx {
        &self.sketch_phase.barrett_ctx()
    }

    pub fn share_method(&self) -> ShareMethod {
        self.share_phase.method()
    }

    pub fn check_method(&self) -> CheckMethod {
        self.check_phase.method()
    }

    pub fn check_property(&self) -> CheckProperty {
        self.check_phase.property()
    }

    pub fn threshold_method(&self) -> ThresholdMethod {
        self.threshold_phase.method()
    }
}

/// Each coordinate's evaluation of one client's key at the current prefix, as the check phase
/// takes them.
fn share_data_evals(share_data: ShareData) -> Vec<u128> {
    match share_data {
        ShareData::OKVS { eval } => eval,
        ShareData::IntervalFSS { data } => data.iter().map(|eval| eval.result()[0]).collect(),
        ShareData::DistanceFSS { eval, .. } => eval,
    }
}

/// Bits, most significant first, as a number.
fn bits_to_u128(bits: &[bool]) -> u128 {
    bits.iter().fold(0u128, |acc, &bit| (acc << 1) | u128::from(bit))
}

fn flatten_verify_values<'a>(
    verify_values: &VerifyValues<'a>,
) -> Vec<Modp<'a>> {
    match verify_values {
        VerifyValues::Dcf { last_layer, consistency } => {
            let mut flattened = Vec::with_capacity(1 + consistency.len());
            flattened.push(*last_layer);
            flattened.extend(consistency.into_iter());
            flattened
        },
        VerifyValues::DcfPayload { length, last_layer_consistency, consistency } => {
            let mut flattened = Vec::with_capacity(*length);
            flattened.extend(last_layer_consistency.into_iter());
            for consistency_vec in consistency.into_iter() {
                flattened.extend(consistency_vec.into_iter());
            }
            flattened
        },
        VerifyValues::Linf { ldcf, rdcf, consistency } => {
            let ldcf_flat = flatten_verify_values(&**ldcf);
            let rdcf_flat = flatten_verify_values(&**rdcf);
            let mut flattened = Vec::with_capacity(ldcf_flat.len() + rdcf_flat.len() + 1);
            flattened.extend(ldcf_flat);
            flattened.extend(rdcf_flat);
            flattened.push(*consistency);
            flattened
        },
        VerifyValues::Lp { p: _, ldcf0, ldcf1, rdcf0, rdcf1, reference_dpf } => {
            let ldcf0_flat = flatten_verify_values(&**ldcf0);
            let ldcf1_flat = flatten_verify_values(&**ldcf1);
            let rdcf0_flat = flatten_verify_values(&**rdcf0);
            let rdcf1_flat = flatten_verify_values(&**rdcf1);
            let mut flattened = Vec::with_capacity(
                ldcf0_flat.len() + ldcf1_flat.len() +
                rdcf0_flat.len() + rdcf1_flat.len() +
                1
            );
            flattened.extend(ldcf0_flat);
            flattened.extend(ldcf1_flat);
            flattened.extend(rdcf0_flat);
            flattened.extend(rdcf1_flat);
            flattened.push(*reference_dpf);
            flattened
        }
    }
}

/// Request FSS keys from dealer
pub fn request_dealer_check(
    signal_dealer_channel: &mut CommTrackingChannel,
    _modulus: u128,
) -> Result<SerializedFssKeyBatch, String> {
    // Send DealerSignal using custom serialization
    let signal = DealerSignal::RequestCheckKeys;
    let signal_bytes = signal.to_bytes();
    let len_bytes = (signal_bytes.len() as u64).to_le_bytes();
    signal_dealer_channel
        .write_bytes(&len_bytes)
        .map_err(|e| format!("Failed to write DealerSignal length: {}", e))?;
    signal_dealer_channel
        .write_bytes(&signal_bytes)
        .map_err(|e| format!("Failed to write DealerSignal: {}", e))?;
    signal_dealer_channel
        .flush()
        .map_err(|e| format!("Failed to flush DealerSignal: {}", e))?;

    // Receive FSS key batch from dealer
    let mut len_bytes = [0u8; 8];
    signal_dealer_channel
        .read_bytes(&mut len_bytes)
        .map_err(|e| format!("Failed to read key batch length: {}", e))?;
    let len = u64::from_le_bytes(len_bytes) as usize;

    let mut batch_data = vec![0u8; len];
    signal_dealer_channel
        .read_bytes(&mut batch_data)
        .map_err(|e| format!("Failed to read key batch data: {}", e))?;

    // Receive serialized check keys from dealer without deserializing the individual keys yet
    let (fss_key_batch, _) =
        SerializedFssKeyBatch::from_bytes(&batch_data)
            .expect("Failed to deserialize SerializedFssKeyBatch");
    Ok(fss_key_batch)
}

pub fn request_dealer_equality(
    signal_dealer_channel: &mut CommTrackingChannel,
    _modulus: u128,
) -> Result<SerializedDpfKeyBatch> {
    // Send DealerSignal using custom serialization
    let signal = DealerSignal::RequestEqualityKeys;
    let signal_bytes = signal.to_bytes();
    let len_bytes = (signal_bytes.len() as u64).to_le_bytes();
    signal_dealer_channel
        .write_bytes(&len_bytes)
        .map_err(|e| anyhow!("Failed to write DealerSignal length: {}", e))?;
    signal_dealer_channel
        .write_bytes(&signal_bytes)
        .map_err(|e| anyhow!("Failed to write DealerSignal: {}", e))?;
    signal_dealer_channel
        .flush()
        .map_err(|e| anyhow!("Failed to flush DealerSignal: {}", e))?;

    // Receive FSS key batch from dealer
    let mut len_bytes = [0u8; 8];
    signal_dealer_channel
        .read_bytes(&mut len_bytes)
        .map_err(|e| anyhow!("Failed to read key batch length: {}", e))?;
    let len = u64::from_le_bytes(len_bytes) as usize;

    let mut batch_data = vec![0u8; len];
    signal_dealer_channel
        .read_bytes(&mut batch_data)
        .map_err(|e| anyhow!("Failed to read key batch data: {}", e))?;

    let (fss_key_batch, _) = SerializedDpfKeyBatch::from_bytes(&batch_data)
        .map_err(|e| anyhow!("Failed to deserialize SerializedDpfKeyBatch for equality testing from dealer: {}", e))?;
    Ok(fss_key_batch)
}

pub fn request_dealer_threshold(
    signal_dealer_channel: &mut CommTrackingChannel,
    _modulus: u128,
) -> Result<SerializedFssKeyBatch, String> {
    // Send DealerSignal using custom serialization
    let signal = DealerSignal::RequestThresholdKeys;
    let signal_bytes = signal.to_bytes();
    let len_bytes = (signal_bytes.len() as u64).to_le_bytes();
    signal_dealer_channel
        .write_bytes(&len_bytes)
        .map_err(|e| format!("Failed to write DealerSignal length: {}", e))?;
    signal_dealer_channel
        .write_bytes(&signal_bytes)
        .map_err(|e| format!("Failed to write DealerSignal: {}", e))?;
    signal_dealer_channel
        .flush()
        .map_err(|e| format!("Failed to flush DealerSignal: {}", e))?;

    // Receive FSS key batch from dealer
    let mut len_bytes = [0u8; 8];
    signal_dealer_channel
        .read_bytes(&mut len_bytes)
        .map_err(|e| format!("Failed to read key batch length: {}", e))?;
    let len = u64::from_le_bytes(len_bytes) as usize;

    let mut batch_data = vec![0u8; len];
    signal_dealer_channel
        .read_bytes(&mut batch_data)
        .map_err(|e| format!("Failed to read key batch data: {}", e))?;

    let (fss_key_batch, _) =
        SerializedFssKeyBatch::from_bytes(&batch_data)
            .map_err(|e| format!("Failed to deserialize SerializedFssKeyBatch for threshold testing from dealer: {}", e))?;
    Ok(fss_key_batch)
}

/// Send shutdown signal to dealer
pub fn shutdown_dealer(dealer_channel: &mut CommTrackingChannel) -> Result<(), String> {
    // Send shutdown signal using custom serialization
    let signal_bytes = DealerSignal::Shutdown.to_bytes();
    let len_bytes = (signal_bytes.len() as u64).to_le_bytes();
    dealer_channel
        .write_bytes(&len_bytes)
        .map_err(|e| format!("Failed to write DealerSignal length: {}", e))?;
    dealer_channel
        .write_bytes(&signal_bytes)
        .map_err(|e| format!("Failed to write DealerSignal: {}", e))?;
    dealer_channel
        .flush()
        .map_err(|e| format!("Failed to flush DealerSignal: {}", e))?;
    Ok(())
}
