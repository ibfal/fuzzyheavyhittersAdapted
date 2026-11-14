//! FSS Dealer Implementation
//!
//! This module provides the trusted dealer functionality for generating FSS keys
//! for both the check phase and threshold phase of the fuzzy heavy hitters protocol.
//!
//! The dealer operates with a coordinator model where only server 0 sends key requests,
//! and the dealer responds by sending matching keys to both servers simultaneously.
//! This ensures both servers always have the same set of FSS keys.

use crate::{
    channel::CommTrackingChannel,
    data_structures::ringvec::RingVec,
    fss::{dpf::DpfKey, ldcf::LdcfKey, rdcf::RdcfKey},
    util::{bits_to_u8s, u128_to_bits_msb, u8s_to_bits},
};
use rand::RngExt;
use rayon::prelude::*;
use scuttlebutt::AbstractChannel;
use std::convert::TryInto;

/// Serialized DPF key batch for equality check phase.
pub type SerializedDpfKey = Vec<u8>;

#[derive(Clone, Debug)]
pub struct SerializedDpfKeyBatch {
    pub keys: Vec<SerializedDpfKey>,
    pub random_values: Vec<Vec<bool>>,
}

impl SerializedDpfKeyBatch {
    pub fn to_bytes(&self) -> Result<Vec<u8>, String> {
        let mut out = Vec::new();
        out.extend_from_slice(&(self.keys.len() as u32).to_le_bytes());
        for key in &self.keys {
            out.extend_from_slice(&(key.len() as u32).to_le_bytes());
            out.extend_from_slice(key);
        }
        out.extend_from_slice(&(self.random_values.len() as u32).to_le_bytes());
        out.extend_from_slice(&(self.random_values[0].len() as u32).to_le_bytes());
        for v in &self.random_values {
            out.extend_from_slice(&bits_to_u8s(v));
        }
        Ok(out)
    }

    pub fn from_bytes(bytes: &[u8]) -> Result<(Self, usize), String> {
        if bytes.len() < 4 {
            return Err("Too short for SerializedDpfKeyBatch keys len".to_string());
        }
        let mut offset = 0;
        let key_count = u32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap()) as usize;
        offset += 4;
        let mut keys = Vec::with_capacity(key_count);
        for _ in 0..key_count {
            if bytes[offset..].len() < 4 {
                return Err("Too short for serialized DPF key length".to_string());
            }
            let key_len = u32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap()) as usize;
            offset += 4;
            if bytes[offset..].len() < key_len {
                return Err("Too short for serialized DPF key bytes".to_string());
            }
            keys.push(bytes[offset..offset + key_len].to_vec());
            offset += key_len;
        }
        if bytes[offset..].len() < 8 {
            return Err("Too short for SerializedDpfKeyBatch random_values header".to_string());
        }
        let val_count = u32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap()) as usize;
        offset += 4;
        let val_length = u32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap()) as usize;
        offset += 4;
        let val_bytes_length = (val_length + 7) / 8;
        let mut random_values = Vec::with_capacity(val_count);
        for _ in 0..val_count {
            if bytes[offset..].len() < val_bytes_length {
                return Err("Too short for serialized DPF random value bytes".to_string());
            }
            random_values.push(u8s_to_bits(
                bytes[offset..offset + val_bytes_length].try_into().unwrap(),
                val_length,
            ));
            offset += val_bytes_length;
        }
        Ok((SerializedDpfKeyBatch { keys, random_values }, offset))
    }
}

/// FSS key batch for check phase - contains serialized keys for one server
pub type SerializedFssKey = (Vec<u8>, Vec<u8>);

#[derive(Clone, Debug)]
pub struct SerializedFssKeyBatch {
    pub keys: Vec<SerializedFssKey>,
    pub random_values: Vec<u128>,
}

impl SerializedFssKeyBatch {
    pub fn to_bytes(&self) -> Result<Vec<u8>, String> {
        let mut out = Vec::new();
        out.extend_from_slice(&(self.keys.len() as u32).to_le_bytes());
        for (k0_bytes, k1_bytes) in &self.keys {
            out.extend_from_slice(&(k0_bytes.len() as u32).to_le_bytes());
            out.extend_from_slice(k0_bytes);
            out.extend_from_slice(&(k1_bytes.len() as u32).to_le_bytes());
            out.extend_from_slice(k1_bytes);
        }
        out.extend_from_slice(&(self.random_values.len() as u32).to_le_bytes());
        for v in &self.random_values {
            out.extend_from_slice(&v.to_le_bytes());
        }
        Ok(out)
    }

