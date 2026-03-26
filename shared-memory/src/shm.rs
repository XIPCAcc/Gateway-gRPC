use std::os::fd::AsRawFd;
use std::sync::atomic::Ordering;
use std::sync::Mutex;

use memmap2::{MmapMut, MmapOptions};
use nix::fcntl::OFlag;
use nix::sys::mman::{shm_open, shm_unlink};
use nix::sys::stat::Mode;
use nix::unistd::ftruncate;
use tracing::info;

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
    write_lock: Mutex<()>,
    read_lock: Mutex<()>,
}

unsafe impl Send for SharedMemoryRingBuffer {}
unsafe impl Sync for SharedMemoryRingBuffer {}

// Note: SharedMemoryRingBuffer cannot implement Clone because MmapMut doesn't support cloning.
// Use Arc<SharedMemoryRingBuffer> for shared ownership.

impl SharedMemoryRingBuffer {
    pub fn create(name: &str, config: ShmConfig) -> Result<Self> {
        let total_size = std::mem::size_of::<RingBufferHeader>() + config.buffer_size;
        
        info!("Creating shared memory: name={}, total_size={} bytes", name, total_size);

        // Create shared memory object
        let fd = match shm_open(
            name,
            OFlag::O_CREAT | OFlag::O_RDWR | OFlag::O_EXCL,
            Mode::S_IRUSR | Mode::S_IWUSR,
        ) {
            Ok(fd) => {
                info!("Shared memory created: name={}, fd={}", name, fd.as_raw_fd());
                fd
            },
            Err(e) => {
                info!("Failed to create shared memory: name={}, error={}", name, e);
                return Err(e.into());
            }
        };

        // Set size
        match ftruncate(&fd, total_size as i64) {
            Ok(_) => info!("Shared memory truncated: name={}, size={} bytes", name, total_size),
            Err(e) => {
                info!("Failed to truncate shared memory: name={}, error={}", name, e);
                return Err(e.into());
            }
        }

        // Memory map directly from fd
        let mut mmap = unsafe {
            match MmapOptions::new().map_mut(&fd) {
                Ok(mmap) => {
                    info!("Shared memory mapped: name={}, size={} bytes", name, total_size);
                    mmap
                },
                Err(e) => {
                    info!("Failed to map shared memory: name={}, error={}", name, e);
                    return Err(e.into());
                }
            }
        };

        // Initialize header
        let header = mmap.as_mut_ptr() as *mut RingBufferHeader;
        unsafe {
            header.write(RingBufferHeader::new(config.buffer_size));
        }

        let data_offset = std::mem::size_of::<RingBufferHeader>();

        info!("Shared memory ring buffer created successfully: name={}", name);

        Ok(Self {
            mmap,
            header,
            data_offset,
            data_size: config.buffer_size,
            write_lock: Mutex::new(()),
            read_lock: Mutex::new(()),
        })
    }

    pub fn open(name: &str, config: ShmConfig) -> Result<Self> {
        let total_size = std::mem::size_of::<RingBufferHeader>() + config.buffer_size;

        info!("Opening shared memory: name={}, total_size={} bytes", name, total_size);

        // Open existing shared memory
        let fd = match shm_open(
            name,
            OFlag::O_RDWR,
            Mode::S_IRUSR | Mode::S_IWUSR,
        ) {
            Ok(fd) => {
                info!("Shared memory opened: name={}, fd={}", name, fd.as_raw_fd());
                fd
            },
            Err(e) => {
                info!("Failed to open shared memory: name={}, error={}", name, e);
                return Err(e.into());
            }
        };

        // Memory map directly from fd
        let mut mmap = unsafe {
            match MmapOptions::new().map_mut(&fd) {
                Ok(mmap) => {
                    info!("Shared memory mapped: name={}, size={} bytes", name, total_size);
                    mmap
                },
                Err(e) => {
                    info!("Failed to map shared memory: name={}, error={}", name, e);
                    return Err(e.into());
                }
            }
        };

        let header = mmap.as_mut_ptr() as *mut RingBufferHeader;

        // Validate header
        unsafe {
            if !(*header).is_valid() {
                info!("Shared memory header invalid: name={}", name);
                return Err(crate::ShmError::InvalidState);
            }
        }

        let data_offset = std::mem::size_of::<RingBufferHeader>();

        info!("Shared memory ring buffer opened successfully: name={}", name);

        Ok(Self {
            mmap,
            header,
            data_offset,
            data_size: config.buffer_size,
            write_lock: Mutex::new(()),
            read_lock: Mutex::new(()),
        })
    }

