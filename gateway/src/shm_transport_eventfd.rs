use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::sync::{Arc, atomic::{AtomicUsize, Ordering}};
use std::time::Duration;
use std::collections::HashMap;

use anyhow::Result;
use async_trait::async_trait;
use nix::sys::eventfd::{eventfd, EfdFlags};
use proto::{EchoRequest, EchoResponse, MatrixMultiplyRequest, MatrixMultiplyResponse};
use shared_memory::shm::SharedMemoryRingBuffer;
use shared_memory::{ShmConfig, ShmRequest, ShmResponse, MatrixDataPool, flatten_matrix, f64_slice_as_bytes};
use tokio::io::unix::AsyncFd;
use tokio::io::AsyncReadExt;
use tokio::sync::Mutex;
use tokio::sync::oneshot;
use tracing::{info, debug, warn};
use uuid::Uuid;

// 日志采样计数器和采样间隔
static LATENCY_LOG_COUNTER: AtomicUsize = AtomicUsize::new(0);
const LATENCY_LOG_INTERVAL: usize = 10;

use crate::transport::Transport;

enum PendingRequest {
    Echo(oneshot::Sender<Result<EchoResponse>>),
    MatrixMultiply(oneshot::Sender<Result<MatrixMultiplyResponse>>),
}

type PendingRequests = Arc<Mutex<HashMap<String, PendingRequest>>>;

pub struct ShmTransportEventfd {
    name: String,
    request_buffer: Arc<SharedMemoryRingBuffer>,
    response_buffer: Arc<SharedMemoryRingBuffer>,
    request_event: Arc<AsyncFd<OwnedFd>>,
    response_event: Arc<AsyncFd<OwnedFd>>,
    pending_requests: PendingRequests,
    data_pool: Arc<MatrixDataPool>,
}

impl ShmTransportEventfd {
    pub async fn new(name: &str) -> Result<Self> {
        let config = ShmConfig::default();
        
        let request_buffer = Self::wait_for_shm(&format!("{}_req_buf", name), config).await?;
        let response_buffer = Self::wait_for_shm(&format!("{}_resp_buf", name), config).await?;

        let control_path = format!("/tmp/{}_control.sock", name);
        let mut control_stream = tokio::net::UnixStream::connect(&control_path).await
            .map_err(|e| anyhow::anyhow!("Failed to connect to control socket: {}", e))?;

        let request_event = Self::recv_eventfd(&mut control_stream).await?;
        let response_event = Self::recv_eventfd(&mut control_stream).await?;

        let data_pool_name = format!("{}_matrix_data", name);
        let data_pool = Self::wait_for_data_pool(&data_pool_name).await?;

        info!(
            "Eventfd SHM transport initialized (name: {}, using eventfd)",
            name
        );
        
        let transport = Self {
            name: name.to_string(),
            request_buffer: Arc::new(request_buffer),
            response_buffer: Arc::new(response_buffer),
            request_event: Arc::new(request_event),
            response_event: Arc::new(response_event),
            pending_requests: Arc::new(Mutex::new(HashMap::new())),
            data_pool: Arc::new(data_pool),
        };
        
        transport.start_response_listener();
        
        Ok(transport)
    }
    
