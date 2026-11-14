use crate::{
    channel::CommTrackingChannel,
    fuzzy_match::{
        share_phase::SharePhase,
        share_phase_types::{ShareConfig, ShareMethod},
        shared_range::SharedRange,
        sketch_phase::SketchPhase,
        sketch_phase_types::{SketchConfig, SketchData},
    },
    randomness::prg::PRG,
};
use scuttlebutt::AbstractChannel;
use rand::RngExt;
use anyhow::{anyhow, ensure, Result};

/// Client structure that handles client-side operations
#[derive(Clone)]
pub struct Client {
    share_phase: SharePhase,
    sketch_phase: Option<SketchPhase>,
    enable_sketch: bool,
    num_clients: usize,
}

impl Client {
    /// Create a new client instance
    pub fn new<C: Into<ShareConfig> + Into<SketchConfig> + Clone>(
        config: C,
        enable_sketch: bool,
        num_clients: usize,
    ) -> Self {
        let share_phase = SharePhase::new(config.clone());
        let sketch_phase = if enable_sketch {
            Some(SketchPhase::new(config))
        } else {
            None
        };

        Self { 
            share_phase,
            sketch_phase,
            enable_sketch,
            num_clients,
        }
    }

    /// Generate shares for all client points
    /// Returns shares for server 0 and server 1 respectively, optionally including sketch data
    pub fn generate_client_shares(
        &self,
        client_points: &[Vec<u128>],
    ) -> Result<
        (
            Vec<SharedRange>,
            Vec<SharedRange>,
        )
    > {
        ensure!(
            client_points.len() == self.num_clients,
            "Number of client points does not match expected number of clients, expected {}, got {}",
            self.num_clients,
            client_points.len(),
        );
        if self.share_method() != ShareMethod::FSS && self.enable_sketch {
            return Err(anyhow!("Sketch generation is only supported with FSS share method"));
        }

        // Generate keys first
        let mut shares_server0 = Vec::new();
        let mut shares_server1 = Vec::new();

        for client_point in client_points {
            let (share0, share1) = self.share_phase
                .share_range(client_point, self.delta())
                .map_err(|e| anyhow!("Failed to generate shares: {:?}", e))?;

            shares_server0.push(share0.clone());
            shares_server1.push(share1.clone());
        }

        Ok((
            shares_server0, 
            shares_server1, 
        ))
    }

    pub fn send_client_shares(
        &self,
        shares_server0: Vec<SharedRange>,
        shares_server1: Vec<SharedRange>,
        channel_server0: &mut CommTrackingChannel,
        channel_server1: &mut CommTrackingChannel,
    ) -> Result<()> {
        // Custom serialization for Vec<SharedRange>

        // Send shares for server 0
        let mut data = Vec::new();
        data.extend_from_slice(&(shares_server0.len() as u64).to_le_bytes());
        for share in &shares_server0 {
            data.extend_from_slice(&share.to_bytes());
        }
        let len_bytes = (data.len() as u64).to_le_bytes();
        crate::bench::mark("client_first_byte"); // A3: the upload starts with the first byte sent
        channel_server0
            .write_bytes(&len_bytes)
            .map_err(|e| anyhow!("Failed to send length to server 0: {}", e))?;
        channel_server0
            .write_bytes(&data)
            .map_err(|e| anyhow!("Failed to send shares to server 0: {}", e))?;
        channel_server0
            .flush()
            .map_err(|e| anyhow!("Failed to flush to server 0: {}", e))?;

        // Send shares for server 1
        let mut data = Vec::new();
        data.extend_from_slice(&(shares_server1.len() as u64).to_le_bytes());
        for share in &shares_server1 {
            data.extend_from_slice(&share.to_bytes());
        }
        let len_bytes = (data.len() as u64).to_le_bytes();
        channel_server1
            .write_bytes(&len_bytes)
            .map_err(|e| anyhow!("Failed to send length to server 1: {}", e))?;
        channel_server1
            .write_bytes(&data)
            .map_err(|e| anyhow!("Failed to send shares to server 1: {}", e))?;
        channel_server1
            .flush()
            .map_err(|e| anyhow!("Failed to flush to server 1: {}", e))?;

        Ok(())
    }