    pub fn from_bytes(bytes: &[u8]) -> Result<(Self, usize), String> {
        if bytes.len() < 4 {
            return Err("Too short for SerializedFssKeyBatch keys len".to_string());
        }
        let mut offset = 0;
        let key_count = u32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap()) as usize;
        offset += 4;
        let mut keys = Vec::with_capacity(key_count);
        for _ in 0..key_count {
            if bytes[offset..].len() < 4 {
                return Err("Too short for serialized LDCF key length".to_string());
            }
            let k0_len = u32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap()) as usize;
            offset += 4;
            if bytes[offset..].len() < k0_len {
                return Err("Too short for serialized LDCF key bytes".to_string());
            }
            let k0_bytes = bytes[offset..offset + k0_len].to_vec();
            offset += k0_len;
            if bytes[offset..].len() < 4 {
                return Err("Too short for serialized RDCF key length".to_string());
            }
            let k1_len = u32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap()) as usize;
            offset += 4;
            if bytes[offset..].len() < k1_len {
                return Err("Too short for serialized RDCF key bytes".to_string());
            }
            let k1_bytes = bytes[offset..offset + k1_len].to_vec();
            offset += k1_len;
            keys.push((k0_bytes, k1_bytes));
        }
        if bytes[offset..].len() < 4 {
            return Err("Too short for SerializedFssKeyBatch random_values len".to_string());
        }
        let val_count = u32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap()) as usize;
        offset += 4;
        let mut random_values = Vec::with_capacity(val_count);
        for _ in 0..val_count {
            if bytes[offset..].len() < 16 {
                return Err("Too short for u128 in SerializedFssKeyBatch".to_string());
            }
            random_values.push(u128::from_le_bytes(
                bytes[offset..offset + 16].try_into().unwrap(),
            ));
            offset += 16;
        }
        Ok((
            SerializedFssKeyBatch {
                keys,
                random_values,
            },
            offset,
        ))
    }
}

/// FSS key batch for threshold/equality phases - contains deserialized keys
#[derive(Clone, Debug)]
pub struct FssKeyBatch {
    pub keys: Vec<(LdcfKey, RdcfKey)>,
    pub random_values: Vec<u128>,
}

impl FssKeyBatch {
    pub fn to_bytes(&self) -> Result<Vec<u8>, String> {
        let mut out = Vec::new();
        out.extend_from_slice(&(self.keys.len() as u32).to_le_bytes());
        for (k0, k1) in &self.keys {
            let k0_bytes = k0
                .to_bytes()
                .map_err(|e| format!("Failed to serialize LDCF key: {}", e))?;
            out.extend_from_slice(&k0_bytes);
            let k1_bytes = k1
                .to_bytes()
                .map_err(|e| format!("Failed to serialize RDCF key: {}", e))?;
            out.extend_from_slice(&k1_bytes);
        }
        out.extend_from_slice(&(self.random_values.len() as u32).to_le_bytes());
        for v in &self.random_values {
            out.extend_from_slice(&v.to_le_bytes());
        }
        Ok(out)
    }

    pub fn from_bytes(bytes: &[u8], modulus: u128) -> Result<(Self, usize), String> {
        if bytes.len() < 4 {
            return Err("Too short for FssKeyBatch keys len".to_string());
        }
        let mut offset = 0;
        let key_count = u32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap()) as usize;
        offset += 4;
        let mut keys = Vec::with_capacity(key_count);
        for _ in 0..key_count {
            let (k0, key_used) = LdcfKey::from_bytes(&bytes[offset..], modulus)
                .map_err(|e| format!("Failed to deserialize LDCF key: {}", e))?;
            offset += key_used;
            let (k1, key_used) = RdcfKey::from_bytes(&bytes[offset..], modulus)
                .map_err(|e| format!("Failed to deserialize RDCF key: {}", e))?;
            offset += key_used;
            keys.push((k0, k1));
        }
        if bytes[offset..].len() < 4 {
            return Err("Too short for FssKeyBatch random_values len".to_string());
        }
        let val_count = u32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap()) as usize;
        offset += 4;
        let mut random_values = Vec::with_capacity(val_count);
        for _ in 0..val_count {
            if bytes[offset..].len() < 16 {
                return Err("Too short for u128 in FssKeyBatch".to_string());
            }
            random_values.push(u128::from_le_bytes(
                bytes[offset..offset + 16].try_into().unwrap(),
            ));
            offset += 16;
        }
        Ok((
            FssKeyBatch {
                keys,
                random_values,
            },
            offset,
        ))
    }
}

