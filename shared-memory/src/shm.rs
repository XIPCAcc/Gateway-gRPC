use std::fs::OpenOptions;

use memmap2::{MmapMut, MmapOptions};
use nix::fcntl::OFlag;
use nix::sys::mman::{shm_open, shm_unlink};
use nix::sys::stat::Mode;
use nix::unistd::ftruncate;

use crate::{Result, ShmConfig};

/// Ring buffer header stored in shared memory
#[repr(C)]
#[derive(Debug)]
pub struct RingBufferHeader {
    /// Write position (atomic)
    pub write_pos: std::sync::atomic::AtomicU64,
    /// Read position (atomic)
    pub read_pos: std::sync::atomic::AtomicU64,
    /// Buffer capacity
    pub capacity: u64,
    /// Magic number for validation
    pub magic: u64,
}

impl RingBufferHeader {
    pub const MAGIC: u64 = 0x53484D52_42554621; // "SHMRBUF!"

    pub fn new(capacity: usize) -> Self {
        Self {
            write_pos: std::sync::atomic::AtomicU64::new(0),
            read_pos: std::sync::atomic::AtomicU64::new(0),
            capacity: capacity as u64,
            magic: Self::MAGIC,
        }
    }

    pub fn is_valid(&self) -> bool {
        self.magic == Self::MAGIC
    }
}

/// Shared memory ring buffer for zero-copy communication
pub struct SharedMemoryRingBuffer {
    mmap: MmapMut,
    header: *mut RingBufferHeader,
    data_offset: usize,
    data_size: usize,
}

unsafe impl Send for SharedMemoryRingBuffer {}
unsafe impl Sync for SharedMemoryRingBuffer {}

impl SharedMemoryRingBuffer {
    pub fn create(name: &str, config: ShmConfig) -> Result<Self> {
        let total_size = std::mem::size_of::<RingBufferHeader>() + config.buffer_size;

        // Create shared memory object
        let fd = shm_open(
            name,
            OFlag::O_CREAT | OFlag::O_RDWR | OFlag::O_EXCL,
            Mode::S_IRUSR | Mode::S_IWUSR,
        )?;

        // Set size
        ftruncate(&fd, total_size as i64)?;

        // Memory map
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .open(format!("/dev/shm/{}", name))?;

        let mut mmap = unsafe { MmapOptions::new().map_mut(&file)? };

        // Initialize header
        let header = mmap.as_mut_ptr() as *mut RingBufferHeader;
        unsafe {
            header.write(RingBufferHeader::new(config.buffer_size));
        }

        let data_offset = std::mem::size_of::<RingBufferHeader>();

        Ok(Self {
            mmap,
            header,
            data_offset,
            data_size: config.buffer_size,
        })
    }

    pub fn open(name: &str, config: ShmConfig) -> Result<Self> {
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .open(format!("/dev/shm/{}", name))?;

        let mut mmap = unsafe { MmapOptions::new().map_mut(&file)? };

        let header = mmap.as_mut_ptr() as *mut RingBufferHeader;

        // Validate header
        unsafe {
            if !(*header).is_valid() {
                return Err(crate::ShmError::InvalidState);
            }
        }

        let data_offset = std::mem::size_of::<RingBufferHeader>();

        Ok(Self {
            mmap,
            header,
            data_offset,
            data_size: config.buffer_size,
        })
    }

    pub fn write(&self, data: &[u8]) -> Result<usize> {
        let header = unsafe { &*self.header };
        let capacity = header.capacity as usize;
        
        let write_pos = header.write_pos.load(std::sync::atomic::Ordering::Acquire) as usize;
        let read_pos = header.read_pos.load(std::sync::atomic::Ordering::Acquire) as usize;
        
        // Calculate available space
        let available = if write_pos >= read_pos {
            capacity - (write_pos - read_pos) - 1
        } else {
            read_pos - write_pos - 1
        };

        if available < data.len() + 4 {
            return Err(crate::ShmError::BufferFull);
        }

        let data_ptr = unsafe { self.mmap.as_ptr().add(self.data_offset) as *mut u8 };

        // Write length prefix (4 bytes)
        let len_bytes = (data.len() as u32).to_le_bytes();
        for (i, byte) in len_bytes.iter().enumerate() {
            let pos = (write_pos + i) % capacity;
            unsafe { data_ptr.add(pos).write(*byte) };
        }

        // Write data
        let data_start = (write_pos + 4) % capacity;
        for (i, byte) in data.iter().enumerate() {
            let pos = (data_start + i) % capacity;
            unsafe { data_ptr.add(pos).write(*byte) };
        }

        // Update write position
        let new_write_pos = (write_pos + 4 + data.len()) % capacity;
        header.write_pos.store(new_write_pos as u64, std::sync::atomic::Ordering::Release);

        Ok(data.len())
    }

    pub fn read(&self, buf: &mut [u8]) -> Result<usize> {
        let header = unsafe { &*self.header };
        let capacity = header.capacity as usize;
        
        let write_pos = header.write_pos.load(std::sync::atomic::Ordering::Acquire) as usize;
        let read_pos = header.read_pos.load(std::sync::atomic::Ordering::Acquire) as usize;

        if write_pos == read_pos {
            return Err(crate::ShmError::BufferEmpty);
        }

        let data_ptr = unsafe { self.mmap.as_ptr().add(self.data_offset) as *const u8 };

        // Read length prefix
        let mut len_bytes = [0u8; 4];
        for i in 0..4 {
            let pos = (read_pos + i) % capacity;
            len_bytes[i] = unsafe { data_ptr.add(pos).read() };
        }
        let msg_len = u32::from_le_bytes(len_bytes) as usize;

        if buf.len() < msg_len {
            return Err(crate::ShmError::InvalidState);
        }

        // Read data
        let data_start = (read_pos + 4) % capacity;
        for i in 0..msg_len {
            let pos = (data_start + i) % capacity;
            buf[i] = unsafe { data_ptr.add(pos).read() };
        }

        // Update read position
        let new_read_pos = (read_pos + 4 + msg_len) % capacity;
        header.read_pos.store(new_read_pos as u64, std::sync::atomic::Ordering::Release);

        Ok(msg_len)
    }

    pub fn available_to_read(&self) -> usize {
        let header = unsafe { &*self.header };
        let write_pos = header.write_pos.load(std::sync::atomic::Ordering::Acquire) as usize;
        let read_pos = header.read_pos.load(std::sync::atomic::Ordering::Acquire) as usize;

        if write_pos >= read_pos {
            write_pos - read_pos
        } else {
            header.capacity as usize - (read_pos - write_pos)
        }
    }

    pub fn unlink(name: &str) -> Result<()> {
        shm_unlink(name)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ring_buffer() {
        let config = ShmConfig {
            buffer_size: 4096,
            max_message_size: 1024,
            timeout_ms: 1000,
        };

        let name = "test_shm_001";
        let _ = SharedMemoryRingBuffer::unlink(name);

        let writer = SharedMemoryRingBuffer::create(name, config).unwrap();
        let reader = SharedMemoryRingBuffer::open(name, config).unwrap();

        let test_data = b"Hello, Shared Memory!";
        writer.write(test_data).unwrap();

        let mut read_buf = vec![0u8; 256];
        let n = reader.read(&mut read_buf).unwrap();
        assert_eq!(&read_buf[..n], test_data);

        SharedMemoryRingBuffer::unlink(name).unwrap();
    }
}
