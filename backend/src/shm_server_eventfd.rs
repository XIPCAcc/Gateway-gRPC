use std::os::fd::AsRawFd;
use std::os::unix::io::OwnedFd;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use nix::sys::eventfd::{eventfd, EfdFlags};
use nix::sys::socket::{sendmsg, ControlMessage, MsgFlags};
use proto::{EchoRequest, EchoResponse};
use shared_memory::shm::SharedMemoryRingBuffer;
use shared_memory::ShmConfig;
use tokio::io::unix::AsyncFd;
use tracing::{info, warn, debug};

pub struct ShmServerEventfd {
    name: String,
    request_buffer: SharedMemoryRingBuffer,
    response_buffer: SharedMemoryRingBuffer,
    request_event: OwnedFd,
    response_event: OwnedFd,
    control_listener: tokio::net::UnixListener,
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
                
                let (request_id, request): (String, EchoRequest) = match bincode::deserialize(&request_buf[..n]) {
                    Ok(req) => req,
                    Err(e) => {
                        warn!("Failed to parse request: {}", e);
                        continue;
                    }
                };
                
                let response_buffer_clone = response_buffer.clone();
                let response_event_clone = response_event.clone();
                let delay_clone = delay;
                
                tokio::spawn(async move {
                    let start = std::time::Instant::now();
                    
                    if delay_clone > Duration::ZERO {
                        tokio::time::sleep(delay_clone).await;
                    }
                    
                    let processing_time = start.elapsed();
                    
                    let response = EchoResponse {
                        message: request.message,
                        timestamp_ns: processing_time.as_nanos() as i64,
                        processing_time_us: processing_time.as_micros() as i64,
                        payload: request.payload,
                    };
                    
                    let response_with_id = (request_id.clone(), response);
                    
                    let payload = match bincode::serialize(&response_with_id) {
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