    pub fn generate_client_sketch_data<'a>(
        &'a self,
    ) -> Result<(Vec<Vec<SketchData<'a>>>, Vec<Vec<SketchData<'a>>>)> {
        ensure!(
            self.enable_sketch,
            "Sketch generation is not enabled for this client",
        );
        if self.share_method() != ShareMethod::FSS {
            return Err(anyhow!("Sketch generation is only supported with FSS share method"));
        }

        let mut sketches_server0: Vec<Vec<SketchData>> = Vec::with_capacity(self.num_clients);
        let mut sketches_server1: Vec<Vec<SketchData>> = Vec::with_capacity(self.num_clients);
        if let Some(phase) = &self.sketch_phase {
            for _ in 0..self.num_clients {
                let mut prg =
                    PRG::new(Some(&rand::rng().random::<[u8; 16]>()), 0);
                let (mut sketches0, mut sketches1) = (Vec::new(), Vec::new());
                for _ in 0..self.d() {
                    let (d0, d1) = phase
                        .get_sketch_data(&mut prg)
                        .map_err(|e| anyhow!("Failed to get sketch data: {}", e))?;
                    sketches0.push(d0);
                    sketches1.push(d1);
                }
                sketches_server0.push(sketches0);
                sketches_server1.push(sketches1);
            }
            Ok((sketches_server0, sketches_server1))
        } else {
            Err(anyhow!("Sketch phase is not initialized while self.enable_sketch is true"))
        }
    }

    pub fn send_sketch_data(
        &self,
        sketch_data_server0: Vec<Vec<SketchData>>,
        sketch_data_server1: Vec<Vec<SketchData>>,
        channel_server0: &mut CommTrackingChannel,
        channel_server1: &mut CommTrackingChannel,
    ) -> Result<()> {
        // Send sketch data for server 0
        let mut data = Vec::new();
        data.extend_from_slice(&(sketch_data_server0.len() as u64).to_le_bytes());
        data.extend_from_slice(&self.d().to_le_bytes());
        for sketch_vec in &sketch_data_server0 {
            ensure!(
                sketch_vec.len() == self.d(),
                "Sketch data length does not match d, expected {}, got {}",
                self.d(),
                sketch_vec.len(),
            );
            for sketch in sketch_vec {
                data.extend_from_slice(&sketch.to_bytes()); 
            }
        }

        let len_bytes = (data.len() as u64).to_le_bytes();
        channel_server0
            .write_bytes(&len_bytes)
            .map_err(|e| anyhow!("Failed to send sketch length to server 0: {}", e))?;
        channel_server0
            .write_bytes(&data)
            .map_err(|e| anyhow!("Failed to send sketch data to server 0: {}", e))?;
        channel_server0
            .flush()
            .map_err(|e| anyhow!("Failed to flush to server 0: {}", e))?;

        // Send sketch data for server 1
        let mut data = Vec::new();
        data.extend_from_slice(&(sketch_data_server1.len() as u64).to_le_bytes());
        data.extend_from_slice(&self.d().to_le_bytes());
        for sketch_vec in &sketch_data_server1 {
            ensure!(
                sketch_vec.len() == self.d(),
                "Sketch data length does not match d, expected {}, got {}",
                self.d(),
                sketch_vec.len(),
            );
            for sketch in sketch_vec {
                data.extend_from_slice(&sketch.to_bytes()); 
            }
        }
        let len_bytes = (data.len() as u64).to_le_bytes();
        channel_server1
            .write_bytes(&len_bytes)
            .map_err(|e| anyhow!("Failed to send sketch length to server 1: {}", e))?;
        channel_server1
            .write_bytes(&data)
            .map_err(|e| anyhow!("Failed to send sketch data to server 1: {}", e))?;
        channel_server1
            .flush()
            .map_err(|e| anyhow!("Failed to flush to server 1: {}", e))?;

        Ok(())
    }
}

impl Client {
    pub fn share_method(&self) -> ShareMethod {
        self.share_phase.method()
    }

    pub fn delta(&self) -> u128 {
        self.share_phase.delta()
    }

    pub fn d(&self) -> usize {
        self.share_phase.d()
    }
}