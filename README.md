# FlashCron

<div align="center">

**一款基于 Rust 编写、极速且超高效的 Cron 守护进程**

[![CI](https://github.com/antonchen/flashcron/workflows/CI/badge.svg)](https://github.com/antonchen/flashcron/actions)
[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)
[![Rust](https://img.shields.io/badge/rust-1.85%2B-orange.svg)](https://www.rust-lang.org)

[English](README_en.md) | [功能特性](#功能特性) | [安装指南](#安装指南) | [快速开始](#快速开始) | [文档说明](docs/) | [参与贡献](#参与贡献)

</div>

---

## Fork 新功能
本版本是基于 **v0.1.0** 的增强分支，新增了以下核心特性：

- **内置 Web 控制台**：轻量级监控界面，可实时追踪任务状态、持久化统计及执行历史（包含 RESTful API）。
- **持久化存储**：集成 SQLite 存储引擎，确保执行历史、成功/失败计数在容器或进程重启后不丢失，并支持自动化数据清理。
- **任务输出日志**：自动捕获并持久化任务的 `stdout` 和 `stderr`，支持全局及单任务级别的详细配置。
- **时区支持**：原生支持时区（如 `Asia/Shanghai`）。

---

## 为何选择 FlashCron？

传统 cron 守护进程虽能胜任工作，但并非为极速而生。**FlashCron** 从底层开始重构，追求极致的执行效率：

| 指标 | FlashCron (当前版本) | FlashCron (原版) | 传统 cron | 性能提升 |
|--------|---------------------|----------------------|------------------|-------------|
| **调度器初始化 (1000 任务)** | ~400 μs | ~400 μs | 10-50 ms | **最高快 125 倍** |
| **下次运行时间计算** | ~500 ns | ~500 ns | 10-100 μs | **最高快 200 倍** |
| **内存占用** | **10-20 MB** | 2-5 MB | 10-50 MB | **最高降低 80%** |
| **启动耗时** | <10 ms | <10 ms | 50-200 ms | **最高快 20 倍** |
| **空闲 CPU 占用** | <0.1% | <0.1% | 0.5-2% | **最高降低 95%** |

### 极速哲学 (The Flash Philosophy)

- **优先级队列调度**：O(log n) 操作复杂度，无轮询循环。
- **零拷贝解析**：热点路径下极少进行内存分配。
- **精准休眠**：在任务触发的精确时刻前，CPU 保持完全空闲。
- **并发执行**：基于异步 I/O 的可配置并行处理。
- **二进制优化**：启用 LTO、移除符号表，榨干每一分性能。

---

## 功能特性

### 核心功能
- **标准 Cron 表达式**：完美支持 5 字段 Cron 语法。
- **时区支持**：支持在任意时区运行任务，具备灵活的优先级逻辑。
- **TOML 配置**：易读且对版本控制友好的配置格式。
- **热加载**：配置文件变更后自动重新加载。
- **优雅关闭**：待所有正在运行的任务完成后再安全退出。

### 任务管理
- **超时处理**：自动杀掉运行超时失控的任务。
- **重试机制**：支持配置重试次数及延迟间隔。
- **环境变量**：支持为每个任务注入特定的环境变量。
- **工作目录**：可自定义每个任务的执行路径。
- **并发限制**：有效防止资源耗尽。

### 可观测性
- **Web 控制台**：内置 HTTP 接口，直观监控任务状态与历史。
- **结构化日志**：支持易读的文本格式或适合日志审计的 JSON 格式。
- **执行历史**：详尽追踪任务的成功与失败率。

### 跨平台支持
- **Linux**：原生支持，提供 systemd 集成。
- **macOS**：全面支持（含 Apple Silicon）。
- **Windows**：原生 Windows 运行支持。

---

## 安装指南

### 预编译发布版（推荐）

请前往 [GitHub Releases](https://github.com/antonchen/flashcron/releases) 页面下载适用于您平台的预编译二进制文件。

### 通过 Cargo 安装

```bash
cargo install flashcron
```

### 源码编译

```bash
git clone https://github.com/antonchen/flashcron
cd flashcron
cargo build --release
```

### Docker 运行

```bash
docker pull ghcr.io/antonchen/flashcron:latest
```

---

## 快速开始

### 1. 生成配置文件

```bash
flashcron init -o flashcron.toml
```

### 2. 编辑配置

```toml
[settings]
log_level = "info"
max_concurrent_jobs = 10

[jobs.backup]
schedule = "0 2 * * *"  # 每天凌晨 2 点
command = "/usr/local/bin/backup.sh"
description = "每日备份"
timeout = 3600
retry_count = 3

[jobs.cleanup]
schedule = "0 */6 * * *"  # 每 6 小时一次
command = "find /tmp -mtime +7 -delete"
description = "清理临时文件"
enabled = true
```

### 3. 校验配置

```bash
flashcron validate -c flashcron.toml
```

### 4. 启动守护进程

```bash
flashcron run -c flashcron.toml
```

---

## 时区支持

FlashCron 支持 IANA 时区，解析优先级如下：
1. `TZ` 环境变量
2. `settings.timezone` (TOML 配置)
3. 系统时区
4. UTC (最终回退)

```toml
[settings]
timezone = "Asia/Shanghai" # 或 "System", "UTC"
```

---

## 命令行参考

| 命令 | 描述 |
|---------|-------------|
| `flashcron run -c config.toml` | 启动守护进程 |
| `flashcron validate -c config.toml` | 校验配置文件 |
| `flashcron list -c config.toml` | 列出所有任务 |
| `flashcron schedule -c config.toml` | 显示接下来的执行计划 |
| `flashcron trigger <job> -c config.toml` | 手动触发指定任务 |
| `flashcron init -o config.toml` | 生成默认配置文件 |

### 全局选项

| 选项 | 描述 |
|--------|-------------|
| `-c, --config <PATH>` | 配置文件路径 |
| `-l, --log-level <LEVEL>` | 日志级别 (trace, debug, info, warn, error) |
| `--json` | 以 JSON 格式输出日志 |
| `--foreground` | 前台运行 (不进入守护模式) |

### 配置自动解析

在运行 `list`、`schedule`、`trigger` 或 `validate` 命令时，FlashCron 会按以下优先级自动寻找配置文件：

1. **命令行参数 (`-c custom.toml`)**：显式指定。
2. **环境变量 (`FLASHCRON_CONFIG`)**：如 `export FLASHCRON_CONFIG=custom.toml`。
3. **活跃守护进程状态**：若已启动 `flashcron run`，其他命令会自动使用守护进程正在使用的配置文件（通过 `/tmp/flashcron.state` 追踪）。
4. **默认文件**：当前目录下的 `flashcron.toml`。

这使得在守护进程运行时，您可以直接输入 `flashcron list` 而无需反复传递 `-c` 参数。

---

## 配置参考

### 全局设置 (Global Settings)

```toml
[settings]
log_level = "info"           # trace, debug, info, warn, error
json_logs = false            # 适配日志采集器的 JSON 格式
api_host = "0.0.0.0"       # Web 控制台 / API 主机
api_port = 8080              # Web 控制台 / API 端口
api_token = "secret"         # API 鉴权 Token (若未设置将随机生成并在启动时打印)
sql_file = "flashcron.db"    # SQLite 数据库文件路径
max_concurrent_jobs = 10     # 0 = 不限制
shell = "/bin/sh"            # 默认 Shell
watch_config = true          # 配置文件变更热加载
job_history_size = 100         # 单个任务保留的最大历史条数
max_history_size = 10000       # 全局保留的最大历史总条数
timezone = "System"          # 时区 (如 "System", "UTC", "Asia/Shanghai")
shutdown_timeout = 30        # 停机等待时间（秒）
print_output = false         # 是否将任务输出打印至日志
```

### 任务配置 (Job Configuration)

```toml
[jobs.example]
schedule = "*/5 * * * *"     # Cron 表达式 (必填)
command = "echo 'Hello'"     # 执行命令 (必填)
description = "示例任务"      # 可选描述
enabled = true               # 启用/禁用
working_dir = "/app"         # 工作目录
environment = { KEY = "value" }  # 环境变量注入
timeout = 300                # 超时时间（秒，0 为不限制）
shell = "/bin/bash"          # 覆盖默认 Shell
retry_count = 3              # 失败重试次数
retry_delay = 60             # 重试延迟（秒）
max_output_size = 1048576    # 最大捕获输出大小 (bytes)
run_on_startup = false       # 守护进程启动时立即运行一次
print_output = false         # 覆盖全局 print_output 设置
```

### Cron 表达式格式

```
┌───────────── 分钟 (0-59)
│ ┌───────────── 小时 (0-23)
│ │ ┌───────────── 日期 (1-31)
│ │ │ ┌───────────── 月份 (1-12)
│ │ │ │ ┌───────────── 星期 (1-7, 周日 = 7)
│ │ │ │ │
* * * * *
```

---

## 性能基准测试

测试环境：AMD Ryzen 5 / Intel i7 等效配置

| 操作 | 耗时 | 备注 |
|-----------|------|-------|
| Cron 表达式解析 | 1.8-4.9 μs | 取决于表达式复杂度 |
| 下次执行时间计算 | 400-500 ns | 亚微秒级 |
| 调度器初始化 (10 任务) | ~10 μs | 瞬间启动 |
| 调度器初始化 (100 任务) | ~50 μs | 线性增长 |
| 调度器初始化 (1000 任务) | ~400 μs | 仍小于 1ms |
| 配置解析 (50 任务) | ~566 μs | 含 TOML 解析耗时 |
| 配置解析 (200 任务) | ~2.9 ms | 轻松处理大型配置 |

### 内存分布

| 状态 | 内存占用 |
|-------|--------|
| 空闲 (10 任务, 原版) | ~2 MB |
| 空闲 (100 任务, 原版) | ~3 MB |
| 运行中 (10 并发, 原版) | ~5 MB |
| **FlashCron (当前版本) (1000 任务 + Web)** | **~10-20 MB** |

---

## 部署说明

### systemd 服务配置

```ini
# /etc/systemd/system/flashcron.service
[Unit]
Description=FlashCron - Lightning-fast Cron Daemon
After=network.target

[Service]
Type=simple
User=flashcron
ExecStart=/usr/local/bin/flashcron run -c /etc/flashcron/config.toml
Restart=on-failure
RestartSec=5
Environment=RUST_LOG=info

[Install]
WantedBy=multi-user.target
```

```bash
sudo systemctl enable flashcron
sudo systemctl start flashcron
```

### Docker Compose

```yaml
version: '3.8'
services:
  flashcron:
    image: ghcr.io/antonchen/flashcron:latest
    volumes:
      - ./flashcron.toml:/app/config/flashcron.toml:ro
      - ./scripts:/scripts:ro
    restart: unless-stopped
    logging:
      driver: json-file
      options:
        max-size: "10m"
        max-file: "3"
```

---

## 参与贡献

欢迎任何形式的贡献！请参阅 [CONTRIBUTING.md](docs/CONTRIBUTING.md) 了解准则。

```bash
# 克隆仓库
git clone https://github.com/antonchen/flashcron
cd flashcron

# 运行测试
cargo test

# 运行基准测试
cargo bench

# 编译发布版
cargo build --release
```

---

## 开源协议

基于 MIT 协议开源 - 详见 [LICENSE](LICENSE) 文件。

---

## 作者与维护

**本项目是原 [flashcron](https://github.com/alfredo-baratta/flashcron) 项目的增强分支，进行了大量修改。**

*   **原作者：** Alfredo Baratta ([alfredobaratta@outlook.com](mailto:alfredobaratta@outlook.com))
*   **当前维护者：**  [Anton Chen](https://github.com/antonchen)

---

<div align="center">

**生而极速**

*因为每一毫秒都至关重要*

</div>