pub struct DpfKeyBatch {
    pub keys: Vec<DpfKey>,
    pub random_values: Vec<Vec<bool>>,
}

impl DpfKeyBatch {
    pub fn to_bytes(&self) -> Result<Vec<u8>, String> {
        let mut out = Vec::new();
        out.extend_from_slice(&(self.keys.len() as u32).to_le_bytes());
        for k in &self.keys {
            let key_bytes = k
                .to_bytes()
                .map_err(|e| format!("Failed to serialize DPF key: {}", e))?;
            out.extend_from_slice(&key_bytes);
        }
        out.extend_from_slice(&(self.random_values.len() as u32).to_le_bytes());
        out.extend_from_slice(&(self.random_values[0].len() as u32).to_le_bytes());
        for v in &self.random_values {
            out.extend_from_slice(&bits_to_u8s(v));
        }
        Ok(out)
    }

    pub fn from_bytes(bytes: &[u8], modulus: u128) -> Result<(Self, usize), String> {
        if bytes.len() < 4 {
            return Err("Too short for DpfKeyBatch keys len".to_string());
        }
        let mut offset = 0;
        let key_count = u32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap()) as usize;
        offset += 4;
        let mut keys = Vec::with_capacity(key_count);
        for _ in 0..key_count {
            let (k, key_used) = DpfKey::from_bytes(&bytes[offset..], modulus)
                .map_err(|e| format!("Failed to deserialize DPF key: {}", e))?;
            offset += key_used;
            keys.push(k);
        }
        if bytes[offset..].len() < 4 {
            return Err("Too short for DpfKeyBatch random_values len".to_string());
        }
        let val_count = u32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap()) as usize;
        offset += 4;
        let val_length = u32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap()) as usize;
        offset += 4;
        let val_bytes_length = (val_length + 7) / 8;
        let mut random_values = Vec::with_capacity(val_count);
        for _ in 0..val_count {
            random_values.push(u8s_to_bits(
                bytes[offset..offset + val_bytes_length].try_into().unwrap(),
                val_length,
            ));
            offset += val_bytes_length;
        }
        Ok((
            DpfKeyBatch {
                keys,
                random_values,
            },
            offset,
        ))
    }
}

/// Signal sent from servers to the dealer
#[derive(Clone, Debug, PartialEq)]
pub enum DealerSignal {
    RequestEqualityKeys,
    RequestCheckKeys,
    RequestThresholdKeys,
    Shutdown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CheckKeyMode {
    Equality,
    MuBounded,
}

impl DealerSignal {
    pub fn to_bytes(&self) -> Vec<u8> {
        match self {
            DealerSignal::RequestEqualityKeys => vec![0u8],
            DealerSignal::RequestCheckKeys => vec![1u8],
            DealerSignal::RequestThresholdKeys => vec![2u8],
            DealerSignal::Shutdown => vec![3u8],
        }
    }

    pub fn from_bytes(bytes: &[u8]) -> Result<Self, String> {
        if bytes.is_empty() {
            return Err("Empty bytes for DealerSignal".to_string());
        }
        match bytes[0] {
            0 => Ok(DealerSignal::RequestEqualityKeys),
            1 => Ok(DealerSignal::RequestCheckKeys),
            2 => Ok(DealerSignal::RequestThresholdKeys),
            3 => Ok(DealerSignal::Shutdown),
            _ => Err("Unknown DealerSignal tag".to_string()),
        }
    }
}

/// The keys one dealer channel pair starts with (see `FssDealer::pregenerate_keys`).
pub struct PregeneratedKeys {
    equality_keys: Option<(Vec<SerializedDpfKey>, Vec<SerializedDpfKey>, Vec<(Vec<bool>, Vec<bool>)>)>,
    check_keys: Option<(Vec<SerializedFssKey>, Vec<SerializedFssKey>, Vec<(u128, u128)>)>,
    threshold_keys: (Vec<SerializedFssKey>, Vec<SerializedFssKey>, Vec<(u128, u128)>),
}

/// Networked FSS Dealer that communicates with servers over TCP
pub struct FssDealer {
    distance_threshold: u128,
    count_threshold: u128,
    check_input_bit_length: usize,
    check_output_bit_length: usize,
    n_clients: usize,
    dimensions: usize,
}

impl FssDealer {
    /// Create a new networked FSS dealer
    pub fn new(
        distance_threshold: u128,
        count_threshold: u128,
        check_input_bit_length: usize,
        check_output_bit_length: usize,
        n_clients: usize,
        dimensions: usize,
    ) -> Self {
        FssDealer {
            distance_threshold,
            count_threshold,
            check_input_bit_length,
            check_output_bit_length,
            n_clients,
            dimensions,
        }
    }

