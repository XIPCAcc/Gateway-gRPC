use std::os::unix::io::RawFd;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use anyhow::Result;
use proto::{EchoRequest, EchoResponse, MatrixMultiplyResponse};
use shared_memory::shm::SharedMemoryRingBuffer;
use shared_memory::{ShmConfig, ShmError, ShmRequest, ShmResponse, MatrixDataPool, multiply_matrices, calculate_checksum, bytes_as_f64_slice, reconstruct_matrix};
use tracing::{info, warn, debug};
use nix::libc;

use uintr::syscall::{uintr_register_handler, uintr_create_fd, uintr_register_sender, senduipi, stui};
use uintr::connection::setup_server_connection;
use uintr::UINTR_HANDLER_FLAG_WAITING_ANY;
use uintr::async_wait::{init_token, uintr_wait, process_global_uintr_wakers, get_notify_count, get_wake_count};

unsafe extern "C" {
    pub fn ui_handler(ui_frame: *mut uintr::syscall::UintrFrame, vector: u64);
}

static mut SERVER_UINTRFD: RawFd = -1;
static mut SERVER_UIPI_INDEX: libc::c_int = -1;
static mut GLOBAL_REQUEST_COUNTER: u64 = 0;
static mut GLOBAL_RESPONSE_COUNTER: u64 = 0;
static mut PACKET_COUNTER: u64 = 0;
static mut LAST_NOTIFY_TIME: Option<std::time::Instant> = None;

fn get_server_uintrfd() -> RawFd {
    unsafe { SERVER_UINTRFD }
}

fn set_server_uintrfd(fd: RawFd) {
    unsafe {
        SERVER_UINTRFD = fd;
    }
}

fn get_server_uipi_index() -> libc::c_int {
    unsafe { SERVER_UIPI_INDEX }
}

