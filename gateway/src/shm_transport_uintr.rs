use std::collections::HashMap;
use std::os::unix::io::RawFd;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use anyhow::Result;
use async_trait::async_trait;
use proto::{EchoRequest, EchoResponse, MatrixMultiplyRequest, MatrixMultiplyResponse};
use shared_memory::shm::SharedMemoryRingBuffer;
use shared_memory::{ShmConfig, ShmError, ShmRequest, ShmResponse, MatrixDataPool, flatten_matrix, f64_slice_as_bytes};
use tokio::sync::{Mutex, oneshot};
use tracing::{info, warn, error, debug};
use uuid::Uuid;
use nix::libc;

// 日志采样计数器和采样间隔
static LATENCY_LOG_COUNTER: AtomicUsize = AtomicUsize::new(0);
const LATENCY_LOG_INTERVAL: usize = 10;

use crate::transport::Transport;

use uintr::syscall::{uintr_register_handler, uintr_create_fd, uintr_register_sender, senduipi, stui};
use uintr::connection::setup_client_connection;
use uintr::UINTR_HANDLER_FLAG_WAITING_ANY;
use uintr::async_wait::{init_token, get_token, uintr_wait, process_global_uintr_wakers, get_notify_count, get_wake_count};

enum PendingRequest {
    Echo(oneshot::Sender<Result<EchoResponse>>),
    MatrixMultiply(oneshot::Sender<Result<MatrixMultiplyResponse>>),
}

