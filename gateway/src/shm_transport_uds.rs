use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use async_trait::async_trait;
use proto::{EchoRequest, EchoResponse, MatrixMultiplyRequest, MatrixMultiplyResponse};
use shared_memory::shm::SharedMemoryRingBuffer;
use shared_memory::{ShmConfig, ShmRequest, ShmResponse, MatrixDataPool, flatten_matrix, f64_slice_as_bytes};
use tokio::io::{AsyncReadExt, AsyncWriteExt, split};
use tokio::sync::{Mutex, oneshot};
use tracing::{info, warn, error, debug};
use uuid::Uuid;

use crate::transport::Transport;

enum PendingRequest {
    Echo(oneshot::Sender<Result<EchoResponse>>),
    MatrixMultiply(oneshot::Sender<Result<MatrixMultiplyResponse>>),
}

type PendingRequests = Arc<Mutex<HashMap<String, PendingRequest>>>;

pub struct ShmTransportUds {
    name: String,
    request_buffer: Arc<SharedMemoryRingBuffer>,
    response_buffer: Arc<SharedMemoryRingBuffer>,
    notify_write: Arc<Mutex<tokio::io::WriteHalf<tokio::net::UnixStream>>>,
    pending_requests: PendingRequests,
    data_pool: Arc<MatrixDataPool>,
}

impl ShmTransportUds {
    pub async fn new(name: &str) -> Result<Self> {
        let config = ShmConfig::default();
        
        let request_buffer = Arc::new(Self::wait_for_shm(&format!("{}_req_buf", name), config).await?);
        let response_buffer = Arc::new(Self::wait_for_shm(&format!("{}_resp_buf", name), config).await?);

        let notify_path = format!("/tmp/{}_notify.sock", name);
        let notify_stream = tokio::net::UnixStream::connect(&notify_path).await
            .map_err(|e| anyhow::anyhow!("Failed to connect to notification socket: {}", e))?;

        let pending_requests: PendingRequests = Arc::new(Mutex::new(HashMap::new()));
        let pending_requests_clone = pending_requests.clone();
        let response_buffer_clone = response_buffer.clone();

        let (mut notify_read, notify_write) = split(notify_stream);
        
        tokio::spawn(async move {
            let mut buf = [0u8; 1];
            let mut response_buf = vec![0u8; 131072];
            
            info!("Response listener thread started");
            
            loop {
                debug!("Waiting for notification...");
                match notify_read.read_exact(&mut buf).await {
                    Ok(_) => {
                        debug!("Received notification byte");
                    },
                    Err(e) => {
                        warn!("Notification socket error: {}, exiting response listener", e);
                        break;
                    }
                }
                
                let mut processed = 0;
                loop {
                    let n = match response_buffer_clone.read(&mut response_buf) {
                        Ok(n) => n,
                        Err(_) => break,
                    };
                    
                    processed += 1;
                    
                    let shm_response: ShmResponse = 
                        match bincode::deserialize(&response_buf[..n]) {
                            Ok(data) => data,
                            Err(e) => {
                                warn!("Failed to deserialize response: {}", e);
                                continue;
                            }
                        };
                    
                    let mut pending = pending_requests_clone.lock().await;
                    match shm_response {
                        ShmResponse::Echo { id: request_id, response: response_bytes } => {
                            let response: EchoResponse = match bincode::deserialize(&response_bytes) {
                                Ok(r) => r,
                                Err(e) => {
                                    warn!("Failed to deserialize EchoResponse: {}", e);
                                    continue;
                                }
                            };
                            debug!("Received Echo response for request {}", request_id);
                            if let Some(PendingRequest::Echo(tx)) = pending.remove(&request_id) {
                                let _ = tx.send(Ok(response));
                                debug!("Sent response to pending request {}", request_id);
                            } else {
                                warn!("No pending Echo request for ID: {}", request_id);
                            }
                        }
                        ShmResponse::MatrixMultiply { id: request_id, response: response_bytes } => {
                            let response: MatrixMultiplyResponse = match bincode::deserialize(&response_bytes) {
                                Ok(r) => r,
                                Err(e) => {
                                    warn!("Failed to deserialize MatrixMultiplyResponse: {}", e);
                                    continue;
                                }
                            };
                            debug!("Received MatrixMultiply response for request {}", request_id);
                            if let Some(PendingRequest::MatrixMultiply(tx)) = pending.remove(&request_id) {
                                let _ = tx.send(Ok(response));
                                debug!("Sent response to pending request {}", request_id);
                            } else {
                                warn!("No pending MatrixMultiply request for ID: {}", request_id);
                            }
                        }
                    }
                }
                
                if processed > 0 {
                    debug!("Processed {} responses", processed);
                }
            }
            
            error!("Response listener thread exited!");
        });

        info!(
            "SHM transport with UDS notification initialized (name: {}, CONCURRENT MODE)",
            name
        );
        
        let data_pool_name = format!("{}_matrix_data", name);
        let data_pool = Self::wait_for_data_pool(&data_pool_name).await?;
        
        Ok(Self {
            name: name.to_string(),
            request_buffer,
            response_buffer,
            notify_write: Arc::new(Mutex::new(notify_write)),
            pending_requests,
            data_pool: Arc::new(data_pool),
        })
    }
    
