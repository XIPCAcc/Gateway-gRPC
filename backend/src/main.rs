use std::net::SocketAddr;
use std::path::Path;
use std::time::{Duration, Instant};

use anyhow::Result;
use clap::{Parser, ValueEnum};
use prometheus::{register_counter, register_histogram, Counter, Histogram};
use proto::echo_service_server::{EchoService as EchoServiceTrait, EchoServiceServer};
use proto::{ComputeRequest, ComputeResponse, EchoRequest, EchoResponse, MatrixMultiplyRequest, MatrixMultiplyResponse};
use shared_memory::{multiply_matrices, calculate_checksum, ShmRequest, ShmResponse, MatrixDataPool, bytes_as_f64_slice, reconstruct_matrix};
use tonic::{transport::Server, Request, Response, Status};
use tokio::net::UnixListener;
use tokio::runtime::Builder;
use tracing::{info, warn};

use shared_memory::shm::SharedMemoryRingBuffer;
use shared_memory::ShmConfig as SharedMemConfig;

#[cfg(target_os = "linux")]
mod shm_server_uds;
#[cfg(target_os = "linux")]
use shm_server_uds::ShmServerUds;

#[cfg(target_os = "linux")]
mod shm_server_eventfd;
#[cfg(target_os = "linux")]
use shm_server_eventfd::ShmServerEventfd;

#[cfg(target_os = "linux")]
mod shm_server_uintr;
#[cfg(target_os = "linux")]
use shm_server_uintr::ShmServerUintr;

#[cfg(target_os = "linux")]
mod uintr_server;
#[cfg(target_os = "linux")]
use uintr_server::UintrServer;

lazy_static::lazy_static! {
    static ref REQUEST_COUNTER: Counter = register_counter!(
        "backend_requests_total",
        "Total number of requests received"
    ).unwrap();

    static ref REQUEST_DURATION: Histogram = register_histogram!(
        "backend_request_duration_seconds",
        "Request processing duration in seconds",
        prometheus::exponential_buckets(0.0001, 2.0, 20).unwrap()
    ).unwrap();
}

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
    #[arg(short, long, default_value = "127.0.0.1:50051")]
    addr: String,

    #[arg(short, long, value_enum, default_value = "tcp")]
    transport: TransportType,

    #[arg(long, default_value = "/tmp/backend.sock")]
    uds_path: String,

    #[arg(long, default_value = "backend")]
    shm_name: String,

    #[arg(short, long, default_value_t = 1000)]
    delay_us: u64,

    #[arg(short, long, default_value_t = 100)]
    compute_iterations: i32,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum TransportType {
    Tcp,
    Uds,
    Shm,
    #[cfg(target_os = "linux")]
    ShmUds,
    #[cfg(target_os = "linux")]
    ShmEventfd,
    #[cfg(target_os = "linux")]
    ShmUintr,
    #[cfg(target_os = "linux")]
    Uintr,
}

#[derive(Debug, Default)]
pub struct EchoServiceImpl {
    delay: Duration,
    compute_iterations: i32,
}

impl EchoServiceImpl {
    pub fn new(delay: Duration, compute_iterations: i32) -> Self {
        Self {
            delay,
            compute_iterations,
        }
    }
}

#[tonic::async_trait]
impl EchoServiceTrait for EchoServiceImpl {
    async fn echo(&self, request: Request<EchoRequest>) -> Result<Response<EchoResponse>, Status> {
        REQUEST_COUNTER.inc();
        let start = Instant::now();

        let req = request.into_inner();
        let recv_time = Instant::now();

        // Simulate fixed processing delay
        if self.delay > Duration::ZERO {
            tokio::time::sleep(self.delay).await;
        }

        let processing_time = start.elapsed();

        let response = EchoResponse {
            message: req.message,
            timestamp_ns: recv_time.elapsed().as_nanos() as i64,
            processing_time_us: processing_time.as_micros() as i64,
            payload: req.payload,
        };

        REQUEST_DURATION.observe(start.elapsed().as_secs_f64());
        Ok(Response::new(response))
    }

    async fn compute(
        &self,
        request: Request<ComputeRequest>,
    ) -> Result<Response<ComputeResponse>, Status> {
        REQUEST_COUNTER.inc();
        let start = Instant::now();

        let req = request.into_inner();

        // Simulate CPU-intensive computation
        let mut result = req.input_value;
        let iterations = req.iterations.max(1).min(10000);
        for i in 0..iterations {
            result = result.sin() * result.cos() + (i as f64).sqrt();
        }

        let processing_time = start.elapsed();

        let response = ComputeResponse {
            result,
            timestamp_ns: start.elapsed().as_nanos() as i64,
            processing_time_us: processing_time.as_micros() as i64,
        };

        REQUEST_DURATION.observe(start.elapsed().as_secs_f64());
        Ok(Response::new(response))
    }

