use std::collections::HashMap;
use std::os::unix::io::RawFd;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::Result;
use async_trait::async_trait;
use proto::{EchoRequest, EchoResponse};
use shared_memory::shm::SharedMemoryRingBuffer;
use shared_memory::{ShmConfig, ShmError};
use tokio::sync::{Mutex, oneshot};
use tracing::{info, warn, error, debug};
use uuid::Uuid;
use nix::libc;

use crate::transport::Transport;

use uintr::{UintrError, UintrResult};
use uintr::syscall::{uintr_register_handler, uintr_create_fd, uintr_register_sender, senduipi, stui};
use uintr::connection::setup_client_connection;
use uintr::UINTR_HANDLER_FLAG_WAITING_ANY;
use uintr::async_wait::{init_token, get_token, process_uintr_wakers, uintr_wait};

type PendingRequests = Arc<Mutex<HashMap<String, oneshot::Sender<Result<EchoResponse>>>>>;

unsafe extern "C" {
    pub fn ui_handler(ui_frame: *mut uintr::syscall::UintrFrame, vector: u64);
    static mut uintr_received: libc::c_ulong;
}

static mut CLIENT_UINTRFD: RawFd = -1;
static mut CLIENT_UIPI_INDEX: libc::c_int = -1;

fn get_client_uintrfd() -> RawFd {
    unsafe { CLIENT_UINTRFD }
}

fn set_client_uintrfd(fd: RawFd) {
    unsafe {
        CLIENT_UINTRFD = fd;
    }
}

fn get_client_uipi_index() -> libc::c_int {
    unsafe { CLIENT_UIPI_INDEX }
}

fn set_client_uipi_index(index: libc::c_int) {
    unsafe {
        CLIENT_UIPI_INDEX = index;
    }
}

pub struct ShmTransportUintr {
    name: String,
    request_buffer: Arc<SharedMemoryRingBuffer>,
    response_buffer: Arc<SharedMemoryRingBuffer>,
    pending_requests: PendingRequests,
    running: Arc<AtomicBool>,
}

impl ShmTransportUintr {
    pub async fn new(name: &str) -> Result<Self> {
        let config = ShmConfig::default();
        
        let request_buffer = Arc::new(Self::wait_for_shm(&format!("{}_req_buf", name), config).await?);
        let response_buffer = Arc::new(Self::wait_for_shm(&format!("{}_resp_buf", name), config).await?);

        let socket_path = format!("/tmp/{}_uintr.sock", name);
        
        let pending_requests: PendingRequests = Arc::new(Mutex::new(HashMap::new()));
        let pending_requests_clone = pending_requests.clone();
        let response_buffer_clone = response_buffer.clone();
        let running = Arc::new(AtomicBool::new(true));
        let running_clone = running.clone();

        let uipi_index = Self::setup_uintr(&socket_path).await?;

        let token = get_token()
            .map_err(|e| anyhow::anyhow!("Failed to get token: {}", e))?;
        
        tokio::spawn({
            let token = token.clone();
            async move {
                let mut interval = tokio::time::interval(tokio::time::Duration::from_micros(5));
                loop {
                    interval.tick().await;
                    process_uintr_wakers(&token);
                }
            }
        });

        let pending_requests_clone2 = pending_requests_clone.clone();
        tokio::spawn(async move {
            let mut response_buf = vec![0u8; 131072];
            
            info!("UINTR response listener thread started");
            
            while running_clone.load(Ordering::SeqCst) {
                debug!("Waiting for UINTR notification...");
                info!("Response listener: 等待 UINTR 响应通知...");
                
                match Self::wait_for_uintr().await {
                    Ok(_) => {
                        debug!("Received UINTR notification");
                        info!("Response listener: 收到 UINTR 响应通知");
                    },
                    Err(e) => {
                        warn!("UINTR wait error: {}, exiting response listener", e);
                        break;
                    }
                }
                
                let mut processed = 0;
                loop {
                    let n = match response_buffer_clone.read(&mut response_buf) {
                        Ok(n) => n,
                        Err(ShmError::BufferEmpty) => {
                            debug!("Response listener: 没有更多数据可读");
                            break;
                        },
                        Err(e) => {
                            warn!("Failed to read from shared memory: {}", e);
                            break;
                        }
                    };
                    
                    processed += 1;
                    debug!("Response listener: 读取到 {} 字节的数据", n);
                    
                    let (request_id, response): (String, EchoResponse) = 
                        match bincode::deserialize(&response_buf[..n]) {
                            Ok(data) => data,
                            Err(e) => {
                                warn!("Failed to deserialize response: {}", e);
                                continue;
                            }
                        };
                    
                    debug!("Response listener: 收到请求 {} 的响应", request_id);
                    
                    let mut pending = pending_requests_clone2.lock().await;
                    if let Some(tx) = pending.remove(&request_id) {
                        let _ = tx.send(Ok(response));
                        debug!("Response listener: 已将响应发送给等待的请求 {}", request_id);
                    } else {
                        warn!("No pending request for ID: {}", request_id);
                    }
                }
                
                if processed > 0 {
                    info!("Response listener: 处理了 {} 个响应", processed);
                }
            }
            
            error!("UINTR response listener thread exited!");
        });

        info!(
            "SHM transport with UINTR notification initialized (name: {}, UIPI index: {})",
            name, uipi_index
        );
        
        Ok(Self {
            name: name.to_string(),
            request_buffer,
            response_buffer,
            pending_requests,
            running,
        })
    }

