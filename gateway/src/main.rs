use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::Result;
use clap::{Parser, ValueEnum};
use http::{Request, Response};
use hyper::body::Incoming;
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper_util::rt::TokioIo;
use http_body_util::BodyExt;
use prometheus::{register_counter, register_histogram, Counter, Histogram};

use proto::{EchoRequest, EchoResponse, MatrixMultiplyRequest, MatrixMultiplyResponse};
use shared_memory::generate_matrix;
use tokio::net::{TcpListener, TcpStream, UnixListener, UnixStream};
use tokio::runtime::Builder;
use tonic::transport::{Channel, Endpoint, Uri};
use tracing::{error, info, warn};
use uintr::affinity::{init_uipi_core, build_tokio_worker_affinity};

mod transport;
mod metrics;

#[cfg(target_os = "linux")]
mod shm_transport_uds;
#[cfg(target_os = "linux")]
mod shm_transport_eventfd;
#[cfg(target_os = "linux")]
mod shm_transport_uintr;

#[cfg(target_os = "linux")]
mod uintr_client;
#[cfg(target_os = "linux")]
use uintr_client::UintrClient;

use metrics::Metrics;
use transport::Transport;

lazy_static::lazy_static! {
    static ref HTTP_REQUESTS_TOTAL: Counter = register_counter!(
        "gateway_http_requests_total",
        "Total HTTP requests received"
    ).unwrap();

    static ref HTTP_REQUEST_DURATION: Histogram = register_histogram!(
        "gateway_http_request_duration_seconds",
        "HTTP request duration in seconds",
        prometheus::exponential_buckets(0.0001, 2.0, 20).unwrap()
    ).unwrap();

    static ref GRPC_REQUESTS_TOTAL: Counter = register_counter!(
        "gateway_grpc_requests_total",
        "Total gRPC requests forwarded"
    ).unwrap();

    static ref GRPC_REQUEST_DURATION: Histogram = register_histogram!(
        "gateway_grpc_request_duration_seconds",
        "gRPC request duration in seconds",
        prometheus::exponential_buckets(0.0001, 2.0, 20).unwrap()
    ).unwrap();
}

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
    #[arg(short, long, default_value = "127.0.0.1:8080")]
    listen_addr: String,

    #[arg(short, long, default_value = "127.0.0.1:50051")]
    backend_addr: String,

    #[arg(short, long, value_enum, default_value = "tcp")]
    transport: TransportType,

    #[arg(long, default_value = "/tmp/gateway.sock")]
    uds_path: PathBuf,

    #[arg(long, default_value = "gateway_shm")]
    shm_name: String,

    #[arg(long, default_value_t = 10000)]
    max_concurrent: usize,
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

/// Gateway service that forwards HTTP requests to gRPC backend
pub struct Gateway {
    transport: Arc<transport::TransportEnum>,
    metrics: Arc<Metrics>,
}

impl Gateway {
    pub fn new(transport: Arc<transport::TransportEnum>) -> Self {
        Self {
            transport,
            metrics: Arc::new(Metrics::new()),
        }
    }

    /// Handle HTTP request and forward to gRPC backend
    async fn handle_request(&self, req: Request<Incoming>) -> Result<Response<String>, hyper::Error> {
        let start = Instant::now();
        HTTP_REQUESTS_TOTAL.inc();

        let method = req.method().clone();
        let uri = req.uri().clone();
        
        info!("Received {} request to {}", method, uri);

        // Extract request body
        let body_bytes = match self.collect_body(req).await {
            Ok(bytes) => bytes,
            Err(e) => {
                error!("Failed to read request body: {}", e);
                return Ok(Response::builder()
                    .status(400)
                    .body("Bad Request".to_string())
                    .unwrap());
            }
        };

        // Check if it's a matrix multiply request
        if uri.path().contains("/matrix") {
            let matrix_size = if body_bytes.is_empty() {
                128
            } else {
                match serde_json::from_slice::<serde_json::Value>(&body_bytes) {
                    Ok(json) => json.get("matrix_size")
                        .and_then(|v| v.as_i64())
                        .unwrap_or(128) as usize,
                    Err(_) => 128,
                }
            };

            let size = matrix_size as usize;
            let a = generate_matrix(size);
            let b = generate_matrix(size);

            let matrix_a_bytes = bincode::serialize(&a)
                .unwrap_or_default();
            let matrix_b_bytes = bincode::serialize(&b)
                .unwrap_or_default();

            let matrix_req = MatrixMultiplyRequest {
                matrix_size: size as i32,
                matrix_a: matrix_a_bytes,
                matrix_b: matrix_b_bytes,
                timestamp_ns: start.elapsed().as_nanos() as i64,
            };

            // Forward to backend
            let grpc_start = Instant::now();
            GRPC_REQUESTS_TOTAL.inc();

            let response = match self.transport.matrix_multiply(matrix_req).await {
                Ok(resp) => {
                    GRPC_REQUEST_DURATION.observe(grpc_start.elapsed().as_secs_f64());
                    
                    let json_resp = serde_json::json!({
                        "checksum": resp.checksum,
                        "processing_time_us": resp.processing_time_us,
                        "timestamp_ns": resp.timestamp_ns,
                        "gflops": resp.gflops,
                    });

                    Response::builder()
                        .status(200)
                        .header("Content-Type", "application/json")
                        .body(json_resp.to_string())
                        .unwrap()
                }
                Err(e) => {
                    error!("Backend call failed: {}", e);
                    Response::builder()
                        .status(503)
                        .body(format!("Service Unavailable: {}", e))
                        .unwrap()
                }
            };

            HTTP_REQUEST_DURATION.observe(start.elapsed().as_secs_f64());
            return Ok(response);
        }

        // Regular echo request
        // Parse request
        let echo_req = if body_bytes.is_empty() {
            EchoRequest {
                message: uri.path().to_string(),
                timestamp_ns: start.elapsed().as_nanos() as i64,
                payload: vec![],
            }
        } else {
            match serde_json::from_slice::<serde_json::Value>(&body_bytes) {
                Ok(json) => EchoRequest {
                    message: json.get("message")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string(),
                    timestamp_ns: start.elapsed().as_nanos() as i64,
                    payload: body_bytes.to_vec(),
                },
                Err(_) => EchoRequest {
                    message: String::from_utf8_lossy(&body_bytes).to_string(),
                    timestamp_ns: start.elapsed().as_nanos() as i64,
                    payload: body_bytes.to_vec(),
                },
            }
        };

        // Forward to backend
        let grpc_start = Instant::now();
        GRPC_REQUESTS_TOTAL.inc();

        let response = match self.transport.call(echo_req).await {
            Ok(resp) => {
                GRPC_REQUEST_DURATION.observe(grpc_start.elapsed().as_secs_f64());
                
                let json_resp = serde_json::json!({
                    "message": resp.message,
                    "processing_time_us": resp.processing_time_us,
                    "timestamp_ns": resp.timestamp_ns,
                });

                Response::builder()
                    .status(200)
                    .header("Content-Type", "application/json")
                    .body(json_resp.to_string())
                    .unwrap()
            }
            Err(e) => {
                error!("Backend call failed: {}", e);
                Response::builder()
                    .status(503)
                    .body(format!("Service Unavailable: {}", e))
                    .unwrap()
            }
        };

        HTTP_REQUEST_DURATION.observe(start.elapsed().as_secs_f64());
        Ok(response)
    }