    async fn wait_for_shm(name: &str, config: ShmConfig) -> Result<SharedMemoryRingBuffer> {
        let mut attempts = 0;
        loop {
            match SharedMemoryRingBuffer::open(name, config) {
                Ok(buffer) => return Ok(buffer),
                Err(_) => {
                    attempts += 1;
                    if attempts > 50 {
                        return Err(anyhow::anyhow!("Timeout waiting for shared memory: {}", name));
                    }
                    tokio::time::sleep(Duration::from_millis(100)).await;
                }
            }
        }
    }

    async fn wait_for_data_pool(name: &str) -> Result<MatrixDataPool> {
        let mut attempts = 0;
        loop {
            match MatrixDataPool::open(name) {
                Ok(pool) => return Ok(pool),
                Err(_) => {
                    attempts += 1;
                    if attempts > 50 {
                        return Err(anyhow::anyhow!("Timeout waiting for data pool: {}", name));
                    }
                    tokio::time::sleep(Duration::from_millis(100)).await;
                }
            }
        }
    }
}

#[async_trait]
impl Transport for ShmTransportUds {
    async fn call(&self, request: EchoRequest) -> Result<EchoResponse> {
        let request_id = Uuid::new_v4().to_string();
        debug!("Starting call() for request {}", request_id);
        
        let (tx, rx) = oneshot::channel();
        {
            let mut pending = self.pending_requests.lock().await;
            pending.insert(request_id.clone(), PendingRequest::Echo(tx));
            debug!("Inserted pending request {}", request_id);
        }
        
        let request_bytes = bincode::serialize(&request)
            .map_err(|e| anyhow::anyhow!("Failed to serialize request: {}", e))?;
        let shm_request = ShmRequest::Echo {
            id: request_id.clone(),
            request: request_bytes,
        };
        let payload = bincode::serialize(&shm_request)
            .map_err(|e| anyhow::anyhow!("Failed to serialize ShmRequest: {}", e))?;
        
        debug!("Writing {} bytes to shared memory for request {}", payload.len(), request_id);
        
        self.request_buffer.write(&payload)
            .map_err(|e| anyhow::anyhow!("Failed to write to shared memory: {}", e))?;
        
        debug!("Wrote request {} to shared memory", request_id);
        
        {
            let mut notify = self.notify_write.lock().await;
            notify.write_u8(1).await?;
            notify.flush().await?;
            debug!("Sent notification for request {}", request_id);
        }
        
        debug!("Waiting for response for request {}", request_id);
        
        rx.await
            .map_err(|_| anyhow::anyhow!("Response channel closed"))?
    }

    async fn matrix_multiply(&self, request: MatrixMultiplyRequest) -> Result<MatrixMultiplyResponse> {
        let request_id = Uuid::new_v4().to_string();
        debug!("Starting matrix_multiply() for request {}", request_id);
        
        let (tx, rx) = oneshot::channel();
        {
            let mut pending = self.pending_requests.lock().await;
            pending.insert(request_id.clone(), PendingRequest::MatrixMultiply(tx));
        }
        
        let a: Vec<Vec<f64>> = bincode::deserialize(&request.matrix_a)
            .map_err(|e| anyhow::anyhow!("Failed to deserialize matrix_a: {}", e))?;
        let b: Vec<Vec<f64>> = bincode::deserialize(&request.matrix_b)
            .map_err(|e| anyhow::anyhow!("Failed to deserialize matrix_b: {}", e))?;
        
        let flat_a = flatten_matrix(&a);
        let flat_b = flatten_matrix(&b);
        let bytes_a = f64_slice_as_bytes(&flat_a);
        let bytes_b = f64_slice_as_bytes(&flat_b);
        let total_len = bytes_a.len() + bytes_b.len();
        
        let offset = self.data_pool.allocate(total_len)
            .map_err(|e| anyhow::anyhow!("Failed to allocate data pool: {}", e))?;
        
        self.data_pool.write_data(offset, bytes_a);
        self.data_pool.write_data(offset + bytes_a.len() as u64, bytes_b);
        
        let shm_request = ShmRequest::MatrixMultiply {
            id: request_id.clone(),
            matrix_size: request.matrix_size,
            data_offset: offset,
            data_len: total_len as u32,
        };
        let payload = bincode::serialize(&shm_request)
            .map_err(|e| anyhow::anyhow!("Failed to serialize ShmRequest: {}", e))?;
        
        self.request_buffer.write(&payload)
            .map_err(|e| anyhow::anyhow!("Failed to write to shared memory: {}", e))?;
        
        {
            let mut notify = self.notify_write.lock().await;
            notify.write_u8(1).await?;
            notify.flush().await?;
        }
        
        rx.await
            .map_err(|_| anyhow::anyhow!("Response channel closed"))?
    }
}
