use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use proto::{EchoRequest, EchoResponse, MatrixMultiplyResponse};
use shared_memory::shm::SharedMemoryRingBuffer;
use shared_memory::{ShmConfig, ShmRequest, ShmResponse, MatrixDataPool, multiply_matrices, calculate_checksum, bytes_as_f64_slice, reconstruct_matrix};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::Mutex;
use tracing::{info, warn, debug};

pub struct ShmServerUds {
    name: String,
    request_buffer: SharedMemoryRingBuffer,
    response_buffer: Arc<SharedMemoryRingBuffer>,
    notify_listener: tokio::net::UnixListener,
    data_pool: MatrixDataPool,
}

impl ShmServerUds {
    pub fn new(name: &str) -> Result<Self> {
        let config = ShmConfig::default();
        
        let request_buffer = SharedMemoryRingBuffer::create(&format!("{}_req_buf", name), config)
            .map_err(|e| anyhow::anyhow!("Failed to create request buffer: {}", e))?;
        let response_buffer = SharedMemoryRingBuffer::create(&format!("{}_resp_buf", name), config)
            .map_err(|e| anyhow::anyhow!("Failed to create response buffer: {}", e))?;
        
        let notify_path = format!("/tmp/{}_notify.sock", name);
        let _ = std::fs::remove_file(&notify_path);
        
        let notify_listener = tokio::net::UnixListener::bind(&notify_path)
            .map_err(|e| anyhow::anyhow!("Failed to bind notification socket: {}", e))?;
        
        let data_pool_name = format!("{}_matrix_data", name);
        let data_pool_capacity = 256 * 1024 * 1024;
        let data_pool = MatrixDataPool::create(&data_pool_name, data_pool_capacity)
            .map_err(|e| anyhow::anyhow!("Failed to create matrix data pool: {}", e))?;
        
        info!(
            "SHM server with UDS notification initialized (name: {}, CONCURRENT MODE)",
            name
        );
        
        Ok(Self {
            name: name.to_string(),
            request_buffer,
            response_buffer: Arc::new(response_buffer),
            notify_listener,
            data_pool,
        })
    }
    