    async fn collect_body(&self, req: Request<Incoming>) -> Result<Vec<u8>> {
        let body = req.into_body();
        let bytes = body.collect().await?.to_bytes();
        Ok(bytes.to_vec())
    }
}

/// Create HTTP server
async fn run_http_server(
    addr: SocketAddr,
    gateway: Arc<Gateway>,
) -> Result<()> {
    let listener = TcpListener::bind(addr).await?;
    info!("HTTP server listening on {}", addr);

    loop {
        let (stream, peer_addr) = listener.accept().await?;
        let gateway = gateway.clone();

        tokio::spawn(async move {
            let io = TokioIo::new(stream);
            let service = service_fn(move |req| {
                let gateway = gateway.clone();
                async move { gateway.handle_request(req).await }
            });

            if let Err(e) = http1::Builder::new()
                .serve_connection(io, service)
                .await
            {
                warn!("Error serving connection from {}: {}", peer_addr, e);
            }
        });
    }
}

fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    let uipi_core = init_uipi_core();
    let total_cpus = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(1);
    let worker_threads = if total_cpus > 1 { total_cpus - 1 } else { 1 };

    let rt = Builder::new_multi_thread()
        .enable_all()
        .build()?;
        // .worker_threads(worker_threads)
        // .event_interval(event_interval)
        // .on_thread_start(build_tokio_worker_affinity(uipi_core))

    rt.block_on(async_main())
}

async fn async_main() -> Result<()> {
    let args = Args::parse();

    // Create transport based on configuration
    let transport: Arc<transport::TransportEnum> = match args.transport {
        TransportType::Tcp => {
            info!("Using TCP transport to backend {}", args.backend_addr);
            Arc::new(transport::TransportEnum::Tcp(transport::TcpTransport::new(&args.backend_addr).await?))
        }
        TransportType::Uds => {
            info!("Using UDS transport at {:?}", args.uds_path);
            Arc::new(transport::TransportEnum::Uds(transport::UdsTransport::new(&args.uds_path).await?))
        }
        TransportType::Shm => {
            info!("Using Shared Memory transport with name {}", args.shm_name);
            Arc::new(transport::TransportEnum::Shm(transport::ShmTransport::new(&args.shm_name).await?))
        }
        #[cfg(target_os = "linux")]
        TransportType::ShmUds => {
            info!("Using SHM transport with UDS notification (name: {})", args.shm_name);
            Arc::new(transport::TransportEnum::ShmUds(transport::ShmTransportUds::new(&args.shm_name).await?))
        }
        #[cfg(target_os = "linux")]
        TransportType::ShmEventfd => {
            info!("Using Eventfd Shared Memory transport with name {}", args.shm_name);
            Arc::new(transport::TransportEnum::ShmEventfd(transport::ShmTransportEventfd::new(&args.shm_name).await?))
        }
        #[cfg(target_os = "linux")]
        TransportType::ShmUintr => {
            info!("Using SHM transport with UINTR notification (name: {})", args.shm_name);
            Arc::new(transport::TransportEnum::ShmUintr(transport::ShmTransportUintr::new(&args.shm_name).await?))
        }
        #[cfg(target_os = "linux")]
        TransportType::Uintr => {
            info!("Using SHM transport with UINTR notification (name: {})", args.shm_name);
            Arc::new(transport::TransportEnum::ShmUintr(transport::ShmTransportUintr::new(&args.shm_name).await?))
        }
    };

    let gateway = Arc::new(Gateway::new(transport));

    let addr: SocketAddr = args.listen_addr.parse()?;
    run_http_server(addr, gateway).await?;

    Ok(())
}
