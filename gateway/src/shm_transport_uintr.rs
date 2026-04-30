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
use uintr::async_wait::{init_token, get_token, uintr_wait};

type PendingRequests = Arc<Mutex<HashMap<String, oneshot::Sender<Result<EchoResponse>>>>>;

unsafe extern "C" {
    pub fn ui_handler(ui_frame: *mut uintr::syscall::UintrFrame, vector: u64);
}

static mut CLIENT_UINTRFD: RawFd = -1;
static mut CLIENT_UIPI_INDEX: libc::c_int = -1;
static mut GLOBAL_REQUEST_COUNTER: u64 = 0;
static mut GLOBAL_RESPONSE_COUNTER: u64 = 0;
static mut PACKET_COUNTER: u64 = 0;
static mut LAST_NOTIFY_TIME: Option<std::time::Instant> = None;

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

fn get_packet_counter() -> u64 {
    unsafe { PACKET_COUNTER }
}

fn set_packet_counter(counter: u64) {
    unsafe {
        PACKET_COUNTER = counter;
    }
}

fn increment_packet_counter() -> u64 {
    unsafe {
        PACKET_COUNTER += 1;
        PACKET_COUNTER
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
                    let global_resp_id = unsafe {
                        GLOBAL_RESPONSE_COUNTER += 1;
                        GLOBAL_RESPONSE_COUNTER
                    };
                    debug!("Response listener: 读取到 {} 字节的数据，响应全局编号: {}", n, global_resp_id);
                    
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

        let was_empty = self.request_buffer.is_empty();

        info!("call(): 准备写入 {} 字节到共享内存，请求 {}", payload.len(), request_id);

        self.request_buffer.write(&payload)
            .map_err(|e| anyhow::anyhow!("Failed to write to shared memory: {}", e))?;

        let global_req_id = unsafe {
            GLOBAL_REQUEST_COUNTER += 1;
            GLOBAL_REQUEST_COUNTER
        };
        let packet_count = increment_packet_counter();
        
        // 自适应中断策略
        let buffer_data_len = self.request_buffer.available_data();
        let now = std::time::Instant::now();

        // 获取上一次通知时间
        let last_notify = unsafe { LAST_NOTIFY_TIME };
        let time_since_last_notify = last_notify.map(|t| now.duration_since(t)).unwrap_or(std::time::Duration::from_secs(u64::MAX));

        // 触发条件（满足任一即可）：
        // 1. buffer为空（新批次开始）
        // 2. 距离上一次通知超过10ms（防止饥饿）
        // 3. 包数超过20个（保底）
        // 4. buffer中数据超过1MB（防止积压）
        let should_notify = was_empty 
            || time_since_last_notify >= std::time::Duration::from_millis(10)
            || packet_count >= 20
            || buffer_data_len >= 1024 * 1024;

        if should_notify {
            set_packet_counter(0);
            // 更新上一次通知时间
            unsafe {
                LAST_NOTIFY_TIME = Some(now);
            }
        }
        
        info!(
            "call(): 已将请求 {} (全局编号: {}) 写入共享内存, was_empty={}, packet_count={}, buffer_len={}, time_since_notify={:?}", 
            request_id, global_req_id, was_empty, packet_count, buffer_data_len, time_since_last_notify
        );
        self.send_uintr_notification()?;
        // if should_notify {
        //     self.send_uintr_notification()?;
        //     info!("call(): 已发送 UINTR 通知，请求 {}", request_id);
        // } else {
        //     debug!("call(): 跳过 UINTR 通知，请求 {}", request_id);
        // }

        info!("call(): 等待响应，请求 {}", request_id);

        let result = rx.await
            .map_err(|_| anyhow::anyhow!("Response channel closed for request {}", request_id))??;

        info!("call(): 收到响应，请求 {}", request_id);
        Ok(result)
    }
}
