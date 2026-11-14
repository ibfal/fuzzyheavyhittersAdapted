use scuttlebutt::{AbstractChannel, SyncChannel};
use std::io::{BufReader, BufWriter};
use std::net::TcpListener;
use std::net::TcpStream;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

/// A wrapper around scuttlebutt's SyncChannel that tracks communication costs
#[derive(Clone)]
pub struct CommTrackingChannel {
    inner: SyncChannel<BufReader<TcpStream>, BufWriter<TcpStream>>,
    bytes_sent: Arc<Mutex<usize>>,
    bytes_received: Arc<Mutex<usize>>,
}

impl CommTrackingChannel {
    pub fn new(reader: BufReader<TcpStream>, writer: BufWriter<TcpStream>) -> Self {
        Self {
            inner: SyncChannel::new(reader, writer),
            bytes_sent: Arc::new(Mutex::new(0)),
            bytes_received: Arc::new(Mutex::new(0)),
        }
    }

    pub fn get_communication_stats(&self) -> (usize, usize) {
        let sent = *self.bytes_sent.lock().unwrap();
        let received = *self.bytes_received.lock().unwrap();
        (sent, received)
    }

    pub fn reset_stats(&self) {
        *self.bytes_sent.lock().unwrap() = 0;
        *self.bytes_received.lock().unwrap() = 0;
    }

    /// Get total communication cost (sent + received)
    pub fn get_total_communication(&self) -> usize {
        let (sent, received) = self.get_communication_stats();
        sent + received
    }

    /// Print communication statistics
    pub fn print_stats(&self, label: &str) {
        let (sent, received) = self.get_communication_stats();
        println!("{} Communication Stats:", label);
        println!("  Bytes sent: {}", sent);
        println!("  Bytes received: {}", received);
        println!("  Total: {} bytes", sent + received);
    }
}

impl AbstractChannel for CommTrackingChannel {
    fn write_bytes(&mut self, data: &[u8]) -> std::io::Result<()> {
        *self.bytes_sent.lock().unwrap() += data.len();
        self.inner.write_bytes(data)
    }

    fn read_bytes(&mut self, data: &mut [u8]) -> std::io::Result<()> {
        let result = self.inner.read_bytes(data);
        if result.is_ok() {
            *self.bytes_received.lock().unwrap() += data.len();
        }
        result
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.inner.flush()
    }
}

pub fn connect_to(
    ip: String,
    port: u16,
) -> Result<CommTrackingChannel, Box<dyn std::error::Error>> {
    let addr = format!("{}:{}", ip, port);
    println!("Connecting to {}", addr);
    let stream = loop {
        match TcpStream::connect(&addr) {
            Ok(s) => break s,
            Err(e) => {
                println!("Failed to connect to {}: {}. Retrying...", addr, e);
                thread::sleep(Duration::from_secs(1));
            }
        }
    };
    stream.set_nodelay(true)?;
    let reader = BufReader::new(stream.try_clone()?);
    let writer = BufWriter::new(stream);
    Ok(CommTrackingChannel::new(reader, writer))
}

/// Environment variable that sets the reader and writer buffer size of the listening side of a
/// channel, in bytes. Unset, the size is the original 1 GiB.
pub const CHANNEL_BUFFER_ENV: &str = "MOSAIC_CHANNEL_BUFFER_BYTES";

/// The original buffer size of the listening side: 64 * 4096 * 4096 bytes = 1 GiB.
pub const DEFAULT_CHANNEL_BUFFER_BYTES: usize = 64 * 4096 * 4096;

/// The buffer size `listen_to` uses: `MOSAIC_CHANNEL_BUFFER_BYTES` if set, else 1 GiB.
pub fn channel_buffer_bytes() -> Result<usize, Box<dyn std::error::Error>> {
    match std::env::var(CHANNEL_BUFFER_ENV) {
        Err(std::env::VarError::NotPresent) => Ok(DEFAULT_CHANNEL_BUFFER_BYTES),
        Err(e) => Err(format!("{}: {}", CHANNEL_BUFFER_ENV, e).into()),
        Ok(value) => match value.trim().parse::<usize>() {
            Ok(bytes) if bytes > 0 => Ok(bytes),
            _ => Err(format!(
                "{} must be a positive number of bytes, got {:?}",
                CHANNEL_BUFFER_ENV, value
            )
            .into()),
        },
    }
}

pub fn listen_to(ip: String, port: u16) -> Result<CommTrackingChannel, Box<dyn std::error::Error>> {
    let addr = format!("{}:{}", ip, port);
    println!("Listening on {}", addr);

    let buffer_bytes = channel_buffer_bytes()?;
    let listener = TcpListener::bind(&addr)?;
    let (stream, _) = listener.accept()?;
    stream.set_nodelay(true)?;
    let reader = BufReader::with_capacity(buffer_bytes, stream.try_clone()?);
    let writer = BufWriter::with_capacity(buffer_bytes, stream);
    Ok(CommTrackingChannel::new(reader, writer))
}

// Set up multiple parallel channels
// CAUTION: The number of channels should be exactly equal to the number of threads used. We do not use Mutex here.
pub fn setup_parallel_channels(
    is_connector: bool, // true if this side initiates connections
    num_channels: usize,
    target_addr: &str,
    base_port: u16,
) -> Result<Vec<CommTrackingChannel>, String> {
    let mut channels = Vec::with_capacity(num_channels);

    for i in 0..num_channels {
        let port = base_port + i as u16;
        let channel = if is_connector {
            connect_to(target_addr.to_string(), port)
                .map_err(|e| format!("Failed to connect to {}: {}: {}", target_addr, port, e))?
        } else {
            listen_to(target_addr.to_string(), port)
                .map_err(|e| format!("Failed to listen on {}: {}: {}", target_addr, port, e))?
        };
        channels.push(channel);
    }

    Ok(channels)
}
