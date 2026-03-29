// UINTR 服务器实现
// 用于 backend 和 gateway 之间的用户态中断通信

use std::os::unix::io::RawFd;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use tracing::{info, warn, error};
use nix::libc;

use uintr::{UintrError, UintrResult};
use uintr::syscall::{uintr_register_handler, uintr_create_fd, uintr_register_sender, senduipi, stui, uintr_wait};
use uintr::connection::setup_server_connection;
use uintr::{UINTR_HANDLER_FLAG_WAITING_ANY, UINTR_WAIT_MAX_USEC};

// 声明C语言中断处理程序和全局变量
unsafe extern "C" {
    pub fn ui_handler(ui_frame: *mut uintr::syscall::UintrFrame, vector: u64);
    static mut uintr_received: libc::c_ulong;
}

// 全局状态
static mut SERVER_UINTRFD: RawFd = -1;
static mut CLIENT_UINTRFD: RawFd = -1;
static mut SERVER_UIPI_INDEX: libc::c_int = -1;
static TEST_DONE: AtomicBool = AtomicBool::new(false);

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

/// UINTR 服务器
pub struct UintrServer {
    socket_path: String,
    running: Arc<AtomicBool>,
}

impl UintrServer {
    /// 创建新的 UINTR 服务器
    pub fn new(socket_path: &str) -> Self {
        Self {
            socket_path: socket_path.to_string(),
            running: Arc::new(AtomicBool::new(false)),
        }
    }

    /// 初始化 UINTR 服务器
    pub async fn initialize(&mut self) -> UintrResult<()> {
        // 注册中断处理程序
        let res = uintr_register_handler(ui_handler, UINTR_HANDLER_FLAG_WAITING_ANY)?;
        info!("UINTR server: Interrupt handler registered successfully: {}", res);

        // 创建服务器 uintrfd 文件描述符
        let server_descriptor = uintr_create_fd(0, 0)?;
        set_server_uintrfd(server_descriptor);
        info!(
            "UINTR server: Created uintrfd with descriptor {} (vector 0)",
            server_descriptor
        );

        // 启用中断
        unsafe {
            stui();
        }
        info!("UINTR server: Interrupts enabled");

        Ok(())
    }

    /// 启动 UINTR 服务器
    pub async fn start(&mut self) -> UintrResult<()> {
        self.running.store(true, Ordering::SeqCst);

        // 清理旧的 socket 文件
        let _ = std::fs::remove_file(&self.socket_path);

        // 等待客户端连接
        info!("UINTR server: Waiting for client connection...");
        let client_fd = setup_server_connection(&self.socket_path, get_server_uintrfd()).await?;
        info!("UINTR server: Received client file descriptor {}", client_fd);

        // 注册发送者
        let uipi_index = uintr_register_sender(client_fd, 0)?;
        set_server_uipi_index(uipi_index);
        info!("UINTR server: Registered sender for client with UIPI index {}", uipi_index);

        Ok(())
    }

    /// 发送中断到客户端
    pub fn send_interrupt(&self) -> UintrResult<()> {
        let uipi_index = get_server_uipi_index();
        if uipi_index < 0 {
            return Err(UintrError::NotInitialized);
        }

        info!("UINTR server: Sending interrupt with UIPI index: {}", uipi_index);
        unsafe {
            senduipi(uipi_index as u64);
        }
        Ok(())
    }

    /// 等待客户端中断
    pub fn wait_interrupt(&self) -> UintrResult<()> {
        info!("UINTR server: Waiting for client interrupt...");
        while unsafe { uintr_received == 0 } && self.running.load(Ordering::SeqCst) {
            uintr_wait(UINTR_WAIT_MAX_USEC, 0)?;
        }
        unsafe { uintr_received = 0; }
        info!("UINTR server: Received client interrupt");
        Ok(())
    }

    /// 停止 UINTR 服务器
    pub fn stop(&mut self) {
        self.running.store(false, Ordering::SeqCst);
        TEST_DONE.store(true, Ordering::SeqCst);
        // 清理 socket 文件
        let _ = std::fs::remove_file(&self.socket_path);
        info!("UINTR server: Stopped");
    }

    /// 运行测试
    pub async fn run_test(&mut self, message_count: usize) -> UintrResult<()> {
        self.initialize().await?;
        self.start().await?;

        info!("UINTR server: Starting test with {} messages", message_count);

        for i in 1..=message_count {
            info!("UINTR server: Sending message #{}", i);
            self.send_interrupt()?;
            info!("UINTR server: Waiting for response #{}", i);
            self.wait_interrupt()?;
            info!("UINTR server: Received response #{}", i);
        }

        info!("UINTR server: Test completed");
        self.stop();

        Ok(())
    }
}
