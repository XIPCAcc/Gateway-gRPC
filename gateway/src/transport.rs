use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use async_trait::async_trait;
use hyper_util::rt::TokioIo;
use proto::{EchoRequest, EchoResponse, MatrixMultiplyRequest, MatrixMultiplyResponse};
use shared_memory::shm::SharedMemoryRingBuffer;
use shared_memory::{ShmConfig, ShmRequest, ShmResponse, MatrixDataPool, flatten_matrix, f64_slice_as_bytes};
use tonic::transport::{Channel, Endpoint, Uri};
use tokio::net::UnixStream;
use tokio::sync::Mutex as TokioMutex;
use tracing::{debug, info};
use uuid::Uuid;

#[cfg(target_os = "linux")]
pub use crate::shm_transport_uds::ShmTransportUds;
#[cfg(target_os = "linux")]
pub use crate::shm_transport_eventfd::ShmTransportEventfd;
#[cfg(target_os = "linux")]
pub use crate::shm_transport_uintr::ShmTransportUintr;

/// Transport trait for different backend communication methods
#[async_trait]
pub trait Transport: Send + Sync {
    async fn call(&self, request: EchoRequest) -> Result<EchoResponse>;
    async fn matrix_multiply(&self, request: MatrixMultiplyRequest) -> Result<MatrixMultiplyResponse>;
}

/// TCP transport using tonic gRPC
pub struct TcpTransport {
    client: proto::echo_service_client::EchoServiceClient<Channel>,
}

impl TcpTransport {
    pub async fn new(addr: &str) -> Result<Self> {
        let endpoint = Endpoint::from_shared(format!("http://{}", addr))?
            .timeout(Duration::from_secs(30))
            .connect_timeout(Duration::from_secs(5))
            .tcp_nodelay(true);

        let channel = endpoint.connect().await?;
        let client = proto::echo_service_client::EchoServiceClient::new(channel);

        info!("TCP transport connected to {}", addr);
        Ok(Self { client })
    }
}

#[async_trait]
impl Transport for TcpTransport {
    async fn call(&self, request: EchoRequest) -> Result<EchoResponse> {
        let mut client = self.client.clone();
        let response: tonic::Response<EchoResponse> = client.echo(tonic::Request::new(request)).await?;
        Ok(response.into_inner())
    }

    async fn matrix_multiply(&self, request: MatrixMultiplyRequest) -> Result<MatrixMultiplyResponse> {
        let mut client = self.client.clone();
        let response: tonic::Response<MatrixMultiplyResponse> = client.matrix_multiply(tonic::Request::new(request)).await?;
        Ok(response.into_inner())
    }
}

/// Unix Domain Socket transport
pub struct UdsTransport {
    client: proto::echo_service_client::EchoServiceClient<Channel>,
}

impl UdsTransport {
    pub async fn new(path: &Path) -> Result<Self> {
        use tonic::transport::Uri;
        
        let path_buf = path.to_path_buf();
        
        // Create UDS endpoint
        let mut endpoint = Endpoint::try_from("http://[::]:50051")?
            .timeout(Duration::from_secs(30))
            .connect_timeout(Duration::from_secs(5));
        
        let endpoint = endpoint.connect_with_connector(tower::service_fn(move |_: Uri| {
            let path = path_buf.clone();
            async move {
                let stream = UnixStream::connect(path).await?;
                Ok::<_, anyhow::Error>(TokioIo::new(stream))
            }
        }));

        let channel = endpoint.await?;
        let client = proto::echo_service_client::EchoServiceClient::new(channel);

        info!("UDS transport initialized (path: {:?})", path);
        Ok(Self { client })
    }
}

#[async_trait]
impl Transport for UdsTransport {
    async fn call(&self, request: EchoRequest) -> Result<EchoResponse> {
        let mut client = self.client.clone();
        let response: tonic::Response<EchoResponse> = client.echo(tonic::Request::new(request)).await?;
        Ok(response.into_inner())
    }

    async fn matrix_multiply(&self, request: MatrixMultiplyRequest) -> Result<MatrixMultiplyResponse> {
        let mut client = self.client.clone();
        let response: tonic::Response<MatrixMultiplyResponse> = client.matrix_multiply(tonic::Request::new(request)).await?;
        Ok(response.into_inner())
    }
}

/// Tokio-based Shared Memory transport using async polling
/// This implementation uses Tokio's async runtime instead of eventfd
pub struct ShmTransport {
    name: String,
    request_buffer: Arc<TokioMutex<SharedMemoryRingBuffer>>,
    response_buffer: Arc<TokioMutex<SharedMemoryRingBuffer>>,
    data_pool: Arc<MatrixDataPool>,
}

impl ShmTransport {
    pub async fn new(name: &str) -> Result<Self> {
        let config = ShmConfig::default();
        
        let request_buffer = Self::wait_for_shm(&format!("{}_req_buf", name), config).await?;
        let response_buffer = Self::wait_for_shm(&format!("{}_resp_buf", name), config).await?;

        let data_pool_name = format!("{}_matrix_data", name);
        let data_pool = Self::wait_for_data_pool(&data_pool_name).await?;

        info!("Shared Memory transport initialized (name: {})", name);
        Ok(Self {
            name: name.to_string(),
            request_buffer: Arc::new(TokioMutex::new(request_buffer)),
            response_buffer: Arc::new(TokioMutex::new(response_buffer)),
            data_pool: Arc::new(data_pool),
        })
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
                    tokio::time::sleep(Duration::from_millis(100)).await;
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
                    tokio::time::sleep(Duration::from_millis(100)).await;
                }
            }
        }
    }
}