    pub async fn run(mut self, delay: Duration) -> Result<()> {
        info!("SHM server started, waiting for gateway connection...");
        
        let (stream, _) = self.notify_listener.accept().await?;
        info!("Gateway connected to notification socket");
        
        let (mut read_half, mut write_half) = tokio::io::split(stream);
        let response_buffer = self.response_buffer.clone();
        let request_buffer = Arc::new(self.request_buffer);
        let mut request_buf = vec![0u8; 65536];
        
        let write_half = Arc::new(Mutex::new(write_half));
        let write_half_clone = write_half.clone();
        
        tokio::spawn(async move {
            let mut notify_buf = [0u8; 1];
            
            info!("Request listener thread started");
            
            loop {
                debug!("Waiting for request notification...");
                match read_half.read_exact(&mut notify_buf).await {
                    Ok(_) => {
                        debug!("Received request notification byte: {}", notify_buf[0]);
                    },
                    Err(e) => {
                        warn!("Failed to read notification: {}", e);
                        break;
                    }
                }
                
                let n = match request_buffer.read(&mut request_buf) {
                    Ok(n) => n,
                    Err(e) => {
                        warn!("Failed to read from shared memory: {}", e);
                        continue;
                    }
                };
                
                debug!("Read {} bytes from shared memory", n);
                
                let shm_request: ShmRequest = 
                    match bincode::deserialize::<ShmRequest>(&request_buf[..n]) {
                        Ok(data) => data,
                        Err(e) => {
                            warn!("Failed to parse ShmRequest: {}", e);
                            continue;
                        }
                    };
                
                let response_buffer_clone = response_buffer.clone();
                let write_half_clone2 = write_half_clone.clone();
                let delay_clone = delay;
                
                match shm_request {
                    ShmRequest::Echo { id: request_id, request: request_bytes } => {
                        let request: EchoRequest = match bincode::deserialize(&request_bytes) {
                            Ok(r) => r,
                            Err(e) => {
                                warn!("Failed to parse EchoRequest: {}", e);
                                continue;
                            }
                        };
                        
                        debug!("Processing echo request {}", request_id);
                        
                        tokio::spawn(async move {
                            let start = std::time::Instant::now();
                            
                            if delay_clone > Duration::ZERO {
                                let start = std::time::Instant::now();
                                while start.elapsed() < delay_clone {
                                    std::hint::spin_loop();
                                }
                            }
                            
                            let processing_time = start.elapsed();
                            
                            let response = EchoResponse {
                                message: request.message,
                                timestamp_ns: processing_time.as_nanos() as i64,
                                processing_time_us: processing_time.as_micros() as i64,
                                payload: request.payload,
                            };
                            
                            let response_bytes = bincode::serialize(&response).unwrap();
                            let shm_response = ShmResponse::Echo {
                                id: request_id.clone(),
                                response: response_bytes,
                            };
                            
                            let payload = match bincode::serialize(&shm_response) {
                                Ok(data) => data,
                                Err(e) => {
                                    warn!("Failed to serialize response: {}", e);
                                    return;
                                }
                            };
                            
                            if let Err(e) = response_buffer_clone.write(&payload) {
                                warn!("Failed to write response: {}", e);
                                return;
                            }
                            
                            debug!("Wrote response for request {} to shared memory", request_id);
                            
                            {
                                let mut write_guard = write_half_clone2.lock().await;
                                if let Err(e) = write_guard.write_u8(1).await {
                                    warn!("Failed to notify gateway: {}", e);
                                } else {
                                    let _ = write_guard.flush().await;
                                    debug!("Sent response notification for request {}", request_id);
                                }
                            }
                        });
                    }
                    ShmRequest::MatrixMultiply { id: request_id, matrix_size, data_offset, data_len } => {
                        let data = self.data_pool.read_data(data_offset, data_len as usize).to_vec();
                        
                        tokio::spawn(async move {
                            let start = std::time::Instant::now();
                            
                            let size = matrix_size as usize;
                            let per_matrix_len = size * size;
                            let f64_data = bytes_as_f64_slice(&data);
                            
                            if f64_data.len() < per_matrix_len * 2 {
                                warn!("Matrix data too short: expected {}, got {}", per_matrix_len * 2, f64_data.len());
                                return;
                            }
                            
                            let a = reconstruct_matrix(&f64_data[..per_matrix_len], size);
                            let b = reconstruct_matrix(&f64_data[per_matrix_len..], size);

                            let mut result = vec![vec![0.0; size]; size];
                            multiply_matrices(&a, &b, &mut result);
                            let checksum = calculate_checksum(&result);
                            
                            let processing_time = start.elapsed();
                            
                            let total_ops = 2.0 * (size as f64).powi(3);
                            let gflops = total_ops / (processing_time.as_secs_f64() * 1e9);
                            
                            let response = MatrixMultiplyResponse {
                                checksum,
                                timestamp_ns: processing_time.as_nanos() as i64,
                                processing_time_us: processing_time.as_micros() as i64,
                                gflops,
                                shm_roundtrip_us: 0,
                            };
                            
                            let response_bytes = bincode::serialize(&response).unwrap();
                            let shm_response = ShmResponse::MatrixMultiply {
                                id: request_id.clone(),
                                response: response_bytes,
                            };
                            
                            let payload = match bincode::serialize(&shm_response) {
                                Ok(data) => data,
                                Err(e) => {
                                    warn!("Failed to serialize response: {}", e);
                                    return;
                                }
                            };
                            
                            if let Err(e) = response_buffer_clone.write(&payload) {
                                warn!("Failed to write response: {}", e);
                                return;
                            }
                            
                            {
                                let mut write_guard = write_half_clone2.lock().await;
                                if let Err(e) = write_guard.write_u8(1).await {
                                    warn!("Failed to notify gateway: {}", e);
                                } else {
                                    let _ = write_guard.flush().await;
                                    debug!("Sent response notification for request {}", request_id);
                                }
                            }
                        });
                    }
                }
            }
            
            info!("Request listener thread exited");
        });
        
        tokio::signal::ctrl_c().await?;
        info!("Shutting down SHM server");
        
        Ok(())
    }
}
