use crate::{
    channel::CommTrackingChannel,
    data_structures::mod2k::Mod2k,
    fss::{ldcf::LdcfKey, rdcf::RdcfKey},
    garbled_circuits::greater_than_or_equal_threshold::{
        multiple_ev_greater_than_ss, multiple_gb_greater_than_ss,
    },
    fuzzy_match::{
        dealer::SerializedFssKey,
        threshold_phase_types::{ThresholdConfig, ThresholdData, ThresholdMethod, ThresholdPhaseError},
    },
    util::u128_to_bits_msb,
};
use scuttlebutt::{AbstractChannel, AesRng};

/// Threshold phase handler
#[derive(Debug, Clone)]
pub struct ThresholdPhase {
    config: ThresholdConfig,
    role: bool,
}

impl ThresholdPhase {
    /// Create a new threshold phase with the given configuration
    pub fn new<C: Into<ThresholdConfig>>(config: C, role: bool) -> Self {
        Self { 
            config: config.into(),
            role
        }
    }

    pub fn aggregate_match_results(
        &self,
        match_results: &[Mod2k],
    ) -> Result<Mod2k, ThresholdPhaseError> {
        let modulus = 1u128 << self.config.h3;
        let mut aggregated_share = Mod2k::new(0, modulus);
        for result in match_results {
            aggregated_share = aggregated_share + *result;
        }
        Ok(aggregated_share)
    }

    /// Aggregate match results and compare with threshold using garbled circuits
    ///
    /// This method:
    /// 1. Takes ring shares from the check phase for all clients: b^1, b^2, ..., b^n
    /// 2. Aggregates them: sum = b^1 + b^2 + ... + b^n (number of clients that "match")
    /// 3. Compares aggregated sum with threshold using garbled circuits
    /// 4. Returns whether matches exceed threshold
    pub fn compare_with_threshold_gc(
        &self,
        match_results: &[Mod2k],
        threshold: Mod2k,
        channel: &mut CommTrackingChannel,
        rng: &mut AesRng,
    ) -> Result<Vec<bool>, ThresholdPhaseError> {
        // Step 2: Compare aggregated share with threshold using garbled circuits
        // Both aggregated_share and threshold are already ModInt, so we can use them directly
        let comparison_result = if self.role {
            multiple_gb_greater_than_ss(rng, channel, match_results, &threshold)
        } else {
            // Evaluator side - gets the actual comparison result
            multiple_ev_greater_than_ss(rng, channel, match_results)
        };
        crate::bench::turns(crate::bench::GC_TURNS);

        Ok(comparison_result)
    }