#[async_trait]
impl Transport for ShmTransport {
    async fn call(&self, request: EchoRequest) -> Result<EchoResponse> {
        // Serialize request
        let payload = serde_json::to_vec(&request)
            .map_err(|e| anyhow::anyhow!("Failed to serialize request: {}", e))?;
        
        // Send request to shared memory
        {
            let buffer = self.request_buffer.lock().await;
            buffer.write(&payload)
                .map_err(|e| anyhow::anyhow!("Failed to write to shared memory: {}", e))?;
        }
        
        // Poll for response using Tokio async
        let start = std::time::Instant::now();
        let timeout = Duration::from_secs(30);
        let mut response_buf = vec![0u8; 65536];
        
        loop {
            if start.elapsed() > timeout {
                return Err(anyhow::anyhow!("Request timeout after {:?}", timeout));
            }
            
            // Try to read response
            let n = {
                let buffer = self.response_buffer.lock().await;
                match buffer.read(&mut response_buf) {
                    Ok(n) => n,
                    Err(_) => {
                        // No data available, yield to Tokio runtime
                        // tokio::task::yield_now().await;
                        // tokio::time::sleep(Duration::from_micros(50)).await;
                        continue;
                    }
                }
            };
            
            // Parse response
            match serde_json::from_slice::<EchoResponse>(&response_buf[..n]) {
                Ok(response) => return Ok(response),
                Err(e) => {
                    return Err(anyhow::anyhow!("Failed to deserialize response: {}", e));
                }
            }
        }
    }

    async fn matrix_multiply(&self, request: MatrixMultiplyRequest) -> Result<MatrixMultiplyResponse> {
        let request_id = Uuid::new_v4().to_string();

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

        {
            let buffer = self.request_buffer.lock().await;
            buffer.write(&payload)
                .map_err(|e| anyhow::anyhow!("Failed to write to shared memory: {}", e))?;
        }

        let start = std::time::Instant::now();
        let timeout = Duration::from_secs(30);
        let mut response_buf = vec![0u8; 65536];

        loop {
            if start.elapsed() > timeout {
                return Err(anyhow::anyhow!("Request {} timeout after {:?}", request_id, timeout));
            }

            let n = {
                let buffer = self.response_buffer.lock().await;
                match buffer.read(&mut response_buf) {
                    Ok(n) => n,
                    Err(_) => {
                        tokio::time::sleep(Duration::from_micros(50)).await;
                        continue;
                    }
                }
            };

            let shm_response: ShmResponse = match bincode::deserialize(&response_buf[..n]) {
                Ok(r) => r,
                Err(_) => continue,
            };

            match shm_response {
                ShmResponse::MatrixMultiply { id, response } => {
                    if id == request_id {
                        let result: MatrixMultiplyResponse = bincode::deserialize(&response)
                            .map_err(|e| anyhow::anyhow!("Failed to deserialize MatrixMultiplyResponse: {}", e))?;
                        return Ok(result);
                    }
                }
                _ => continue,
            }
        }
    }
}

/// Transport enum for type-erased usage
pub enum TransportEnum {
    Tcp(TcpTransport),
    Uds(UdsTransport),
    Shm(ShmTransport),
    #[cfg(target_os = "linux")]
    ShmUds(ShmTransportUds),
    #[cfg(target_os = "linux")]
    ShmEventfd(ShmTransportEventfd),
    #[cfg(target_os = "linux")]
    ShmUintr(ShmTransportUintr),
}

#[async_trait]
impl Transport for TransportEnum {
    async fn call(&self, request: EchoRequest) -> Result<EchoResponse> {
        match self {
            TransportEnum::Tcp(t) => t.call(request).await,
            TransportEnum::Uds(t) => t.call(request).await,
            TransportEnum::Shm(t) => t.call(request).await,
            #[cfg(target_os = "linux")]
            TransportEnum::ShmUds(t) => t.call(request).await,
            #[cfg(target_os = "linux")]
            TransportEnum::ShmEventfd(t) => t.call(request).await,
            #[cfg(target_os = "linux")]
            TransportEnum::ShmUintr(t) => t.call(request).await,
        }
    }

    async fn matrix_multiply(&self, request: MatrixMultiplyRequest) -> Result<MatrixMultiplyResponse> {
        match self {
            TransportEnum::Tcp(t) => t.matrix_multiply(request).await,
            TransportEnum::Uds(t) => t.matrix_multiply(request).await,
            TransportEnum::Shm(t) => t.matrix_multiply(request).await,
            #[cfg(target_os = "linux")]
            TransportEnum::ShmUds(t) => t.matrix_multiply(request).await,
            #[cfg(target_os = "linux")]
            TransportEnum::ShmEventfd(t) => t.matrix_multiply(request).await,
            #[cfg(target_os = "linux")]
            TransportEnum::ShmUintr(t) => t.matrix_multiply(request).await,
        }
    }
}

/// Connection pool for reusing connections
pub struct ConnectionPool<T> {
    connections: Vec<T>,
    current: std::sync::atomic::AtomicUsize,
}

impl<T> ConnectionPool<T> {
    pub fn new(connections: Vec<T>) -> Self {
        Self {
            connections,
            current: std::sync::atomic::AtomicUsize::new(0),
        }
    }

    pub fn get(&self) -> &T {
        let idx = self.current.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            % self.connections.len();
        &self.connections[idx]
    }
}

/// Load balancer for multiple backends
pub struct LoadBalancer {
    backends: Vec<String>,
    current: std::sync::atomic::AtomicUsize,
}

impl LoadBalancer {
    pub fn new(backends: Vec<String>) -> Self {
        Self {
            backends,
            current: std::sync::atomic::AtomicUsize::new(0),
        }
    }

    pub fn next_backend(&self) -> &str {
        let idx = self.current.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            % self.backends.len();
        &self.backends[idx]
    }
}
