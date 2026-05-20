pub mod eventfd;
pub mod shm;

pub use eventfd::*;
pub use shm::*;

use serde::{Deserialize, Serialize};
use thiserror::Error;
use std::sync::atomic::{AtomicU64, Ordering};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ShmRequest {
    Echo { id: String, request: Vec<u8> },
    MatrixMultiply { id: String, matrix_size: i32, data_offset: u64, data_len: u32 },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ShmResponse {
    Echo { id: String, response: Vec<u8> },
    MatrixMultiply { id: String, response: Vec<u8> },
}

pub fn multiply_matrices(a: &[Vec<f64>], b: &[Vec<f64>], result: &mut [Vec<f64>]) {
    let n = a.len();
    for i in 0..n {
        for j in 0..n {
            let mut sum = 0.0;
            for k in 0..n {
                sum += a[i][k] * b[k][j];
            }
            result[i][j] = sum;
        }
    }
}

pub fn generate_matrix(size: usize) -> Vec<Vec<f64>> {
    let mut matrix = vec![vec![0.0; size]; size];
    for i in 0..size {
        for j in 0..size {
            matrix[i][j] = (i * size + j) as f64 / (size * size) as f64;
        }
    }
    matrix
}

pub fn calculate_checksum(matrix: &[Vec<f64>]) -> f64 {
    let mut sum = 0.0;
    for row in matrix {
        for &val in row {
            sum += val;
        }
    }
    sum
}

pub fn flatten_matrix(matrix: &[Vec<f64>]) -> Vec<f64> {
    let size = matrix.len();
    let mut flat = Vec::with_capacity(size * size);
    for row in matrix {
        flat.extend_from_slice(row);
    }
    flat
}

pub fn reconstruct_matrix(flat: &[f64], size: usize) -> Vec<Vec<f64>> {
    let mut matrix = Vec::with_capacity(size);
    for i in 0..size {
        let start = i * size;
        matrix.push(flat[start..start + size].to_vec());
    }
    matrix
}

pub fn f64_slice_as_bytes(slice: &[f64]) -> &[u8] {
    let len = slice.len() * std::mem::size_of::<f64>();
    unsafe { std::slice::from_raw_parts(slice.as_ptr() as *const u8, len) }
}

pub fn bytes_as_f64_slice(bytes: &[u8]) -> &[f64] {
    let len = bytes.len() / std::mem::size_of::<f64>();
    unsafe { std::slice::from_raw_parts(bytes.as_ptr() as *const f64, len) }
}

#[repr(C)]
struct MatrixDataHeader {
    alloc_offset: AtomicU64,
    capacity: u64,
    magic: u64,
}

const MATRIX_DATA_MAGIC: u64 = 0x4D415452_44415441; // "MATRDATA"

pub struct MatrixDataPool {
    mmap: memmap2::MmapMut,
    header: *mut MatrixDataHeader,
    data_offset: usize,
}

unsafe impl Send for MatrixDataPool {}
unsafe impl Sync for MatrixDataPool {}

impl MatrixDataPool {
    pub fn create(name: &str, capacity: usize) -> Result<Self> {
        use nix::fcntl::OFlag;
        use nix::sys::mman::shm_open;
        use nix::sys::stat::Mode;
        use nix::unistd::ftruncate;
        use memmap2::MmapOptions;

        let total_size = std::mem::size_of::<MatrixDataHeader>() + capacity;

        let fd = shm_open(
            name,
            OFlag::O_CREAT | OFlag::O_RDWR | OFlag::O_EXCL,
            Mode::S_IRUSR | Mode::S_IWUSR,
        )?;

        ftruncate(&fd, total_size as i64)?;

        let mut mmap = unsafe { MmapOptions::new().map_mut(&fd)? };

        let header = mmap.as_mut_ptr() as *mut MatrixDataHeader;
        unsafe {
            (*header).alloc_offset = AtomicU64::new(0);
            (*header).capacity = capacity as u64;
            (*header).magic = MATRIX_DATA_MAGIC;
        }

        Ok(Self {
            mmap,
            header,
            data_offset: std::mem::size_of::<MatrixDataHeader>(),
        })
    }

    pub fn open(name: &str) -> Result<Self> {
        use nix::fcntl::OFlag;
        use nix::sys::mman::shm_open;
        use nix::sys::stat::Mode;
        use memmap2::MmapOptions;

        let fd = shm_open(name, OFlag::O_RDWR, Mode::S_IRUSR | Mode::S_IWUSR)?;

        let mmap = unsafe { MmapOptions::new().map_mut(&fd)? };

        let header = mmap.as_ptr() as *mut MatrixDataHeader;
        unsafe {
            if (*header).magic != MATRIX_DATA_MAGIC {
                return Err(ShmError::InvalidState);
            }
        }

        Ok(Self {
            mmap,
            header,
            data_offset: std::mem::size_of::<MatrixDataHeader>(),
        })
    }

    pub fn allocate(&self, size: usize) -> Result<u64> {
        let header = unsafe { &*self.header };
        let capacity = header.capacity as usize;

        if size > capacity {
            return Err(ShmError::BufferFull);
        }

        loop {
            let current = header.alloc_offset.load(Ordering::Acquire);
            let next_offset = current as usize + size;

            if next_offset > capacity {
                match header.alloc_offset.compare_exchange(
                    current,
                    size as u64,
                    Ordering::AcqRel,
                    Ordering::Acquire,
                ) {
                    Ok(_) => return Ok(0),
                    Err(_) => continue,
                }
            } else {
                match header.alloc_offset.compare_exchange(
                    current,
                    next_offset as u64,
                    Ordering::AcqRel,
                    Ordering::Acquire,
                ) {
                    Ok(_) => return Ok(current),
                    Err(_) => continue,
                }
            }
        }
    }

    pub fn write_data(&self, offset: u64, data: &[u8]) {
        let data_ptr = unsafe { self.mmap.as_ptr().add(self.data_offset + offset as usize) as *mut u8 };
        unsafe {
            std::ptr::copy_nonoverlapping(data.as_ptr(), data_ptr, data.len());
        }
    }

    pub fn read_data(&self, offset: u64, len: usize) -> &[u8] {
        let data_ptr = unsafe { self.mmap.as_ptr().add(self.data_offset + offset as usize) };
        unsafe { std::slice::from_raw_parts(data_ptr, len) }
    }
}

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
            buffer_size: 64 * 1024 * 1024, // 64MB
            max_message_size: 64 * 1024, // 64KB
            timeout_ms: 5000,
        }
    }
}