    async fn setup_uintr(socket_path: &str) -> Result<libc::c_int> {
        init_token("client");
        
        let res = uintr_register_handler(ui_handler, UINTR_HANDLER_FLAG_WAITING_ANY)
            .map_err(|e| anyhow::anyhow!("Failed to register UINTR handler: {}", e))?;
        info!("UINTR client: Interrupt handler registered successfully: {}", res);

        let client_descriptor = uintr_create_fd(0, 0)
            .map_err(|e| anyhow::anyhow!("Failed to create uintrfd: {}", e))?;
        set_client_uintrfd(client_descriptor);
        info!(
            "UINTR client: Created uintrfd with descriptor {} (vector 0)",
            client_descriptor
        );

        unsafe {
            stui();
        }
        info!("UINTR client: Interrupts enabled");

        let server_fd = setup_client_connection(socket_path, get_client_uintrfd()).await
            .map_err(|e| anyhow::anyhow!("Failed to connect to UINTR server: {}", e))?;
        info!("UINTR client: Received server file descriptor {}", server_fd);

        let uipi_index = uintr_register_sender(server_fd, 0)
            .map_err(|e| anyhow::anyhow!("Failed to register UINTR sender: {}", e))?;
        set_client_uipi_index(uipi_index);
        info!("UINTR client: Registered sender for server with UIPI index {}", uipi_index);

        Ok(uipi_index)
    }

    fn send_uintr_notification(&self) -> Result<()> {
        let uipi_index = get_client_uipi_index();
        if uipi_index < 0 {
            return Err(anyhow::anyhow!("UINTR not initialized"));
        }

        debug!("Sending UINTR notification with UIPI index: {}", uipi_index);
        unsafe {
            senduipi(uipi_index as u64);
        }
        Ok(())
    }

    async fn wait_for_uintr() -> Result<()> {
        info!("wait_for_uintr: 开始异步等待 UINTR 中断...");
        uintr_wait().await?;
        info!("wait_for_uintr: 收到 UINTR 中断");
        Ok(())
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
                    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                }
            }
        }
    }
}

impl Drop for ShmTransportUintr {
    fn drop(&mut self) {
        self.running.store(false, Ordering::SeqCst);
        info!("ShmTransportUintr dropped");
    }
}

#[async_trait]
impl Transport for ShmTransportUintr {
    async fn call(&self, request: EchoRequest) -> Result<EchoResponse> {
        let request_id = Uuid::new_v4().to_string();
        info!("call(): 开始处理请求 {}", request_id);
        
        let (tx, rx) = oneshot::channel();
        {
            let mut pending = self.pending_requests.lock().await;
            pending.insert(request_id.clone(), tx);
            info!("call(): 已将请求 {} 添加到待处理列表", request_id);
        }
        
        let request_with_id = (request_id.clone(), request);
        let payload = bincode::serialize(&request_with_id)
            .map_err(|e| anyhow::anyhow!("Failed to serialize request: {}", e))?;
        
        info!("call(): 准备写入 {} 字节到共享内存，请求 {}", payload.len(), request_id);
        
        self.request_buffer.write(&payload)
            .map_err(|e| anyhow::anyhow!("Failed to write to shared memory: {}", e))?;
        
        info!("call(): 已将请求 {} 写入共享内存", request_id);
        
        self.send_uintr_notification()?;
        info!("call(): 已发送 UINTR 通知，请求 {}", request_id);
        
        info!("call(): 等待响应，请求 {}", request_id);
        
        let result = rx.await
            .map_err(|_| anyhow::anyhow!("Response channel closed for request {}", request_id))??;
        
        info!("call(): 收到响应，请求 {}", request_id);
        Ok(result)
    }
}
