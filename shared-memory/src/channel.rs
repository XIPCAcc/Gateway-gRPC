use std::sync::Arc;
use std::time::{Duration, Instant};

use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use tokio::sync::oneshot;
use tracing::trace;
use uuid::Uuid;

use crate::eventfd::EventFd;
use crate::shm::SharedMemoryRingBuffer;
use crate::{Result, ShmConfig, ShmError};

/// Message header for shared memory communication
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MessageHeader {
    pub id: String,
    pub timestamp_ns: u64,
    pub payload_len: usize,
}

/// Complete message structure
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    pub header: MessageHeader,
    pub payload: Vec<u8>,
}

impl Message {
    pub fn new(payload: Vec<u8>) -> Self {
        Self {
            header: MessageHeader {
                id: Uuid::new_v4().to_string(),
                timestamp_ns: Instant::now().elapsed().as_nanos() as u64,
                payload_len: payload.len(),
            },
            payload,
        }
    }

    pub fn to_bytes(&self) -> Result<Vec<u8>> {
        Ok(serde_json::to_vec(self)?)
    }

    pub fn from_bytes(data: &[u8]) -> Result<Self> {
        Ok(serde_json::from_slice(data)?)
    }
}

/// Shared memory channel for inter-process communication
pub struct ShmChannel {
    buffer: Arc<SharedMemoryRingBuffer>,
    event_fd: Arc<EventFd>,
    config: ShmConfig,
}

impl ShmChannel {
    pub fn create(name: &str, config: ShmConfig) -> Result<(Self, String)> {
        let buffer_name = format!("{}_buf", name);
        let _event_name = format!("{}_ev", name);

        // Clean up any existing shared memory
        let _ = SharedMemoryRingBuffer::unlink(&buffer_name);

        let buffer = Arc::new(SharedMemoryRingBuffer::create(&buffer_name, config)?);
        let event_fd = Arc::new(EventFd::new()?);

        let channel = Self {
            buffer,
            event_fd,
            config,
        };

        Ok((channel, buffer_name))
    }

    pub fn open(name: &str, config: ShmConfig) -> Result<Self> {
        let buffer_name = format!("{}_buf", name);

        let buffer = Arc::new(SharedMemoryRingBuffer::open(&buffer_name, config)?);
        let event_fd = Arc::new(EventFd::new()?);

        Ok(Self {
            buffer,
            event_fd,
            config,
        })
    }

    pub fn send(&self, msg: Message) -> Result<()> {
        let data = msg.to_bytes()?;
        
        if data.len() > self.config.max_message_size {
            return Err(ShmError::InvalidState);
        }

        self.buffer.write(&data)?;
        self.event_fd.signal()?;
        
        trace!("Sent message {} ({} bytes)", msg.header.id, data.len());
        Ok(())
    }

    pub fn try_recv(&self) -> Result<Option<Message>> {
        // Check if there's data available
        if self.buffer.available_to_read() == 0 {
            // Check eventfd for notification
            match self.event_fd.try_wait()? {
                Some(_) => {
                    // There was a notification, try reading
                }
                None => return Ok(None),
            }
        }

        let mut buf = vec![0u8; self.config.max_message_size];
        match self.buffer.read(&mut buf) {
            Ok(n) => {
                let msg = Message::from_bytes(&buf[..n])?;
                trace!("Received message {} ({} bytes)", msg.header.id, n);
                Ok(Some(msg))
            }
            Err(ShmError::BufferEmpty) => Ok(None),
            Err(e) => Err(e),
        }
    }

    pub fn recv_timeout(&self, timeout: Duration) -> Result<Option<Message>> {
        let start = Instant::now();
        
        loop {
            match self.try_recv()? {
                Some(msg) => return Ok(Some(msg)),
                None => {
                    if start.elapsed() >= timeout {
                        return Err(ShmError::Timeout);
                    }
                    std::thread::sleep(Duration::from_micros(1));
                }
            }
        }
    }

    pub fn event_fd(&self) -> Arc<EventFd> {
        self.event_fd.clone()
    }
}

/// Async wrapper for shared memory channel
pub mod async_channel {
    use super::*;
    use tokio::io::unix::AsyncFd;

    pub struct AsyncShmChannel {
        inner: ShmChannel,
        async_fd: AsyncFd<std::os::fd::RawFd>,
    }

    impl AsyncShmChannel {
        pub fn new(channel: ShmChannel) -> Result<Self> {
            let fd = channel.event_fd().as_raw_fd();
            let async_fd = AsyncFd::new(fd)?;
            
            Ok(Self {
                inner: channel,
                async_fd,
            })
        }

        pub async fn send(&self, msg: Message) -> Result<()> {
            self.inner.send(msg)
        }

        pub async fn recv(&self) -> Result<Message> {
            loop {
                // Try non-blocking read first
                match self.inner.try_recv()? {
                    Some(msg) => return Ok(msg),
                    None => {
                        // Wait for notification
                        let mut guard = self.async_fd.readable().await.map_err(|e| {
                            ShmError::Io(std::io::Error::new(std::io::ErrorKind::Other, e))
                        })?;
                        
                        // Clear the eventfd
                        self.inner.event_fd.clear()?;
                        guard.clear_ready();
                    }
                }
            }
        }
    }
}

/// Request-response channel with correlation IDs
pub struct RpcChannel {
    channel: ShmChannel,
    pending: Arc<Mutex<std::collections::HashMap<String, oneshot::Sender<Message>>>>,
}

impl RpcChannel {
    pub fn new(channel: ShmChannel) -> Self {
        Self {
            channel,
            pending: Arc::new(Mutex::new(std::collections::HashMap::new())),
        }
    }

    pub async fn call(&self, payload: Vec<u8>) -> Result<Message> {
        let id = Uuid::new_v4().to_string();
        let (tx, rx) = oneshot::channel();
        
        {
            let mut pending = self.pending.lock();
            pending.insert(id.clone(), tx);
        }

        let msg = Message {
            header: MessageHeader {
                id: id.clone(),
                timestamp_ns: Instant::now().elapsed().as_nanos() as u64,
                payload_len: payload.len(),
            },
            payload,
        };

        self.channel.send(msg)?;
        
        match rx.await {
            Ok(response) => Ok(response),
            Err(_) => Err(ShmError::InvalidState),
        }
    }

    pub fn handle_response(&self, msg: Message) -> bool {
        let mut pending = self.pending.lock();
        if let Some(tx) = pending.remove(&msg.header.id) {
            let _ = tx.send(msg);
            true
        } else {
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_message_serialization() {
        let msg = Message::new(b"Hello".to_vec());
        let bytes = msg.to_bytes().unwrap();
        let decoded = Message::from_bytes(&bytes).unwrap();
        
        assert_eq!(msg.payload, decoded.payload);
        assert_eq!(msg.header.id, decoded.header.id);
    }
}