    fn start_response_listener(&self) {
        let response_buffer = self.response_buffer.clone();
        let response_event = self.response_event.clone();
        let pending_requests = self.pending_requests.clone();
        
        tokio::spawn(async move {
            let mut response_buf = vec![0u8; 65536];
            
            loop {
                match Self::wait_eventfd_async(&response_event).await {
                    Ok(count) => {
                        for _ in 0..count {
                            let n = match response_buffer.read(&mut response_buf) {
                                Ok(n) => n,
                                Err(_) => break,
                            };
                            
                            if n == 0 {
                                break;
                            }
                            
                            let shm_response: ShmResponse = 
                                match bincode::deserialize(&response_buf[..n]) {
                                    Ok(data) => data,
                                    Err(e) => {
                                        warn!("Failed to deserialize response: {}", e);
                                        continue;
                                    }
                                };
                            
                            let mut pending = pending_requests.lock().await;
                            match shm_response {
                                ShmResponse::Echo { id: request_id, response: response_bytes } => {
                                    let response: EchoResponse = match bincode::deserialize(&response_bytes) {
                                        Ok(r) => r,
                                        Err(e) => {
                                            warn!("Failed to deserialize EchoResponse: {}", e);
                                            continue;
                                        }
                                    };
                                    if let Some(PendingRequest::Echo(tx)) = pending.remove(&request_id) {
                                        let _ = tx.send(Ok(response));
                                    } else {
                                        warn!("No pending Echo request found for {}", request_id);
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
                                    if let Some(PendingRequest::MatrixMultiply(tx)) = pending.remove(&request_id) {
                                        let _ = tx.send(Ok(response));
                                    } else {
                                        warn!("No pending MatrixMultiply request found for {}", request_id);
                                    }
                                }
                            }
                        }
                    }
                    Err(e) => {
                        warn!("Error waiting for response event: {}", e);
                    }
                }
            }
        });
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
    
    async fn recv_eventfd(stream: &mut tokio::net::UnixStream) -> Result<AsyncFd<OwnedFd>> {
        use nix::sys::socket::{recvmsg, ControlMessageOwned, MsgFlags};
        use std::io::IoSliceMut;
        
        let mut buf = [0u8; 1];
        let mut iov = [IoSliceMut::new(&mut buf)];
        let mut cmsg_buffer = nix::cmsg_space!([i32; 1]);
        
        loop {
            stream.readable().await?;
            
            match recvmsg::<()>(
                stream.as_raw_fd(),
                &mut iov,
                Some(&mut cmsg_buffer),
                MsgFlags::empty(),
            ) {
                Ok(msg) => {
                    let received_fd: i32 = msg
                        .cmsgs()
                        .find_map(|cmsg| {
                            if let ControlMessageOwned::ScmRights(fds) = cmsg {
                                fds.into_iter().next()
                            } else {
                                None
                            }
                        })
                        .ok_or_else(|| anyhow::anyhow!("No file descriptor received"))?;
                    
                    let owned_fd = unsafe { OwnedFd::from_raw_fd(received_fd) };
                    
                    return AsyncFd::new(owned_fd)
                        .map_err(|e| anyhow::anyhow!("Failed to create AsyncFd: {}", e));
                }
                Err(nix::errno::Errno::EAGAIN) => {
                    continue;
                }
                Err(e) => {
                    return Err(e.into());
                }
            }
        }
    }
    
    fn signal_eventfd(fd: &AsyncFd<OwnedFd>) -> Result<()> {
        use nix::unistd::write;
        
        let buf: [u8; 8] = 1u64.to_ne_bytes();
        match write(fd.as_raw_fd(), &buf) {
            Ok(_) => Ok(()),
            Err(nix::errno::Errno::EAGAIN) => Ok(()),
            Err(e) => Err(e.into()),
        }
    }
    
    async fn wait_eventfd_async(fd: &AsyncFd<OwnedFd>) -> Result<u64> {
        loop {
            let mut guard = fd.readable().await?;
            
            let mut buf = [0u8; 8];
            match nix::unistd::read(fd.as_raw_fd(), &mut buf) {
                Ok(_) => {
                    let count = u64::from_ne_bytes(buf);
                    if count > 0 {
                        guard.clear_ready();
                        return Ok(count);
                    }
                }
                Err(nix::errno::Errno::EAGAIN) => {
                    guard.clear_ready();
                    continue;
                }
                Err(e) => {
                    return Err(e.into());
                }
            }
        }
    }
}

#[async_trait]
impl Transport for ShmTransportEventfd {
    async fn call(&self, request: EchoRequest) -> Result<EchoResponse> {
        let request_id = Uuid::new_v4().to_string();
        
        let (tx, rx) = oneshot::channel();
        {
            let mut pending = self.pending_requests.lock().await;
            pending.insert(request_id.clone(), PendingRequest::Echo(tx));
        }
        
        let request_bytes = bincode::serialize(&request)
            .map_err(|e| anyhow::anyhow!("Failed to serialize request: {}", e))?;
        let shm_request = ShmRequest::Echo {
            id: request_id.clone(),
            request: request_bytes,
        };
        let payload = bincode::serialize(&shm_request)
            .map_err(|e| anyhow::anyhow!("Failed to serialize ShmRequest: {}", e))?;
        
        self.request_buffer.write(&payload)
            .map_err(|e| anyhow::anyhow!("Failed to write to shared memory: {}", e))?;
        
        Self::signal_eventfd(&self.request_event)?;
        
        rx.await
            .map_err(|_| anyhow::anyhow!("Response channel closed"))?
    }

    async fn matrix_multiply(&self, request: MatrixMultiplyRequest) -> Result<MatrixMultiplyResponse> {
        let request_id = Uuid::new_v4().to_string();
        
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
        
        let start_time = std::time::Instant::now();
        
        Self::signal_eventfd(&self.request_event)?;
        
        let mut result = rx.await
            .map_err(|_| anyhow::anyhow!("Response channel closed"))??;
        
        let shm_roundtrip_us = start_time.elapsed().as_micros() as i64;
        result.shm_roundtrip_us = shm_roundtrip_us;
        
        // 日志采样：每 LATENCY_LOG_INTERVAL 个请求记录一次
        let count = LATENCY_LOG_COUNTER.fetch_add(1, Ordering::Relaxed);
        if count % LATENCY_LOG_INTERVAL == 0 {
            // warn!("matrix_multiply(): 请求 {} 的 SHM 往返延迟: {}us (采样率: 1/{})", 
            //       request_id, shm_roundtrip_us, LATENCY_LOG_INTERVAL);
            warn!("SHM: {}us", shm_roundtrip_us);
        }
        // warn!("SHM: {}us", shm_roundtrip_us);
        Ok(result)
    }
}
