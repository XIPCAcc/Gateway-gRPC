use std::os::fd::AsRawFd;
use std::os::unix::io::OwnedFd;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use nix::sys::eventfd::{eventfd, EfdFlags};
use nix::sys::socket::{sendmsg, ControlMessage, MsgFlags};
use proto::{EchoRequest, EchoResponse, MatrixMultiplyResponse};
use shared_memory::shm::SharedMemoryRingBuffer;
use shared_memory::{ShmConfig, ShmRequest, ShmResponse, MatrixDataPool, multiply_matrices, calculate_checksum, bytes_as_f64_slice, reconstruct_matrix};
use tokio::io::unix::AsyncFd;
use tracing::{info, warn, debug};

pub struct ShmServerEventfd {
    name: String,
    request_buffer: SharedMemoryRingBuffer,
    response_buffer: SharedMemoryRingBuffer,
    request_event: OwnedFd,
    response_event: OwnedFd,
    control_listener: tokio::net::UnixListener,
    data_pool: MatrixDataPool,
}

impl ShmServerEventfd {
    pub fn new(name: &str) -> Result<Self> {
        let config = ShmConfig::default();
        
        let request_buffer = SharedMemoryRingBuffer::create(&format!("{}_req_buf", name), config)
            .map_err(|e| anyhow::anyhow!("Failed to create request buffer: {}", e))?;
        let response_buffer = SharedMemoryRingBuffer::create(&format!("{}_resp_buf", name), config)
            .map_err(|e| anyhow::anyhow!("Failed to create response buffer: {}", e))?;
        
        let request_event = eventfd(0, EfdFlags::EFD_CLOEXEC | EfdFlags::EFD_NONBLOCK)
            .map_err(|e| anyhow::anyhow!("Failed to create request eventfd: {}", e))?;
        let response_event = eventfd(0, EfdFlags::EFD_CLOEXEC | EfdFlags::EFD_NONBLOCK)
            .map_err(|e| anyhow::anyhow!("Failed to create response eventfd: {}", e))?;
        
        let control_path = format!("/tmp/{}_control.sock", name);
        let _ = std::fs::remove_file(&control_path);
        let control_listener = tokio::net::UnixListener::bind(&control_path)
            .map_err(|e| anyhow::anyhow!("Failed to bind control socket: {}", e))?;
        
        let data_pool_name = format!("{}_matrix_data", name);
        let data_pool_capacity = 256 * 1024 * 1024; // 256MB for matrix data
        let data_pool = MatrixDataPool::create(&data_pool_name, data_pool_capacity)
            .map_err(|e| anyhow::anyhow!("Failed to create matrix data pool: {}", e))?;
        
        info!(
            "Eventfd SHM server initialized (name: {}, using eventfd)",
            name
        );
        
        Ok(Self {
            name: name.to_string(),
            request_buffer,
            response_buffer,
            request_event,
            response_event,
            control_listener,
            data_pool,
        })
    }
    
    async fn send_eventfd(stream: &tokio::net::UnixStream, fd: &OwnedFd) -> Result<()> {
        let buf = [1u8; 1];
        let iov = [std::io::IoSlice::new(&buf)];
        let fds = [fd.as_raw_fd()];
        let cmsg = ControlMessage::ScmRights(&fds);
        
        sendmsg::<()>(
            stream.as_raw_fd(),
            &iov,
            &[cmsg],
            MsgFlags::empty(),
            None,
        )?;
        
        debug!("Sent eventfd {} via SCM_RIGHTS", fd.as_raw_fd());
        Ok(())
    }
    
    fn signal_eventfd(fd: &OwnedFd) -> Result<()> {
        use nix::unistd::write;
        
        let buf: [u8; 8] = 1u64.to_ne_bytes();
        match write(fd.as_raw_fd(), &buf) {
            Ok(_) => Ok(()),
            Err(nix::errno::Errno::EAGAIN) => Ok(()),
            Err(e) => Err(e.into()),
        }
    }
    
    async fn wait_eventfd(async_fd: &AsyncFd<OwnedFd>) -> Result<u64> {
        loop {
            let mut guard = async_fd.readable().await?;
            
            let mut buf = [0u8; 8];
            match nix::unistd::read(async_fd.as_raw_fd(), &mut buf) {
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
    
    pub async fn run(self, delay: Duration) -> Result<()> {
        info!("Eventfd SHM server started, waiting for gateway connection...");
        
        let (control_stream, _) = self.control_listener.accept().await?;
        info!("Gateway connected to control socket");
        
        Self::send_eventfd(&control_stream, &self.request_event).await?;
        Self::send_eventfd(&control_stream, &self.response_event).await?;
        
        let request_async_fd = AsyncFd::new(self.request_event.try_clone()?)?;
        let response_buffer = Arc::new(self.response_buffer);
        let request_buffer = Arc::new(self.request_buffer);
        let response_event = Arc::new(self.response_event);
        let mut request_buf = vec![0u8; 65536];
        
        loop {
            let count = Self::wait_eventfd(&request_async_fd).await?;
            
            for _ in 0..count {
                let n = match request_buffer.read(&mut request_buf) {
                    Ok(n) => n,
                    Err(_) => {
                        warn!("Failed to read from shared memory");
                        continue;
                    }
                };
                
                if n == 0 {
                    continue;
                }
                
                let shm_request: ShmRequest = match bincode::deserialize::<ShmRequest>(&request_buf[..n]) {
                    Ok(req) => req,
                    Err(e) => {
                        warn!("Failed to parse ShmRequest: {}", e);
                        continue;
                    }
                };
                
                let response_buffer_clone = response_buffer.clone();
                let response_event_clone = response_event.clone();
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
                                Ok(p) => p,
                                Err(e) => {
                                    warn!("Failed to serialize response: {}", e);
                                    return;
                                }
                            };
                            
                            if let Err(e) = response_buffer_clone.write(&payload) {
                                warn!("Failed to write response: {}", e);
                                return;
                            }
                            
                            if let Err(e) = Self::signal_eventfd(&response_event_clone) {
                                warn!("Failed to signal response: {}", e);
                            }
                            
                            debug!("Sent response notification for request {}", request_id);
                        });
                    }
                    ShmRequest::MatrixMultiply { id: request_id, matrix_size, data_offset, data_len } => {
                        let data_pool = self.data_pool.read_data(data_offset, data_len as usize).to_vec();
                        
                        tokio::spawn(async move {
                            let start = std::time::Instant::now();
                            
                            let size = matrix_size as usize;
                            let per_matrix_len = size * size;
                            let f64_data = bytes_as_f64_slice(&data_pool);
                            
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
                                Ok(p) => p,
                                Err(e) => {
                                    warn!("Failed to serialize response: {}", e);
                                    return;
                                }
                            };
                            
                            if let Err(e) = response_buffer_clone.write(&payload) {
                                warn!("Failed to write response: {}", e);
                                return;
                            }
                            
                            if let Err(e) = Self::signal_eventfd(&response_event_clone) {
                                warn!("Failed to signal response: {}", e);
                            }
                            
                            debug!("Sent response notification for request {}", request_id);
                        });
                    }
                }
            }
        }
    }
}
