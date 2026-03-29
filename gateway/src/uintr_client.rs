// UINTR 客户端实现
// 用于 gateway 和 backend 之间的用户态中断通信

use std::os::unix::io::RawFd;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use tracing::{info, warn, error};
use nix::libc;

use uintr::{UintrError, UintrResult};
use uintr::syscall::{uintr_register_handler, uintr_create_fd, uintr_register_sender, senduipi, stui, uintr_wait};
use uintr::connection::setup_client_connection;
use uintr::{UINTR_HANDLER_FLAG_WAITING_ANY, UINTR_WAIT_MAX_USEC};

// 声明C语言中断处理程序和全局变量
unsafe extern "C" {
    pub fn ui_handler(ui_frame: *mut uintr::syscall::UintrFrame, vector: u64);
    static mut uintr_received: libc::c_ulong;
}

// 全局状态
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

/// UINTR 客户端
pub struct UintrClient {
    socket_path: String,
    running: Arc<AtomicBool>,
}

impl UintrClient {
    /// 创建新的 UINTR 客户端
    pub fn new(socket_path: &str) -> Self {
        Self {
            socket_path: socket_path.to_string(),
            running: Arc::new(AtomicBool::new(false)),
        }
    }

    /// 初始化 UINTR 客户端
    pub async fn initialize(&mut self) -> UintrResult<()> {
        // 注册中断处理程序
        let res = uintr_register_handler(ui_handler, UINTR_HANDLER_FLAG_WAITING_ANY)?;
        info!("UINTR client: Interrupt handler registered successfully: {}", res);

        // 创建客户端 uintrfd 文件描述符
        let client_descriptor = uintr_create_fd(0, 0)?;
        set_client_uintrfd(client_descriptor);
        info!(
            "UINTR client: Created uintrfd with descriptor {} (vector 0)",
            client_descriptor
        );

        // 启用中断
        unsafe {
            stui();
        }
        info!("UINTR client: Interrupts enabled");

        Ok(())
    }

    /// 连接到 UINTR 服务器
    pub async fn connect(&mut self) -> UintrResult<()> {
        self.running.store(true, Ordering::SeqCst);

        // 连接到服务器的 Unix Domain Socket
        info!("UINTR client: Connecting to server at {}", self.socket_path);
        let server_fd = setup_client_connection(&self.socket_path, get_client_uintrfd()).await?;
        info!("UINTR client: Received server file descriptor {}", server_fd);

        // 注册发送者
        let uipi_index = uintr_register_sender(server_fd, 0)?;
        set_client_uipi_index(uipi_index);
        info!("UINTR client: Registered sender for server with UIPI index {}", uipi_index);

        Ok(())
    }

    /// 发送中断到服务器
    pub fn send_interrupt(&self) -> UintrResult<()> {
        let uipi_index = get_client_uipi_index();
        if uipi_index < 0 {
            return Err(UintrError::NotInitialized);
        }

        info!("UINTR client: Sending interrupt with UIPI index: {}", uipi_index);
        unsafe {
            senduipi(uipi_index as u64);
        }
        Ok(())
    }

    /// 等待服务器中断
    pub fn wait_interrupt(&self) -> UintrResult<()> {
        info!("UINTR client: Waiting for server interrupt...");
        while unsafe { uintr_received == 0 } && self.running.load(Ordering::SeqCst) {
            uintr_wait(UINTR_WAIT_MAX_USEC, 0)?;
        }
        unsafe { uintr_received = 0; }
        info!("UINTR client: Received server interrupt");
        Ok(())
    }

    /// 停止 UINTR 客户端
    pub fn stop(&mut self) {
        self.running.store(false, Ordering::SeqCst);
        info!("UINTR client: Stopped");
    }

    /// 运行测试
    pub async fn run_test(&mut self, message_count: usize) -> UintrResult<()> {
        self.initialize().await?;
        self.connect().await?;

        info!("UINTR client: Starting test with {} messages", message_count);

        for i in 1..=message_count {
            info!("UINTR client: Waiting for message #{}", i);
            self.wait_interrupt()?;
            info!("UINTR client: Received message #{}", i);
            info!("UINTR client: Sending response #{}", i);
            self.send_interrupt()?;
        }

        info!("UINTR client: Test completed");
        self.stop();

        Ok(())
    }
}