    /// Compare with threshold using IntervalFSS approach
    ///
    /// This method:
    /// 1. Takes ring shares from the check phase for all clients: b^1, b^2, ..., b^n
    /// 2. Aggregates them: sum = b^1 + b^2 + ... + b^n (number of clients that "match")
    /// 3. Each server adds their random value to their aggregated share and sends to the other server
    /// 4. Each server evaluates the reconstructed count using their FSS key for interval [threshold + r0 + r1, MAX]
    /// 5. Returns the FSS evaluation result (1 if count >= threshold, 0 otherwise)
    pub fn compare_with_threshold_intervalfss(
        &self,
        match_results: &[Mod2k],
        random_values: &[Mod2k],
        fss_keys: &[SerializedFssKey],
        channel: &mut CommTrackingChannel,
    ) -> Result<Vec<bool>, ThresholdPhaseError> {
        let masked_values = match_results
            .iter()
            .zip(random_values.iter())
            .map(|(&input, &random_value)| input + random_value)
            .collect::<Vec<Mod2k>>();

        let combined_masked_values = if self.role {
            // Garbler: send all masked values, then read all counterpart masked values
            for masked_eval in &masked_values {
                let eval_bytes = masked_eval.val().to_le_bytes();
                channel.write_bytes(&eval_bytes).map_err(|e| {
                    ThresholdPhaseError::ChannelError(format!("Failed to send masked evals: {}", e))
                })?;
            }
            channel.flush().map_err(|e| {
                ThresholdPhaseError::ChannelError(format!("Failed to flush after sending: {}", e))
            })?;

            let mut combined = Vec::with_capacity(masked_values.len());
            for &masked_value in &masked_values {
                let mut received_bytes = [0u8; 16];
                channel.read_bytes(&mut received_bytes).map_err(|e| {
                    ThresholdPhaseError::ChannelError(format!(
                        "Failed to read other masked evals: {}",
                        e
                    ))
                })?;
                let other_masked_value =
                    Mod2k::new(u128::from_le_bytes(received_bytes), 1u128 << self.config.h3);
                combined.push(masked_value + other_masked_value);
            }
            combined
        } else {
            // Evaluator: read all counterpart masked values, then send all our masked values
            let mut combined = Vec::with_capacity(masked_values.len());
            for &masked_value in &masked_values {
                let mut received_bytes = [0u8; 16];
                channel.read_bytes(&mut received_bytes).map_err(|e| {
                    ThresholdPhaseError::ChannelError(format!(
                        "Failed to read other masked evals: {}",
                        e
                    ))
                })?;
                let other_masked_value =
                    Mod2k::new(u128::from_le_bytes(received_bytes), 1u128 << self.config.h3);
                combined.push(masked_value + other_masked_value);
            }

            for masked_eval in &masked_values {
                let eval_bytes = masked_eval.val().to_le_bytes();
                channel.write_bytes(&eval_bytes).map_err(|e| {
                    ThresholdPhaseError::ChannelError(format!("Failed to send masked evals: {}", e))
                })?;
            }
            channel.flush().map_err(|e| {
                ThresholdPhaseError::ChannelError(format!("Failed to flush after sending: {}", e))
            })?;

            combined
        };
        crate::bench::turns(1); // the masked-value exchange: server 1 sends first, then waits

        let out_modulus = 2;
        let threshold_exceeded = combined_masked_values
            .iter()
            .zip(fss_keys.iter())
            .map(|(masked_value, (fss_key0_bytes, fss_key1_bytes))| -> Result<bool, ThresholdPhaseError> {
                let (fss_key0, _) = LdcfKey::from_bytes(fss_key0_bytes, out_modulus)
                    .map_err(|e| ThresholdPhaseError::ConversionError(format!("Failed to deserialize LDCF key: {}", e)))?;
                let (fss_key1, _) = RdcfKey::from_bytes(fss_key1_bytes, out_modulus)
                    .map_err(|e| ThresholdPhaseError::ConversionError(format!("Failed to deserialize RDCF key: {}", e)))?;
                let masked_value_bits = u128_to_bits_msb(masked_value.val(), self.config.h3);
                let fss_result = fss_key0
                    .eval_ldcf(&masked_value_bits, out_modulus)
                    .map_err(|e| ThresholdPhaseError::ConversionError(e.to_string()))?;
                let fss_rdcf = fss_key1
                    .eval_rdcf(&masked_value_bits, out_modulus)
                    .map_err(|e| ThresholdPhaseError::ConversionError(e.to_string()))?;
                Ok((fss_result[0] + fss_rdcf[0]) % out_modulus == 1)
            })
            .collect::<Result<Vec<bool>, ThresholdPhaseError>>()?;

        Ok(threshold_exceeded)
    }

    /// Common function to compare with threshold using the configured method
    ///
    /// This function dispatches to either garbled circuits or IntervalFSS based on the config
    pub fn compare_with_threshold(
        &self,
        aggregated_results: &[Mod2k],
        threshold_data_list: &[ThresholdData],
        channel: &mut CommTrackingChannel,
        rng: &mut AesRng,
    ) -> Result<Vec<bool>, ThresholdPhaseError> {
        match self.config.method {
            ThresholdMethod::GC => {
                let mut current_t = Mod2k::zero(1u128 << self.config.h3);
                for (i, threshold_data) in threshold_data_list.iter().enumerate() {
                    match threshold_data {
                        ThresholdData::GarbledCircuits { t } => {
                            if i != 0 {
                                if *t != current_t.val() {
                                    return Err(ThresholdPhaseError::InvalidConfig(
                                        "All GarbledCircuits thresholds must be the same"
                                            .to_string(),
                                    ));
                                }
                            } else {
                                current_t = Mod2k::new(*t, 1u128 << self.config.h3);
                            }
                        }
                        _ => {
                            return Err(ThresholdPhaseError::InvalidConfig(
                                "Garbled circuits method requires GarbledCircuits data".to_string(),
                            ))
                        }
                    }
                }
                self.compare_with_threshold_gc(aggregated_results, current_t, channel, rng)
            }
            ThresholdMethod::FSS => {
                let mut fss_keys = Vec::new();
                let mut random_values = Vec::new();
                for threshold_data in threshold_data_list {
                    match threshold_data {
                        ThresholdData::IntervalFSS {
                            fss_key,
                            random_value,
                        } => {
                            fss_keys.push(fss_key.clone());
                            random_values.push(Mod2k::new(*random_value, 1u128 << self.config.h3));
                        }
                        _ => {
                            return Err(ThresholdPhaseError::InvalidConfig(
                                "FSS method requires FSS data with FSS key and random value"
                                    .to_string(),
                            ))
                        }
                    }
                }
                self.compare_with_threshold_intervalfss(
                    aggregated_results,
                    &random_values,
                    &fss_keys,
                    channel,
                )
            }
        }
    }

    /// Get the configuration
    pub fn config(&self) -> &ThresholdConfig {
        &self.config
    }
}

impl ThresholdPhase {
    pub fn method(&self) -> ThresholdMethod {
        self.config.method
    }
}