    /// The keys one channel pair starts with, generated before its first request. This is what
    /// each channel's thread of `run_dealer_parallel` generates before it serves requests.
    pub fn pregenerate_keys(&self, check_key_mode: CheckKeyMode) -> Result<PregeneratedKeys, String> {
        let mut equality_keys: Option<(
            Vec<SerializedDpfKey>,
            Vec<SerializedDpfKey>,
            Vec<(Vec<bool>, Vec<bool>)>,
        )> = None;
        let mut check_keys: Option<(
            Vec<SerializedFssKey>,
            Vec<SerializedFssKey>,
            Vec<(u128, u128)>,
        )> = None;
        match check_key_mode {
            CheckKeyMode::Equality => {
                equality_keys = Some(
                    self.generate_fss_keys_for_equality()
                        .map_err(|e| format!("Failed to pre-generate equality keys: {}", e))?,
                );
            }
            CheckKeyMode::MuBounded => {
                check_keys = Some(
                    self.generate_fss_keys_for_check()
                        .map_err(|e| format!("Failed to pre-generate check keys: {}", e))?,
                );
            }
        }
        let threshold_keys = self.generate_fss_keys_for_threshold()
            .map_err(|e| format!("Failed to pre-generate threshold keys: {}", e))?;
        Ok(PregeneratedKeys { equality_keys, check_keys, threshold_keys })
    }

    /// Run the dealer with multiple channels - handle key requests from multiple parallel channels
    pub fn run_dealer_parallel(
        &self,
        signal_channels_server0: &mut [CommTrackingChannel],
        signal_channels_server1: &mut [CommTrackingChannel],
        check_key_mode: CheckKeyMode,
    ) -> Result<(), String> {
        self.run_dealer_parallel_with_keys(
            signal_channels_server0,
            signal_channels_server1,
            check_key_mode,
            None,
        )
    }