fn set_server_uipi_index(index: libc::c_int) {
    unsafe {
        SERVER_UIPI_INDEX = index;
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

pub struct ShmServerUintr {
    name: String,
    request_buffer: SharedMemoryRingBuffer,
    response_buffer: Arc<SharedMemoryRingBuffer>,
    running: Arc<AtomicBool>,
    data_pool: MatrixDataPool,
}

impl ShmServerUintr {
    pub fn new(name: &str) -> Result<Self> {
        let config = ShmConfig::default();
        
        let request_buffer = SharedMemoryRingBuffer::create(&format!("{}_req_buf", name), config)
            .map_err(|e| anyhow::anyhow!("Failed to create request buffer: {}", e))?;
        let response_buffer = SharedMemoryRingBuffer::create(&format!("{}_resp_buf", name), config)
            .map_err(|e| anyhow::anyhow!("Failed to create response buffer: {}", e))?;
        
        let running = Arc::new(AtomicBool::new(true));
        
        let data_pool_name = format!("{}_matrix_data", name);
        let data_pool_capacity = 256 * 1024 * 1024;
        let data_pool = MatrixDataPool::create(&data_pool_name, data_pool_capacity)
            .map_err(|e| anyhow::anyhow!("Failed to create matrix data pool: {}", e))?;
        
        info!(
            "SHM server with UINTR notification initialized (name: {})",
            name
        );
        
        Ok(Self {
            name: name.to_string(),
            request_buffer,
            response_buffer: Arc::new(response_buffer),
            running,
            data_pool,
        })
    }

    pub async fn run(mut self, delay: Duration) -> Result<()> {
        info!("SHM server started, setting up UINTR...");
        
        let socket_path = format!("/tmp/{}_uintr.sock", self.name);
        let _ = std::fs::remove_file(&socket_path);
        
        let response_buffer = self.response_buffer.clone();
        let request_buffer = Arc::new(self.request_buffer);
        let running = self.running.clone();
        let mut request_buf = vec![0u8; 65536];
        
        let (done_tx, done_rx) = tokio::sync::oneshot::channel();
        let running_clone = running.clone();
        let socket_path_for_blocking = socket_path.clone();
        
        let uintr_handle = tokio::task::spawn_blocking(move || {
            info!("UINTR blocking thread: initializing...");
            
            init_token("server");
            
            match uintr_register_handler(ui_handler, UINTR_HANDLER_FLAG_WAITING_ANY) {
                Ok(res) => info!("UINTR blocking thread: handler registered: {}", res),
                Err(e) => {
                    warn!("UINTR blocking thread: failed to register handler: {}", e);
                    let _ = done_tx.send(());
                    return;
                }
            }
            
            let server_fd = match uintr_create_fd(0, 0) {
                Ok(fd) => {
                    set_server_uintrfd(fd);
                    info!("UINTR blocking thread: created uintrfd {}", fd);
                    fd
                }
                Err(e) => {
                    warn!("UINTR blocking thread: failed to create uintrfd: {}", e);
                    let _ = done_tx.send(());
                    return;
                }
            };

            info!("UINTR blocking thread: waiting for gateway connection...");
            let client_fd = match setup_server_connection(&socket_path_for_blocking, server_fd) {
                Ok(fd) => fd,
                Err(e) => {
                    warn!("UINTR blocking thread: failed to wait for client: {}", e);
                    let _ = done_tx.send(());
                    return;
                }
            };
            info!("UINTR blocking thread: gateway connected, client_fd={}", client_fd);

            let uipi_index = match uintr_register_sender(client_fd, 0) {
                Ok(idx) => idx,
                Err(e) => {
                    warn!("UINTR blocking thread: failed to register sender: {}", e);
                    let _ = done_tx.send(());
                    return;
                }
            };
            set_server_uipi_index(uipi_index);
            info!("UINTR blocking thread: sender registered, uipi_index={}", uipi_index);
            
            unsafe { stui(); }
            info!("UINTR blocking thread: interrupts enabled");

            let _ = done_tx.send(());
            
            info!("UINTR blocking thread: entering wait loop");
            let mut last_notify = get_notify_count();
            let mut last_wake = get_wake_count();
            while running_clone.load(Ordering::SeqCst) {
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
                // loop {
                    // notify_global_uintr().unwrap();
                // }
            }
            info!("UINTR blocking thread: exited");
        });
        
        done_rx.await
            .map_err(|e| anyhow::anyhow!("UINTR blocking thread failed to initialize: {}", e))?;
        
        tokio::spawn(async move {
            info!("Request listener thread started");
            
                while running.load(Ordering::SeqCst) {
                    debug!("Waiting for UINTR request notification...");
                    
                    match Self::wait_for_uintr().await {
                        Ok(_) => {
                            debug!("Received UINTR request notification");
                        },
                        Err(e) => {
                            warn!("UINTR wait error: {}, exiting request listener", e);
                            break;
                        }
                    }
                    
                    // 处理所有待处理的请求
                    let mut processed = 0;
                    loop {
                        let n = match request_buffer.read(&mut request_buf) {
                            Ok(n) => {
                                let global_req_id = unsafe {
                                    GLOBAL_REQUEST_COUNTER += 1;
                                    GLOBAL_REQUEST_COUNTER
                                };
                                debug!("Request listener: 成功从共享内存读取 {} 字节，请求全局编号: {}", n, global_req_id);
                                n
                            },
                            Err(ShmError::BufferEmpty) => {
                                debug!("Request listener: 没有更多请求可读");
                                break;
                            },
                            Err(e) => {
                                warn!("Failed to read from shared memory: {}", e);
                                break;
                            }
                        };
                        
                        processed += 1;
                        
                        let shm_request: ShmRequest = 
                            match bincode::deserialize::<ShmRequest>(&request_buf[..n]) {
                                Ok(data) => {
                                    debug!("Request listener: 成功解析 ShmRequest");
                                    data
                                },
                                Err(e) => {
                                    warn!("Failed to parse ShmRequest: {}", e);
                                    continue;
                                }
                            };
                        
                        let response_buffer_clone = response_buffer.clone();
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
                                
                                let request_id_clone = request_id.clone();
                                
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
                                        id: request_id_clone.clone(),
                                        response: response_bytes,
                                    };

                                    let global_resp_id = unsafe {
                                        GLOBAL_RESPONSE_COUNTER += 1;
                                        GLOBAL_RESPONSE_COUNTER
                                    };
                                    debug!("Response listener: 准备序列化响应，响应全局编号: {}", global_resp_id);

                                    let payload = match bincode::serialize(&shm_response) {
                                        Ok(data) => data,
                                        Err(e) => {
                                            warn!("Failed to serialize response: {}", e);
                                            return;
                                        }
                                    };
                                    
                                    let was_empty = response_buffer_clone.is_empty();
                                    
                                    if let Err(e) = response_buffer_clone.write(&payload) {
                                        warn!("Failed to write response: {}", e);
                                        return;
                                    }
                                    
                                    let packet_count = increment_packet_counter();

                                    let buffer_data_len = response_buffer_clone.available_data();
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

                                    if let Err(e) = Self::send_uintr_notification() {
                                        warn!("Failed to send UINTR notification: {}", e);
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
                                    
                                    if let Err(e) = Self::send_uintr_notification() {
                                        warn!("Failed to send UINTR notification: {}", e);
                                    }
                                });
                            }
                        }
                    }
                    
                    if processed > 0 {
                        info!("Request listener: 处理了 {} 个请求", processed);
                    }
                }
            
            info!("Request listener thread exited");
        });

        tokio::signal::ctrl_c().await?;
        info!("Shutting down SHM server");
        self.running.store(false, Ordering::SeqCst);

        let _ = uintr_handle.await;
        
        Ok(())
    }

    fn send_uintr_notification() -> Result<()> {
        let uipi_index = get_server_uipi_index();
        if uipi_index < 0 {
            return Err(anyhow::anyhow!("UINTR not initialized"));
        }

        info!(">>> send_uintr_notification: sending UIPI index={}", uipi_index);
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
}