    async fn matrix_multiply(
        &self,
        request: Request<MatrixMultiplyRequest>,
    ) -> Result<Response<MatrixMultiplyResponse>, Status> {
        REQUEST_COUNTER.inc();
        let start = Instant::now();

        let req = request.into_inner();
        let size = req.matrix_size as usize;

        let a: Vec<Vec<f64>> = bincode::deserialize(&req.matrix_a)
            .map_err(|e| Status::invalid_argument(format!("Failed to deserialize matrix_a: {}", e)))?;
        let b: Vec<Vec<f64>> = bincode::deserialize(&req.matrix_b)
            .map_err(|e| Status::invalid_argument(format!("Failed to deserialize matrix_b: {}", e)))?;

        let mut result = vec![vec![0.0; size]; size];
        multiply_matrices(&a, &b, &mut result);
        let checksum = calculate_checksum(&result);

        let processing_time = start.elapsed();

        let total_ops = 2.0 * (size as f64).powi(3);
        let gflops = total_ops / (processing_time.as_secs_f64() * 1e9);

        let response = MatrixMultiplyResponse {
            checksum,
            timestamp_ns: start.elapsed().as_nanos() as i64,
            processing_time_us: processing_time.as_micros() as i64,
            gflops,
        };

        REQUEST_DURATION.observe(start.elapsed().as_secs_f64());
        Ok(Response::new(response))
    }

    type StreamEchoStream =
        tonic::codegen::tokio_stream::wrappers::ReceiverStream<Result<EchoResponse, Status>>;

    async fn stream_echo(
        &self,
        request: Request<tonic::Streaming<EchoRequest>>,
    ) -> Result<Response<Self::StreamEchoStream>, Status> {
        let mut stream = request.into_inner();
        let (tx, rx) = tokio::sync::mpsc::channel(128);
        let delay = self.delay;

        tokio::spawn(async move {
            while let Some(req) = stream.message().await.transpose() {
                match req {
                    Ok(req) => {
                        if delay > Duration::ZERO {
                            tokio::time::sleep(delay).await;
                        }
                        let response = EchoResponse {
                            message: req.message,
                            timestamp_ns: 0,
                            processing_time_us: delay.as_micros() as i64,
                            payload: req.payload,
                        };
                        if tx.send(Ok(response)).await.is_err() {
                            break;
                        }
                    }
                    Err(e) => {
                        warn!("Stream error: {}", e);
                        break;
                    }
                }
            }
        });

        Ok(Response::new(tonic::codegen::tokio_stream::wrappers::ReceiverStream::new(rx)))
    }
}

fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    let args = Args::parse();

    let event_interval: u32 = std::env::var("TOKIO_EVENT_INTERVAL")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(64);

    let rt = Builder::new_multi_thread()
        .event_interval(event_interval)
        .enable_all()
        .build()?;

    rt.block_on(async_main(args))
}