    /// `run_dealer_parallel` with the starting keys of channel pair `i` taken from
    /// `pregenerated[i]` instead of being generated on the channel's thread. With `None` it is
    /// `run_dealer_parallel`. The benchmark runner uses `Some`: the dealer generates the keys
    /// before it connects, and the runner waits for that before it starts the servers and the
    /// client, so the key generation does not run during their measured phases.
    pub fn run_dealer_parallel_with_keys(
        &self,
        signal_channels_server0: &mut [CommTrackingChannel],
        signal_channels_server1: &mut [CommTrackingChannel],
        check_key_mode: CheckKeyMode,
        pregenerated: Option<Vec<PregeneratedKeys>>,
    ) -> Result<(), String> {
        println!(
            "FSS Dealer starting with {} channels per server...",
            signal_channels_server0.len()
        );

        let mut pregenerated: Vec<Option<PregeneratedKeys>> = match pregenerated {
            Some(keys) => {
                if keys.len() != signal_channels_server0.len() {
                    return Err(format!(
                        "{} pre-generated key sets for {} channels",
                        keys.len(),
                        signal_channels_server0.len()
                    ));
                }
                keys.into_iter().map(Some).collect()
            }
            None => (0..signal_channels_server0.len()).map(|_| None).collect(),
        };

        println!("Dealer ready");

        // Process each channel pair in parallel - each gets its own persistent thread
        signal_channels_server0
            .par_iter_mut()
            .zip(signal_channels_server1.par_iter_mut())
            .zip(pregenerated.par_iter_mut())
            .enumerate()
            .try_for_each(|(channel_idx, ((signal_channel_server0, signal_channel_server1), pregenerated))| {
                let PregeneratedKeys { mut equality_keys, mut check_keys, mut threshold_keys } =
                    match pregenerated.take() {
                        Some(keys) => keys,
                        None => self.pregenerate_keys(check_key_mode)?,
                    };
                // Each channel pair runs in its own persistent loop
                loop {
                    let server0_signal = self.read_dealer_signal(&mut *signal_channel_server0);
                    let server1_signal = self.read_dealer_signal(&mut *signal_channel_server1);
                    assert_eq!(server0_signal.clone().unwrap(), server1_signal.unwrap(),
                        "Signals from both servers should match");

                    if let Ok(signal) = server0_signal.clone() {
                        println!("Received signal: {:?}", signal);
                    }

                    // Check if there's a signal waiting on the server0 channel
                    match server0_signal {
                        Ok(signal) => {
                            match signal {
                                DealerSignal::RequestEqualityKeys => {
                                    let mut current_keys = equality_keys
                                        .take()
                                        .ok_or_else(|| {
                                            format!(
                                                "Received equality key request on channel {} but dealer was configured for {:?}",
                                                channel_idx, check_key_mode
                                            )
                                        })?;

                                    let batch_server0 = SerializedDpfKeyBatch {
                                        keys: std::mem::take(&mut current_keys.0),
                                        random_values: current_keys.2.iter().map(|(r0, _)| r0.clone()).collect(),
                                    };

                                    let batch_server1 = SerializedDpfKeyBatch {
                                        keys: std::mem::take(&mut current_keys.1),
                                        random_values: current_keys.2.iter().map(|(_, r1)| r1.clone()).collect(),
                                    };

                                    self.write_equality_key_batch(&mut *signal_channel_server0, &batch_server0)
                                        .map_err(|e| format!("Failed to send keys to server 0 on channel {}: {}", channel_idx, e))?;

                                    self.write_equality_key_batch(&mut *signal_channel_server1, &batch_server1)
                                        .map_err(|e| format!("Failed to send keys to server 1 on channel {}: {}", channel_idx, e))?;

                                    self.fill_fss_keys_for_equality(
                                        &mut current_keys.0,
                                        &mut current_keys.1,
                                        &mut current_keys.2,
                                    ).map_err(|e| format!("Failed to generate next equality keys: {}", e))?;
                                    equality_keys = Some(current_keys);
                                }
                                DealerSignal::RequestCheckKeys => {
                                    let mut current_keys = check_keys
                                        .take()
                                        .ok_or_else(|| {
                                            format!(
                                                "Received check key request on channel {} but dealer was configured for {:?}",
                                                channel_idx, check_key_mode
                                            )
                                        })?;

                                    let batch_server0 = SerializedFssKeyBatch {
                                        keys: std::mem::take(&mut current_keys.0),
                                        random_values: current_keys.2.iter().map(|(r0, _)| *r0).collect(),
                                    };

                                    let batch_server1 = SerializedFssKeyBatch {
                                        keys: std::mem::take(&mut current_keys.1),
                                        random_values: current_keys.2.iter().map(|(_, r1)| *r1).collect(),
                                    };

                                    self.write_check_key_batch(&mut *signal_channel_server0, &batch_server0)
                                        .map_err(|e| format!("Failed to send keys to server 0 on channel {}: {}", channel_idx, e))?;

                                    self.write_check_key_batch(&mut *signal_channel_server1, &batch_server1)
                                        .map_err(|e| format!("Failed to send keys to server 1 on channel {}: {}", channel_idx, e))?;

                                    self.fill_fss_keys_for_check(
                                        &mut current_keys.0,
                                        &mut current_keys.1,
                                        &mut current_keys.2,
                                    ).map_err(|e| format!("Failed to generate next check keys: {}", e))?;
                                    check_keys = Some(current_keys);
                                }
                                DealerSignal::RequestThresholdKeys => {
                                    let current_keys = std::mem::take(&mut threshold_keys);
                                    let (keys0, keys1, random_pairs) = current_keys;

                                    let batch_server0 = SerializedFssKeyBatch {
                                        keys: keys0,
                                        random_values: random_pairs.iter().map(|(r0, _)| *r0).collect(),
                                    };

                                    let batch_server1 = SerializedFssKeyBatch {
                                        keys: keys1,
                                        random_values: random_pairs.iter().map(|(_, r1)| *r1).collect(),
                                    };

                                    self.write_threshold_key_batch(&mut *signal_channel_server0, &batch_server0)
                                        .map_err(|e| format!("Failed to send threshold keys to server 0 on channel {}: {}", channel_idx, e))?;

                                    self.write_threshold_key_batch(&mut *signal_channel_server1, &batch_server1)
                                        .map_err(|e| format!("Failed to send threshold keys to server 1 on channel {}: {}", channel_idx, e))?;

                                    self.fill_fss_keys_for_threshold(
                                        &mut threshold_keys.0,
                                        &mut threshold_keys.1,
                                        &mut threshold_keys.2,
                                    ).map_err(|e| format!("Failed to generate next threshold keys: {}", e))?;
                                }
                                DealerSignal::Shutdown => {
                                    println!("Dealer: Received shutdown signal on channel {}, terminating this thread", channel_idx);
                                    break;
                                }
                            }
                        }
                        Err(e) => {
                            println!("Dealer: Error reading from server 0 channel {}: {}, terminating thread", channel_idx, e);
                            break;
                        }
                    }
                    // No need for sleep since read_dealer_signal is blocking
                }

                Ok::<(), String>(())
            })
            .map_err(|e| format!("Parallel dealer processing failed: {}", e))?;

        println!("All dealer channels terminated");
        Ok(())
    }