    pub fn write(&self, data: &[u8]) -> Result<usize> {
        let _guard = self.write_lock.lock().unwrap();
        
        let header = unsafe { &*self.header };
        let capacity = header.capacity as usize;
        
        let write_pos = header.write_pos.load(Ordering::Acquire) as usize;
        let read_pos = header.read_pos.load(Ordering::Acquire) as usize;
        
        // Calculate available space
        let available = if write_pos >= read_pos {
            capacity - (write_pos - read_pos) - 1
        } else {
            read_pos - write_pos - 1
        };

        let total_size = 4 + data.len();
        if available < total_size {
            return Err(crate::ShmError::BufferFull);
        }

        let data_ptr = unsafe { self.mmap.as_ptr().add(self.data_offset) as *mut u8 };

        // Write length prefix (4 bytes)
        let len_bytes = (data.len() as u32).to_le_bytes();
        unsafe {
            std::ptr::copy_nonoverlapping(
                len_bytes.as_ptr(),
                data_ptr.add(write_pos),
                4
            );
        }

        // Write data
        let data_start = (write_pos + 4) % capacity;
        if data_start + data.len() <= capacity {
            // Continuous memory, copy directly
            unsafe {
                std::ptr::copy_nonoverlapping(
                    data.as_ptr(),
                    data_ptr.add(data_start),
                    data.len()
                );
            }
        } else {
            // Cross boundary, copy in two parts
            let first_part = capacity - data_start;
            let second_part = data.len() - first_part;
            
            unsafe {
                std::ptr::copy_nonoverlapping(
                    data.as_ptr(),
                    data_ptr.add(data_start),
                    first_part
                );
                std::ptr::copy_nonoverlapping(
                    data.as_ptr().add(first_part),
                    data_ptr,
                    second_part
                );
            }
        }

        // Update write position
        let new_write_pos = (write_pos + total_size) % capacity;
        header.write_pos.store(new_write_pos as u64, Ordering::Release);

        Ok(data.len())
    }

    pub fn read(&self, buf: &mut [u8]) -> Result<usize> {
        let _guard = self.read_lock.lock().unwrap();
        
        let header = unsafe { &*self.header };
        let capacity = header.capacity as usize;
        
        let write_pos = header.write_pos.load(Ordering::Acquire) as usize;
        let read_pos = header.read_pos.load(Ordering::Acquire) as usize;

        if write_pos == read_pos {
            return Err(crate::ShmError::BufferEmpty);
        }

        // Read length prefix (4 bytes)
        let data_ptr = unsafe { self.mmap.as_ptr().add(self.data_offset) as *const u8 };
        let len_bytes: [u8; 4] = unsafe {
            [
                *data_ptr.add(read_pos),
                *data_ptr.add((read_pos + 1) % capacity),
                *data_ptr.add((read_pos + 2) % capacity),
                *data_ptr.add((read_pos + 3) % capacity),
            ]
        };
        let data_len = u32::from_le_bytes(len_bytes) as usize;

        if buf.len() < data_len {
            return Err(crate::ShmError::InvalidState);
        }

        // Read data
        let data_start = (read_pos + 4) % capacity;
        if data_start + data_len <= capacity {
            // Continuous memory, copy directly
            unsafe {
                std::ptr::copy_nonoverlapping(
                    data_ptr.add(data_start),
                    buf.as_mut_ptr(),
                    data_len
                );
            }
        } else {
            // Cross boundary, copy in two parts
            let first_part = capacity - data_start;
            let second_part = data_len - first_part;
            
            unsafe {
                std::ptr::copy_nonoverlapping(
                    data_ptr.add(data_start),
                    buf.as_mut_ptr(),
                    first_part
                );
                std::ptr::copy_nonoverlapping(
                    data_ptr.add(0),
                    buf.as_mut_ptr().add(first_part),
                    second_part
                );
            }
        }

        // Update read position
        let new_read_pos = (read_pos + 4 + data_len) % capacity;
        header.read_pos.store(new_read_pos as u64, Ordering::Release);

        Ok(data_len)
    }
}

impl Drop for SharedMemoryRingBuffer {
    fn drop(&mut self) {
        // Note: We don't unlink here as it might be in use by other processes
        // The creator should handle cleanup
    }
}
