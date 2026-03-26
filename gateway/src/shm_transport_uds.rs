use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use async_trait::async_trait;
use proto::{EchoRequest, EchoResponse};
use shared_memory::shm::SharedMemoryRingBuffer;
use shared_memory::ShmConfig;
use tokio::io::{AsyncReadExt, AsyncWriteExt, split};
use tokio::sync::{Mutex, oneshot};
use tracing::{info, warn, error, debug};
use uuid::Uuid;

use crate::transport::Transport;

type PendingRequests = Arc<Mutex<HashMap<String, oneshot::Sender<Result<EchoResponse>>>>>;

pub struct ShmTransportUds {
    name: String,
    request_buffer: Arc<SharedMemoryRingBuffer>,
    response_buffer: Arc<SharedMemoryRingBuffer>,
    notify_write: Arc<Mutex<tokio::io::WriteHalf<tokio::net::UnixStream>>>,
    pending_requests: PendingRequests,
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
                    
                    let (request_id, response): (String, EchoResponse) = 
                        match bincode::deserialize(&response_buf[..n]) {
                            Ok(data) => data,
                            Err(e) => {
                                warn!("Failed to deserialize response: {}", e);
                                continue;
                            }
                        };
                    
                    debug!("Received response for request {}", request_id);
                    
                    let mut pending = pending_requests_clone.lock().await;
                    if let Some(tx) = pending.remove(&request_id) {
                        let _ = tx.send(Ok(response));
                        debug!("Sent response to pending request {}", request_id);
                    } else {
                        warn!("No pending request for ID: {}", request_id);
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
        
        Ok(Self {
            name: name.to_string(),
            request_buffer,
            response_buffer,
            notify_write: Arc::new(Mutex::new(notify_write)),
            pending_requests,
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
}

#[async_trait]
impl Transport for ShmTransportUds {
    async fn call(&self, request: EchoRequest) -> Result<EchoResponse> {
        let request_id = Uuid::new_v4().to_string();
        debug!("Starting call() for request {}", request_id);
        
        let (tx, rx) = oneshot::channel();
        {
            let mut pending = self.pending_requests.lock().await;
            pending.insert(request_id.clone(), tx);
            debug!("Inserted pending request {}", request_id);
        }
        
        let request_with_id = (request_id.clone(), request);
        let payload = bincode::serialize(&request_with_id)
            .map_err(|e| anyhow::anyhow!("Failed to serialize request: {}", e))?;
        
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
}
