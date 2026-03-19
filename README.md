# gRPC 微服务 + 网关性能评测框架

基于 Tokio + tonic 的高性能微服务架构，支持多种通信方式（TCP/UDS/共享内存）和全面的性能评测。

## 项目结构

```
gateway-gRPC/
├── proto/              # Protocol Buffers 定义
├── backend/            # gRPC 后端服务
├── gateway/            # HTTP 网关服务
├── shared-memory/      # 共享内存通信模块
├── benchmark/          # 压测和分析工具
├── scripts/            # 运行脚本
└── results/            # 测试结果输出
```

## 评测维度

### 1. 端到端延迟（HTTP 视角）
- P50 / P90 / P95 / P99 / P99.9 延迟
- 不同并发连接数下的延迟曲线（C=64/256/1024）

### 2. 吞吐量（QPS）
- 在保持 P99 < 10ms 的前提下，比较各实现的最大稳定 QPS

### 3. 系统资源开销
- 系统调用次数（syscalls）
- 上下文切换次数（context switches）
- CPU user / kernel 时间比例

## 快速开始

### 1. 构建项目

```bash
cargo build --release
```

### 2. 启动后端服务

```bash
# 默认 1ms 延迟
./target/release/backend --addr 127.0.0.1:50051 --delay-us 1000
```

### 3. 启动网关

```bash
# TCP 模式
./target/release/gateway --listen-addr 127.0.0.1:8080 --backend-addr 127.0.0.1:50051 --transport tcp

# UDS 模式
./target/release/gateway --listen-addr 127.0.0.1:8080 --transport uds

# 共享内存模式
./target/release/gateway --listen-addr 127.0.0.1:8080 --transport shm
```

### 4. 运行压测

#### 使用内置压测工具
```bash
# HTTP 压测
./target/release/benchmark http --target http://127.0.0.1:8080 --connections 64 --duration-secs 30 --qps 10000

# gRPC 压测
./target/release/benchmark grpc --target http://127.0.0.1:50051 --connections 64 --duration-secs 30 --qps 10000

# 完整测试套件
./target/release/benchmark suite --gateway-url http://127.0.0.1:8080 --max-qps 20000 --target-p99-ms 10
```

#### 使用 wrk
```bash
chmod +x scripts/run_with_wrk.sh
./scripts/run_with_wrk.sh
```

#### 使用 vegeta
```bash
chmod +x scripts/run_with_vegeta.sh
./scripts/run_with_vegeta.sh
```

#### 使用 ghz
```bash
chmod +x scripts/run_with_ghz.sh
./scripts/run_with_ghz.sh
```

### 5. 系统资源监控

```bash
chmod +x scripts/monitor_resources.sh
./scripts/monitor_resources.sh 60  # 监控 60 秒
```

### 6. 完整自动化测试

```bash
chmod +x scripts/run_benchmark.sh
./scripts/run_benchmark.sh
```

## 通信方式对比

| 通信方式 | 延迟 | 吞吐量 | CPU 开销 | 适用场景 |
|---------|------|--------|----------|----------|
| TCP | 中 | 高 | 中 | 通用场景 |
| UDS | 低 | 高 | 低 | 同机部署 |
| 共享内存+eventfd | 极低 | 极高 | 极低 | 超低延迟需求 |
| 共享内存+UINTR | 最低 | 最高 | 最低 | 需要硬件支持 |

## 关键特性

### 共享内存通信
- 基于 ring buffer 的零拷贝设计
- eventfd 用于进程间通知
- 可选 UINTR 支持（需要 Intel Sapphire Rapids+）

### 性能指标收集
- HDR Histogram 实现精确的延迟百分位计算
- perf stat 收集系统调用和上下文切换
- pidstat 收集 CPU 使用率

### 压测工具支持
- 内置 Rust 压测工具（支持 HTTP/gRPC）
- wrk: HTTP 压测
- vegeta: HTTP 负载测试
- ghz: gRPC 压测

## 配置选项

### 后端服务
```
--addr          监听地址 (默认: 127.0.0.1:50051)
--delay-us      固定延迟（微秒）(默认: 1000)
--compute-iterations 计算迭代次数 (默认: 100)
```

### 网关服务
```
--listen-addr   HTTP 监听地址 (默认: 127.0.0.1:8080)
--backend-addr  后端 gRPC 地址 (默认: 127.0.0.1:50051)
--transport     传输方式: tcp/uds/shm (默认: tcp)
--uds-path      UDS 路径 (默认: /tmp/gateway.sock)
--shm-name      共享内存名称 (默认: gateway_shm)
--max-concurrent 最大并发数 (默认: 10000)
```

### 压测工具
```
--connections   并发连接数 (默认: 64)
--duration-secs 测试持续时间（秒）(默认: 30)
--qps          目标 QPS (默认: 10000)
--payload-size  请求体大小（字节）
```

## 结果分析

```bash
# 分析测试结果
./target/release/latency_analyzer \
    --input results/all_results.json \
    --output results/analysis_report.json

# 对比不同测试
./target/release/latency_analyzer \
    --input results/baseline.json \
    --compare results/optimized.json \
    --output results/comparison.json
```

## 系统要求

- Linux 内核 5.x+
- Rust 1.70+
- perf 工具（用于系统监控）
- pidstat（sysstat 包）
- 可选: wrk, vegeta, ghz

## 性能优化建议

1. **内核参数调优**
```bash
# 增加文件描述符限制
ulimit -n 65535

# 网络栈优化
sysctl -w net.ipv4.tcp_tw_reuse=1
sysctl -w net.core.somaxconn=65535
sysctl -w net.ipv4.ip_local_port_range="1024 65535"
```

2. **CPU 亲和性**
```bash
# 绑定到特定 CPU 核心
taskset -c 0-3 ./target/release/backend
taskset -c 4-7 ./target/release/gateway
```

3. **共享内存优化**
- 使用大页内存（HugePages）
- 调整共享内存大小匹配工作负载

## 许可证

MIT
