# 性能对比报告：TCP vs UDS vs SHM

## 测试配置

- **后端延迟**: 1000μs (1ms)
- **测试时长**: 10秒
- **并发连接数**: 64
- **线程数**: 4
- **测试工具**: wrk

## 性能对比总览

| 传输方式 | QPS (请求/秒) | 平均延迟 | P50延迟 | P90延迟 | P99延迟 |
|---------|--------------|---------|---------|---------|---------|
| **TCP** | 18,210.83 | 3.50ms | 3.45ms | 4.48ms | 5.87ms |
| **UDS** | 19,142.38 | 3.32ms | 3.25ms | 4.18ms | 5.37ms |
| **SHM** | 476.37 | 145.77ms | 98.09ms | 288.16ms | 526.35ms |

## 详细分析

### 1. TCP (传输控制协议)

**性能指标**:
- QPS: 18,210.83 req/s
- 平均延迟: 3.50ms
- P99延迟: 5.87ms
- 吞吐量: 3.18 MB/s

**特点**:
- ✅ 稳定可靠，经过广泛测试
- ✅ 支持跨网络通信
- ✅ 延迟分布均匀（标准差 0.86ms）
- ❌ 需要经过网络协议栈
- ❌ 有TCP连接开销

**适用场景**:
- 跨机器通信
- 需要可靠传输的场景
- 网络环境复杂的情况

### 2. UDS (Unix Domain Socket)

**性能指标**:
- QPS: 19,142.38 req/s
- 平均延迟: 3.32ms
- P99延迟: 5.37ms
- 吞吐量: 3.34 MB/s

**性能提升**:
- 相比TCP: **+5.1% QPS**
- 延迟降低: **-5.1%**

**特点**:
- ✅ 性能优于TCP
- ✅ 延迟更低，抖动更小（标准差 0.74ms）
- ✅ 绕过网络协议栈
- ✅ 仅限本机通信
- ❌ 无法跨机器使用

**适用场景**:
- 本机进程间通信
- 微服务网关与后端同机部署
- 需要低延迟的场景

### 3. SHM (共享内存)

**性能指标**:
- QPS: 476.37 req/s
- 平均延迟: 145.77ms
- P99延迟: 526.35ms
- 吞吐量: 85.13 KB/s

**性能对比**:
- 相比TCP: **-97.4% QPS**
- 相比UDS: **-97.5% QPS**

**特点**:
- ✅ 理论上零拷贝，最低延迟
- ✅ 无需序列化/反序列化（如果实现得当）
- ❌ 当前实现使用轮询，效率低
- ❌ 延迟极高，不适合生产环境
- ❌ 需要复杂的同步机制

**问题分析**:
当前SHM实现存在以下问题：
1. **轮询开销大**: 使用 `tokio::time::sleep(50μs)` 进行轮询，导致大量CPU空转
2. **单请求处理**: 当前实现是串行的，无法并发处理多个请求
3. **序列化开销**: 使用JSON序列化，而不是零拷贝
4. **缺乏同步机制**: 没有使用eventfd或信号量进行高效通知

**改进建议**:
1. 使用共享的eventfd或POSIX信号量进行通知
2. 实现请求-响应关联机制，支持并发请求
3. 使用bincode或cap'n proto替代JSON序列化
4. 实现批量处理机制

## 性能排名

### QPS排名（从高到低）
1. **UDS**: 19,142.38 req/s (100%)
2. **TCP**: 18,210.83 req/s (95.1%)
3. **SHM**: 476.37 req/s (2.5%)

### 延迟排名（从低到高）
1. **UDS**: 3.32ms (100%)
2. **TCP**: 3.50ms (105.4%)
3. **SHM**: 145.77ms (4390.6%)

## 结论与建议

### 生产环境推荐

**首选方案**: **UDS**
- 在本机通信场景下，UDS提供最佳性能
- 延迟最低，吞吐量最高
- 实现简单，稳定可靠

**备选方案**: **TCP**
- 需要跨机器通信时使用
- 性能略低于UDS，但差异不大（~5%）
- 更灵活，支持分布式部署

**不推荐**: **SHM**（当前实现）
- 性能远低于预期
- 需要重新设计和实现
- 仅用于实验和学习目的

### 优化建议

#### 对于TCP
- 启用TCP_NODELAY（已启用）
- 调整内核参数：`net.core.somaxconn`, `net.ipv4.tcp_max_syn_backlog`
- 考虑使用连接池

#### 对于UDS
- 已接近最优性能
- 可以调整文件权限和路径
- 考虑使用abstract socket namespace

#### 对于SHM
- **必须重新设计**: 当前实现不可用于生产
- 使用eventfd或Futex进行同步
- 实现并发请求处理
- 使用零拷贝序列化
- 考虑使用SPSC（单生产者单消费者）队列

## 测试环境

- 操作系统: Linux
- CPU: [需要补充]
- 内存: [需要补充]
- Rust版本: [需要补充]
- Tokio版本: 1.50.0

## 附录：原始测试数据

### TCP详细数据
```
Running 10s test @ http://127.0.0.1:8080/echo
  4 threads and 64 connections
  Thread Stats   Avg      Stdev     Max   +/- Stdev
    Latency     3.50ms    0.86ms  21.59ms   74.17%
    Req/Sec     4.58k   250.36     5.21k    70.00%
  Latency Distribution
     50%    3.45ms
     75%    3.96ms
     90%    4.48ms
     99%    5.87ms
  182205 requests in 10.01s, 31.80MB read
Requests/sec:  18210.83
Transfer/sec:      3.18MB
```

### UDS详细数据
```
Running 10s test @ http://127.0.0.1:8080/echo
  4 threads and 64 connections
  Thread Stats   Avg      Stdev     Max   +/- Stdev
    Latency     3.32ms  742.21us  18.62ms   76.06%
    Req/Sec     4.81k   210.65     5.31k    69.25%
  Latency Distribution
     50%    3.25ms
     75%    3.69ms
     90%    4.18ms
     99%    5.37ms
  191611 requests in 10.01s, 33.44MB read
Requests/sec:  19142.38
Transfer/sec:      3.34MB
```

### SHM详细数据
```
Running 10s test @ http://127.0.0.1:8080/echo
  4 threads and 64 connections
  Thread Stats   Avg      Stdev     Max   +/- Stdev
    Latency   145.77ms  110.14ms 948.32ms   87.14%
    Req/Sec   119.60     17.55   170.00     59.50%
  Latency Distribution
     50%   98.09ms
     75%  192.37ms
     90%  288.16ms
     99%  526.35ms
  4769 requests in 10.01s, 852.27KB read
Requests/sec:    476.37
Transfer/sec:     85.13KB
```

---

**测试日期**: 2026-03-20  
**测试脚本**: [scripts/test_all_simple.sh](../scripts/test_all_simple.sh)
