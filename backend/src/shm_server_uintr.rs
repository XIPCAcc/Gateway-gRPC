use std::os::unix::io::RawFd;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use anyhow::Result;
use proto::{EchoRequest, EchoResponse};
use shared_memory::shm::SharedMemoryRingBuffer;
use shared_memory::{ShmConfig, ShmError};
use tokio::sync::Mutex;
use tracing::{info, warn, debug};
use nix::libc;

use uintr::{UintrError, UintrResult};
use uintr::syscall::{uintr_register_handler, uintr_create_fd, uintr_register_sender, senduipi, stui};
use uintr::connection::setup_server_connection;
use uintr::UINTR_HANDLER_FLAG_WAITING_ANY;
use uintr::async_wait::{init_token, get_token, process_uintr_wakers, uintr_wait};

unsafe extern "C" {
    pub fn ui_handler(ui_frame: *mut uintr::syscall::UintrFrame, vector: u64);
    static mut uintr_received: libc::c_ulong;
}

static mut SERVER_UINTRFD: RawFd = -1;
static mut SERVER_UIPI_INDEX: libc::c_int = -1;

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

pub struct ShmServerUintr {
    name: String,
    request_buffer: SharedMemoryRingBuffer,
    response_buffer: Arc<SharedMemoryRingBuffer>,
    running: Arc<AtomicBool>,
}

impl ShmServerUintr {
    pub fn new(name: &str) -> Result<Self> {
        let config = ShmConfig::default();
        
        let request_buffer = SharedMemoryRingBuffer::create(&format!("{}_req_buf", name), config)
            .map_err(|e| anyhow::anyhow!("Failed to create request buffer: {}", e))?;
        let response_buffer = SharedMemoryRingBuffer::create(&format!("{}_resp_buf", name), config)
            .map_err(|e| anyhow::anyhow!("Failed to create response buffer: {}", e))?;
        
        let running = Arc::new(AtomicBool::new(true));
        
        info!(
            "SHM server with UINTR notification initialized (name: {})",
            name
        );
        
        Ok(Self {
            name: name.to_string(),
            request_buffer,
            response_buffer: Arc::new(response_buffer),
            running,
        })
    }

    async fn setup_uintr(&self) -> Result<libc::c_int> {
        init_token("server");
        
        let res = uintr_register_handler(ui_handler, UINTR_HANDLER_FLAG_WAITING_ANY)
            .map_err(|e| anyhow::anyhow!("Failed to register UINTR handler: {}", e))?;
        info!("UINTR server: Interrupt handler registered successfully: {}", res);

        let server_descriptor = uintr_create_fd(0, 0)
            .map_err(|e| anyhow::anyhow!("Failed to create uintrfd: {}", e))?;
        set_server_uintrfd(server_descriptor);
        info!(
            "UINTR server: Created uintrfd with descriptor {} (vector 0)",
            server_descriptor
        );

        unsafe {
            stui();
        }
        info!("UINTR server: Interrupts enabled");

        Ok(res)
    }
    
    pub async fn run(mut self, delay: Duration) -> Result<()> {
        info!("SHM server started, setting up UINTR...");
        
        let socket_path = format!("/tmp/{}_uintr.sock", self.name);
        let _ = std::fs::remove_file(&socket_path);
        
        self.setup_uintr().await?;
        
        info!("Waiting for gateway connection...");
        let client_fd = setup_server_connection(&socket_path, get_server_uintrfd()).await
            .map_err(|e| anyhow::anyhow!("Failed to wait for client: {}", e))?;
        info!("Gateway connected via UINTR");

        let uipi_index = uintr_register_sender(client_fd, 0)
            .map_err(|e| anyhow::anyhow!("Failed to register UINTR sender: {}", e))?;
        set_server_uipi_index(uipi_index);
        info!("UINTR server: Registered sender for client with UIPI index {}", uipi_index);

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

        let response_buffer = self.response_buffer.clone();
        let request_buffer = Arc::new(self.request_buffer);
        let running = self.running.clone();
        let mut request_buf = vec![0u8; 65536];
        
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
                                debug!("Request listener: 成功从共享内存读取 {} 字节", n);
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
                        
                        let (request_id, request): (String, EchoRequest) = 
                            match bincode::deserialize::<(String, EchoRequest)>(&request_buf[..n]) {
                                Ok(data) => {
                                    debug!("Request listener: 成功解析请求，请求 ID: {}", data.0);
                                    data
                                },
                                Err(e) => {
                                    warn!("Failed to parse request: {}", e);
                                    continue;
                                }
                            };
                        
                        debug!("Processing request {}", request_id);
                        
                        let response_buffer_clone = response_buffer.clone();
                        let delay_clone = delay;
                        let request_id_clone = request_id.clone();
                        
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
                            
                            let response_with_id = (request_id_clone.clone(), response);
                            
                            let payload = match bincode::serialize(&response_with_id) {
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
                    
                    if processed > 0 {
                        info!("Request listener: 处理了 {} 个请求", processed);
                    }
                }
            
            info!("Request listener thread exited");
        });
        
        tokio::signal::ctrl_c().await?;
        info!("Shutting down SHM server");
        self.running.store(false, Ordering::SeqCst);
        
        Ok(())
    }

    fn send_uintr_notification() -> Result<()> {
        let uipi_index = get_server_uipi_index();
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
}