    /// Read a dealer signal from the channel
    pub fn read_dealer_signal(
        &self,
        channel: &mut CommTrackingChannel,
    ) -> Result<DealerSignal, String> {
        // Read the length first
        let mut len_bytes = [0u8; 8];
        channel
            .read_bytes(&mut len_bytes)
            .map_err(|e| format!("Failed to read length: {}", e))?;
        let len = u64::from_le_bytes(len_bytes) as usize;
        // Read the data
        let mut data = vec![0u8; len];
        channel
            .read_bytes(&mut data)
            .map_err(|e| format!("Failed to read data: {}", e))?;
        DealerSignal::from_bytes(&data)
    }

    pub fn write_equality_key_batch(
        &self,
        channel: &mut CommTrackingChannel,
        batch: &SerializedDpfKeyBatch,
    ) -> Result<(), String> {
        let data = batch
            .to_bytes()
            .map_err(|e| format!("Failed to serialize DPF key batch: {}", e))?;
        let len_bytes = (data.len() as u64).to_le_bytes();
        println!(
            "Writing DPF key batch of size {} bytes to channel",
            data.len()
        );
        channel
            .write_bytes(&len_bytes)
            .map_err(|e| format!("Failed to write length: {}", e))?;
        channel
            .write_bytes(&data)
            .map_err(|e| format!("Failed to write data: {}", e))?;
        channel
            .flush()
            .map_err(|e| format!("Failed to flush: {}", e))?;
        Ok(())
    }

    /// Write an FSS key batch to the channel
    pub fn write_check_key_batch(
        &self,
        channel: &mut CommTrackingChannel,
        batch: &SerializedFssKeyBatch,
    ) -> Result<(), String> {
        let data = batch.to_bytes()?;
        let len_bytes = (data.len() as u64).to_le_bytes();
        channel
            .write_bytes(&len_bytes)
            .map_err(|e| format!("Failed to write length: {}", e))?;
        channel
            .write_bytes(&data)
            .map_err(|e| format!("Failed to write data: {}", e))?;
        channel
            .flush()
            .map_err(|e| format!("Failed to flush: {}", e))?;
        Ok(())
    }

    pub fn write_threshold_key_batch(
        &self,
        channel: &mut CommTrackingChannel,
        batch: &SerializedFssKeyBatch,
    ) -> Result<(), String> {
        let data = batch.to_bytes()?;
        let len_bytes = (data.len() as u64).to_le_bytes();
        // println!("Writing Threshold key batch of size {} bytes to channel", data.len());
        channel
            .write_bytes(&len_bytes)
            .map_err(|e| format!("Failed to write length: {}", e))?;
        channel
            .write_bytes(&data)
            .map_err(|e| format!("Failed to write data: {}", e))?;
        channel
            .flush()
            .map_err(|e| format!("Failed to flush: {}", e))?;
        Ok(())
    }

    fn fill_fss_keys_for_equality(
        &self,
        key0s: &mut Vec<SerializedDpfKey>,
        key1s: &mut Vec<SerializedDpfKey>,
        random_pairs: &mut Vec<(Vec<bool>, Vec<bool>)>,
    ) -> Result<(), String> {
        let out_modulus = 1u128 << self.check_output_bit_length;

        random_pairs.clear();
        random_pairs.resize(self.n_clients, (Vec::new(), Vec::new()));
        for idx in 0..self.n_clients {
            let r0 = (0..self.check_input_bit_length * self.dimensions)
                .map(|_| rand::rng().random::<bool>())
                .collect::<Vec<_>>();
            let r1 = (0..self.check_input_bit_length * self.dimensions)
                .map(|_| rand::rng().random::<bool>())
                .collect::<Vec<_>>();
            random_pairs[idx] = (r0, r1);
        }

        key0s.clear();
        key1s.clear();
        key0s.resize_with(self.n_clients, Vec::new);
        key1s.resize_with(self.n_clients, Vec::new);

        for (idx, (r0, r1)) in random_pairs.iter().enumerate() {
            let sum_bits = r0
                .iter()
                .zip(r1.iter())
                .map(|(&b0, &b1)| b0 ^ b1)
                .collect::<Vec<_>>();
            let one = RingVec::new(vec![1], out_modulus)
                .map_err(|e| format!("Failed to build one payload: {}", e))?;
            let zero = RingVec::new(vec![0], out_modulus)
                .map_err(|e| format!("Failed to build zero payload: {}", e))?;
            let (key0, key1) = DpfKey::gen_dpf_key(&sum_bits, &one, &zero, out_modulus)
                .map_err(|e| format!("Failed to generate DPF key: {}", e))?;
            key0s[idx] = key0.to_bytes().map_err(|e| format!("Failed to serialize DPF key: {}", e))?;
            key1s[idx] = key1.to_bytes().map_err(|e| format!("Failed to serialize DPF key: {}", e))?;
        }

        Ok(())
    }