type PendingRequests = Arc<Mutex<HashMap<String, PendingRequest>>>;

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
    data_pool: Arc<MatrixDataPool>,
    _uintr_handle: tokio::task::JoinHandle<()>,
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
        let running_clone2 = running.clone();

        let (done_tx, done_rx) = oneshot::channel();

        let uintr_handle = tokio::task::spawn_blocking(move || {
            init_token("client");

            match uintr_register_handler(ui_handler, UINTR_HANDLER_FLAG_WAITING_ANY) {
                Ok(res) => info!("UINTR blocking thread: handler registered: {}", res),
                Err(e) => {
                    warn!("UINTR blocking thread: failed to register handler: {}", e);
                    let _ = done_tx.send(());
                    return;
                }
            }

            let client_fd = match uintr_create_fd(0, 0) {
                Ok(fd) => fd,
                Err(e) => {
                    warn!("UINTR blocking thread: failed to create uintrfd: {}", e);
                    let _ = done_tx.send(());
                    return;
                }
            };
            set_client_uintrfd(client_fd);
            info!("UINTR blocking thread: uintrfd created: {}", client_fd);

            let server_fd = match setup_client_connection(&socket_path, client_fd) {
                Ok(fd) => fd,
                Err(e) => {
                    warn!("UINTR blocking thread: failed to connect to server: {}", e);
                    let _ = done_tx.send(());
                    return;
                }
            };
            info!("UINTR blocking thread: connected to server, server_fd={}", server_fd);

            let uipi_index = match uintr_register_sender(server_fd, 0) {
                Ok(idx) => idx,
                Err(e) => {
                    warn!("UINTR blocking thread: failed to register sender: {}", e);
                    let _ = done_tx.send(());
                    return;
                }
            };
            set_client_uipi_index(uipi_index);
            info!("UINTR blocking thread: sender registered, uipi_index={}", uipi_index);

            unsafe { stui(); }
            info!("UINTR blocking thread: interrupts enabled");

            let _ = done_tx.send(());

            let mut last_notify = get_notify_count();
            let mut last_wake = get_wake_count();

            while running_clone2.load(Ordering::SeqCst) {
                match uintr::syscall::uintr_wait(uintr::UINTR_WAIT_MAX_USEC, 0) {
                    Ok(true) => {
                        let cur_notify = get_notify_count();
                        debug!(
                            "UINTR blocking wait: interrupt received (notify_cnt: {} -> {}, +{})",
                            last_notify, cur_notify, cur_notify - last_notify
                        );
                        last_notify = cur_notify;
                        let woken = process_global_uintr_wakers();
                        let cur_wake = get_wake_count();
                        info!(
                            "UINTR: process_global_uintr_wakers returned {} (wake_cnt: {} -> {}, +{})",
                            woken, last_wake, cur_wake, cur_wake - last_wake
                        );
                        last_wake = cur_wake;
                    }
                    Ok(false) => {
                        debug!("UINTR blocking wait: timeout (no interrupt in window)");
                    }
                    Err(e) => {
                        warn!("UINTR blocking wait error: {}, exiting", e);
                        break;
                    }
                }
            }
            info!("UINTR blocking wait task exited");
        });

        done_rx
            .await
            .map_err(|_| anyhow::anyhow!("UINTR blocking thread failed to initialize"))?;

        let _token = get_token()
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

                    let shm_response: ShmResponse =
                        match bincode::deserialize(&response_buf[..n]) {
                            Ok(data) => data,
                            Err(e) => {
                                warn!("Failed to deserialize response: {}", e);
                                continue;
                            }
                        };

                    let mut pending = pending_requests_clone2.lock().await;
                    match shm_response {
                        ShmResponse::Echo { id: request_id, response: response_bytes } => {
                            let response: EchoResponse = match bincode::deserialize(&response_bytes) {
                                Ok(r) => r,
                                Err(e) => {
                                    warn!("Failed to deserialize EchoResponse: {}", e);
                                    continue;
                                }
                            };
                            debug!("Response listener: 收到请求 {} 的 Echo 响应", request_id);
                            if let Some(PendingRequest::Echo(tx)) = pending.remove(&request_id) {
                                let _ = tx.send(Ok(response));
                                debug!("Response listener: 已将响应发送给等待的请求 {}", request_id);
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
                            debug!("Response listener: 收到请求 {} 的 MatrixMultiply 响应", request_id);
                            if let Some(PendingRequest::MatrixMultiply(tx)) = pending.remove(&request_id) {
                                let _ = tx.send(Ok(response));
                                debug!("Response listener: 已将响应发送给等待的请求 {}", request_id);
                            } else {
                                warn!("No pending MatrixMultiply request for ID: {}", request_id);
                            }
                        }
                    }
                }

                if processed > 0 {
                    info!("Response listener: 处理了 {} 个响应", processed);
                }
            }

            error!("UINTR response listener thread exited!");
        });

        info!(
            "SHM transport with UINTR notification initialized (name: {})",
            name
        );

        let data_pool_name = format!("{}_matrix_data", name);
        let data_pool = Self::wait_for_data_pool(&data_pool_name).await?;

        Ok(Self {
            name: name.to_string(),
            request_buffer,
            response_buffer,
            pending_requests,
            running,
            data_pool: Arc::new(data_pool),
            _uintr_handle: uintr_handle,
        })
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
            pending.insert(request_id.clone(), PendingRequest::Echo(tx));
            info!("call(): 已将请求 {} 添加到待处理列表", request_id);
        }

        let request_bytes = bincode::serialize(&request)
            .map_err(|e| anyhow::anyhow!("Failed to serialize request: {}", e))?;
        let shm_request = ShmRequest::Echo {
            id: request_id.clone(),
            request: request_bytes,
        };
        let payload = bincode::serialize(&shm_request)
            .map_err(|e| anyhow::anyhow!("Failed to serialize ShmRequest: {}", e))?;

        let was_empty = self.request_buffer.is_empty();

        info!("call(): 准备写入 {} 字节到共享内存，请求 {}", payload.len(), request_id);

        self.request_buffer.write(&payload)
            .map_err(|e| anyhow::anyhow!("Failed to write to shared memory: {}", e))?;

        let global_req_id = unsafe {
            GLOBAL_REQUEST_COUNTER += 1;
            GLOBAL_REQUEST_COUNTER
        };
        let packet_count = increment_packet_counter();
        
        let buffer_data_len = self.request_buffer.available_data();
        let now = std::time::Instant::now();

        let last_notify = unsafe { LAST_NOTIFY_TIME };
        let time_since_last_notify = last_notify.map(|t| now.duration_since(t)).unwrap_or(std::time::Duration::from_secs(u64::MAX));

        let should_notify = was_empty 
            || time_since_last_notify >= std::time::Duration::from_millis(10)
            || packet_count >= 20
            || buffer_data_len >= 1024 * 1024;

        if should_notify {
            set_packet_counter(0);
            unsafe {
                LAST_NOTIFY_TIME = Some(now);
            }
        }
        
        info!(
            "call(): 已将请求 {} (全局编号: {}) 写入共享内存, was_empty={}, packet_count={}, buffer_len={}, time_since_notify={:?}", 
            request_id, global_req_id, was_empty, packet_count, buffer_data_len, time_since_last_notify
        );
        self.send_uintr_notification()?;

        info!("call(): 等待响应，请求 {}", request_id);

        let result = rx.await
            .map_err(|_| anyhow::anyhow!("Response channel closed for request {}", request_id))??;

        info!("call(): 收到响应，请求 {}", request_id);
        Ok(result)
    }

    async fn matrix_multiply(&self, request: MatrixMultiplyRequest) -> Result<MatrixMultiplyResponse> {
        let request_id = Uuid::new_v4().to_string();
        info!("matrix_multiply(): 开始处理请求 {}", request_id);
        
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

        // 记录发送请求前的时间戳
        let start_time = std::time::Instant::now();
        
        self.send_uintr_notification()?;

        let mut result = rx.await
            .map_err(|_| anyhow::anyhow!("Response channel closed for request {}", request_id))??;

        // 计算 gateway 到 backend 的往返延迟
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
