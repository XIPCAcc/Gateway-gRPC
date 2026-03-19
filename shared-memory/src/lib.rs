pub mod eventfd;
pub mod shm;
pub mod uintr;
pub mod channel;

pub use eventfd::*;
pub use shm::*;
pub use channel::*;

use thiserror::Error;

#[derive(Error, Debug)]
pub enum ShmError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    
    #[error("Nix error: {0}")]
    Nix(#[from] nix::Error),
    
    #[error("Serialization error: {0}")]
    Serialization(#[from] serde_json::Error),
    
    #[error("Buffer full")]
    BufferFull,
    
    #[error("Buffer empty")]
    BufferEmpty,
    
    #[error("Invalid state")]
    InvalidState,
    
    #[error("Timeout")]
    Timeout,
}

pub type Result<T> = std::result::Result<T, ShmError>;

/// Shared memory configuration
#[derive(Debug, Clone, Copy)]
pub struct ShmConfig {
    pub buffer_size: usize,
    pub max_message_size: usize,
    pub timeout_ms: u64,
}

impl Default for ShmConfig {
    fn default() -> Self {
        Self {
            buffer_size: 1024 * 1024, // 1MB
            max_message_size: 64 * 1024, // 64KB
            timeout_ms: 5000,
        }
    }
}