    pub fn generate_fss_keys_for_equality(
        &self,
    ) -> Result<(Vec<SerializedDpfKey>, Vec<SerializedDpfKey>, Vec<(Vec<bool>, Vec<bool>)>), String> {
        let mut key0s = Vec::new();
        let mut key1s = Vec::new();
        let mut random_pairs = Vec::new();
        self.fill_fss_keys_for_equality(&mut key0s, &mut key1s, &mut random_pairs)?;
        Ok((key0s, key1s, random_pairs))
    }

    /// Fill preallocated buffers with FSS keys for check phase comparison (Lp distance with IntervalFSS)
    fn fill_fss_keys_for_check(
        &self,
        key0s: &mut Vec<SerializedFssKey>,
        key1s: &mut Vec<SerializedFssKey>,
        random_pairs: &mut Vec<(u128, u128)>,
    ) -> Result<(), String> {
        let in_modulus = 1u128 << self.check_input_bit_length;
        let out_modulus = 1u128 << self.check_output_bit_length;

        random_pairs.clear();
        random_pairs.resize(self.n_clients, (0u128, 0u128));
        for idx in 0..self.n_clients {
            let mut std_rng = rand::rng();
            random_pairs[idx] = (
                std_rng.random_range(0..in_modulus),
                std_rng.random_range(0..in_modulus),
            );
        }

        key0s.clear();
        key1s.clear();
        key0s.resize_with(self.n_clients, || (Vec::new(), Vec::new()));
        key1s.resize_with(self.n_clients, || (Vec::new(), Vec::new()));

        for (idx, &(r0, r1)) in random_pairs.iter().enumerate() {
            let sum = self.distance_threshold + (r0 + r1) % in_modulus;
            let wraps_around = sum >= in_modulus;
            let zero_payload = RingVec::new(vec![0], out_modulus).expect("Failed to create zero payload");
            let one_payload = RingVec::new(vec![1], out_modulus).expect("Failed to create one payload");
            if wraps_around {
                let interval_start = (sum + 1) % in_modulus;
                let interval_end = (r0 + r1 - 1) % in_modulus;
                let alpha_bits = u128_to_bits_msb(interval_start, self.check_input_bit_length);
                let beta_bits = u128_to_bits_msb(interval_end, self.check_input_bit_length);
                let (key00, key10) = LdcfKey::gen_ldcf_key(
                    &alpha_bits,
                    &one_payload,
                    &zero_payload,
                    out_modulus,
                ).map_err(|e| e.to_string())?;
                let (key01, key11) = RdcfKey::gen_rdcf_key(
                    &beta_bits,
                    &zero_payload,
                    &one_payload,
                    out_modulus,
                ).map_err(|e| e.to_string())?;
                key0s[idx] = (
                    key00.to_bytes().map_err(|e| e.to_string())?,
                    key01.to_bytes().map_err(|e| e.to_string())?,
                );
                key1s[idx] = (
                    key10.to_bytes().map_err(|e| e.to_string())?,
                    key11.to_bytes().map_err(|e| e.to_string())?,
                );
            } else {
                let interval_start = (r0 + r1) % in_modulus;
                let interval_end = sum;
                let alpha_bits = u128_to_bits_msb(interval_start, self.check_input_bit_length);
                let beta_bits = u128_to_bits_msb(interval_end, self.check_input_bit_length);
                let (key00, key10) = LdcfKey::gen_ldcf_key(
                    &alpha_bits,
                    &zero_payload,
                    &one_payload,
                    out_modulus,
                ).map_err(|e| e.to_string())?;
                let (key01, key11) = RdcfKey::gen_rdcf_key(
                    &beta_bits,
                    &zero_payload,
                    &(zero_payload.clone() - one_payload.clone()),
                    out_modulus,
                ).map_err(|e| e.to_string())?;
                key0s[idx] = (
                    key00.to_bytes().map_err(|e| e.to_string())?,
                    key01.to_bytes().map_err(|e| e.to_string())?,
                );
                key1s[idx] = (
                    key10.to_bytes().map_err(|e| e.to_string())?,
                    key11.to_bytes().map_err(|e| e.to_string())?,
                );
            }
        }

        Ok(())
    }