async fn async_main(args: Args) -> Result<()> {
    let delay = Duration::from_micros(args.delay_us);
    let echo_service = EchoServiceImpl::new(delay, args.compute_iterations);

    match args.transport {
        TransportType::Tcp => {
            let addr: SocketAddr = args.addr.parse()?;
            info!(
                "Starting backend server on {} (TCP) with delay {}us",
                args.addr, args.delay_us
            );

            Server::builder()
                .add_service(EchoServiceServer::new(echo_service))
                .serve(addr)
                .await?;
        }
        TransportType::Uds => {
            let path = Path::new(&args.uds_path);
            
            // Remove existing socket file if it exists
            if path.exists() {
                std::fs::remove_file(path)?;
            }

            info!(
                "Starting backend server on {:?} (UDS) with delay {}us",
                path, args.delay_us
            );

            let listener = UnixListener::bind(path)?;
            
            // Convert UnixListener to stream of incoming connections
            let incoming = async_stream::stream! {
                loop {
                    match listener.accept().await {
                        Ok((stream, _)) => {
                            yield Ok::<_, tonic::transport::Error>(stream);
                        },
                        Err(e) => {
                            warn!("UDS accept error: {:?}", e);
                            continue;
                        }
                    }
                }
            };

            Server::builder()
                .add_service(EchoServiceServer::new(echo_service))
                .serve_with_incoming(incoming)
                .await?;
        }
        TransportType::Shm => {
            let shm_name = &args.shm_name;
            let config = SharedMemConfig::default();
            
            info!(
                "Starting backend server on shared memory (name: {}) with delay {}us",
                shm_name, args.delay_us
            );

            // Create shared memory ring buffers
            let request_buffer = SharedMemoryRingBuffer::create(&format!("{}_req_buf", shm_name), config)
                .map_err(|e| anyhow::anyhow!("Failed to create request buffer: {}", e))?;
            let response_buffer = SharedMemoryRingBuffer::create(&format!("{}_resp_buf", shm_name), config)
                .map_err(|e| anyhow::anyhow!("Failed to create response buffer: {}", e))?;

            info!("Shared memory buffers created successfully");

            let data_pool_name = format!("{}_matrix_data", shm_name);
            let data_pool_capacity = 256 * 1024 * 1024;
            let data_pool = MatrixDataPool::create(&data_pool_name, data_pool_capacity)
                .map_err(|e| anyhow::anyhow!("Failed to create matrix data pool: {}", e))?;

            run_shm_server(request_buffer, response_buffer, data_pool, delay).await?;
        }
        #[cfg(target_os = "linux")]
        TransportType::ShmUds => {
            let shm_name = &args.shm_name;
            
            info!(
                "Starting backend server on SHM with UDS notification (name: {}) with delay {}us",
                shm_name, args.delay_us
            );

            // Create SHM server with UDS notification
            let server = ShmServerUds::new(shm_name)?;
            
            // Run server
            server.run(delay).await?;
        }
        #[cfg(target_os = "linux")]
        TransportType::ShmEventfd => {
            let shm_name = &args.shm_name;
            
            info!(
                "Starting backend server on eventfd SHM (name: {}) with delay {}us",
                shm_name, args.delay_us
            );

            // Create eventfd-based SHM server
            let server = ShmServerEventfd::new(shm_name)?;
            
            // Run server
            server.run(delay).await?;
        }
        #[cfg(target_os = "linux")]
        TransportType::ShmUintr => {
            let shm_name = &args.shm_name;
            
            info!(
                "Starting backend server on SHM with UINTR notification (name: {}) with delay {}us",
                shm_name, args.delay_us
            );

            // Create SHM server with UINTR notification
            let server = ShmServerUintr::new(shm_name)?;
            
            // Run server
            server.run(delay).await?;
        }
        #[cfg(target_os = "linux")]
        TransportType::Uintr => {
            let socket_path = &args.uds_path;
            
            info!(
                "Starting backend server on UINTR (socket: {}) with delay {}us",
                socket_path, args.delay_us
            );

            // Create UINTR server
            let mut server = UintrServer::new(socket_path);
            
            // Run UINTR test
            server.run_test(10).await.map_err(|e| anyhow::anyhow!("UINTR test failed: {}", e))?;
        }
    }

    Ok(())
}

async fn run_shm_server(
    request_buffer: SharedMemoryRingBuffer,
    response_buffer: SharedMemoryRingBuffer,
    data_pool: MatrixDataPool,
    delay: Duration,
) -> Result<()> {
    info!("Shared memory server started, waiting for requests...");

    let mut request_buf = vec![0u8; 65536];
    
    loop {
        match request_buffer.read(&mut request_buf) {
            Ok(n) => {
                if let Ok(shm_request) = bincode::deserialize::<ShmRequest>(&request_buf[..n]) {
                    match shm_request {
                        ShmRequest::MatrixMultiply { id: request_id, matrix_size, data_offset, data_len } => {
                            let data = data_pool.read_data(data_offset, data_len as usize).to_vec();

                            let start = Instant::now();
                            let size = matrix_size as usize;
                            let per_matrix_len = size * size;
                            let f64_data = bytes_as_f64_slice(&data);

                            if f64_data.len() < per_matrix_len * 2 {
                                warn!("Matrix data too short: expected {}, got {}", per_matrix_len * 2, f64_data.len());
                                continue;
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
                            };

                            let response_bytes = bincode::serialize(&response).unwrap();
                            let shm_response = ShmResponse::MatrixMultiply {
                                id: request_id,
                                response: response_bytes,
                            };
                            let payload = bincode::serialize(&shm_response)
                                .map_err(|e| {
                                    warn!("Failed to serialize response: {}", e);
                                    return;
                                })
                                .unwrap();

                            if let Err(e) = response_buffer.write(&payload) {
                                warn!("Failed to write response: {}", e);
                            }
                        }
                        _ => {
                            warn!("Unknown ShmRequest variant");
                        }
                    }
                } else if let Ok(request) = serde_json::from_slice::<EchoRequest>(&request_buf[..n]) {
                    let start = Instant::now();
                    let recv_time = Instant::now();

                    if delay > Duration::ZERO {
                        let start = Instant::now();
                        while start.elapsed() < delay {
                            std::hint::spin_loop();
                        }
                    }

                    let processing_time = start.elapsed();

                    let response = EchoResponse {
                        message: request.message,
                        timestamp_ns: recv_time.elapsed().as_nanos() as i64,
                        processing_time_us: processing_time.as_micros() as i64,
                        payload: request.payload,
                    };

                    if let Ok(payload) = serde_json::to_vec(&response) {
                        if let Err(e) = response_buffer.write(&payload) {
                            warn!("Failed to write response: {}", e);
                        }
                    }
                }
            }
            Err(_) => {}
        }
    }
}