    /// Generate FSS keys for check phase comparison (Lp distance with IntervalFSS)
    /// This simulates a trusted dealer generating FSS keys for distance threshold comparison
    pub fn generate_fss_keys_for_check(
        &self,
    ) -> Result<
        (
            Vec<SerializedFssKey>,
            Vec<SerializedFssKey>,
            Vec<(u128, u128)>,
        ),
        String,
    > {
        let mut key0s = Vec::new();
        let mut key1s = Vec::new();
        let mut random_pairs = Vec::new();
        self.fill_fss_keys_for_check(&mut key0s, &mut key1s, &mut random_pairs)?;
        Ok((key0s, key1s, random_pairs))
    }

    fn fill_fss_keys_for_threshold(
        &self,
        key0s: &mut Vec<SerializedFssKey>,
        key1s: &mut Vec<SerializedFssKey>,
        random_pairs: &mut Vec<(u128, u128)>,
    ) -> Result<(), String> {
        let modulus = 1u128 << self.check_output_bit_length;

        random_pairs.clear();
        random_pairs.resize(1, (0u128, 0u128));
        let mut std_rng = rand::rng();
        random_pairs[0] = (
            std_rng.random_range(0..modulus),
            std_rng.random_range(0..modulus),
        );

        key0s.clear();
        key1s.clear();
        key0s.resize_with(1, || (Vec::new(), Vec::new()));
        key1s.resize_with(1, || (Vec::new(), Vec::new()));

        let (r0, r1) = random_pairs[0];
        let zero_payload = RingVec::new(vec![0], 2).expect("Failed to create zero payload");
        let one_payload = RingVec::new(vec![1], 2).expect("Failed to create one payload");
        let sum = self.count_threshold + (r0 + r1) % modulus;
        if sum >= modulus {
            let interval_start = sum % modulus;
            let interval_end = (r0 + r1) % modulus;
            let alpha_bits = u128_to_bits_msb(interval_start, self.check_output_bit_length);
            let beta_bits = u128_to_bits_msb(interval_end, self.check_output_bit_length);
            let (key00, key10) = LdcfKey::gen_ldcf_key(&alpha_bits, &zero_payload, &one_payload, 2)
                .map_err(|e| e.to_string())?;
            let (key01, key11) = RdcfKey::gen_rdcf_key(
                &beta_bits,
                &zero_payload,
                &(zero_payload.clone() - one_payload.clone()),
                2,
            )
            .map_err(|e| e.to_string())?;
            key0s[0] = (
                key00.to_bytes().map_err(|e| e.to_string())?,
                key01.to_bytes().map_err(|e| e.to_string())?,
            );
            key1s[0] = (
                key10.to_bytes().map_err(|e| e.to_string())?,
                key11.to_bytes().map_err(|e| e.to_string())?,
            );
        } else {
            let interval_start = (r0 + r1) % modulus;
            let interval_end = (sum + modulus - 1) % modulus;
            let alpha_bits = u128_to_bits_msb(interval_start, self.check_output_bit_length);
            let beta_bits = u128_to_bits_msb(interval_end, self.check_output_bit_length);
            let (key00, key10) = LdcfKey::gen_ldcf_key(&alpha_bits, &one_payload, &zero_payload, 2)
                .map_err(|e| e.to_string())?;
            let (key01, key11) = RdcfKey::gen_rdcf_key(&beta_bits, &zero_payload, &one_payload, 2)
                .map_err(|e| e.to_string())?;
            key0s[0] = (
                key00.to_bytes().map_err(|e| e.to_string())?,
                key01.to_bytes().map_err(|e| e.to_string())?,
            );
            key1s[0] = (
                key10.to_bytes().map_err(|e| e.to_string())?,
                key11.to_bytes().map_err(|e| e.to_string())?,
            );
        }

        Ok(())
    }

    /// Generate FSS keys for interval FSS threshold comparison
    /// This simulates a trusted dealer generating FSS keys
    pub fn generate_fss_keys_for_threshold(
        &self,
    ) -> Result<
        (
            Vec<SerializedFssKey>,
            Vec<SerializedFssKey>,
            Vec<(u128, u128)>,
        ),
        String,
    > {
        let mut key0s = Vec::new();
        let mut key1s = Vec::new();
        let mut random_pairs = Vec::new();
        self.fill_fss_keys_for_threshold(&mut key0s, &mut key1s, &mut random_pairs)?;
        Ok((key0s, key1s, random_pairs))
    }
}
