# FlashCron

<div align="center">

**一款基于 Rust 编写、面向 Linux 容器的 Cron 守护进程**

[![CI](https://github.com/antonchen/flashcron/workflows/CI/badge.svg)](https://github.com/antonchen/flashcron/actions)
[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)
[![Rust](https://img.shields.io/badge/rust-1.89%2B-orange.svg)](https://www.rust-lang.org)

[English](README_en.md) | [功能特性](#功能特性) | [安装指南](#安装指南) | [快速开始](#快速开始) | [文档说明](docs/) | [参与贡献](#参与贡献)

</div>

---

## 功能特性

### 核心功能
- **标准 Cron 表达式**：完美支持 5 字段 Cron 语法。
- **时区支持**：支持在任意时区运行任务，具备灵活的优先级逻辑。
- **TOML 配置**：易读且对版本控制友好的配置格式。
- **热加载**：配置文件变更后自动重新加载。
- **优雅关闭**：按 `shutdown_timeout` 等待正在运行的任务。

### 任务管理
- **Web 任务管理**：通过 Dashboard 新增/编辑任务，保存后立即应用。
- **独立脚本管理**：编辑持久目录中的脚本文件，并从任务表单选择填入 command。
- **依赖管理**：通过 Dashboard 添加或移除 APT/PIP 依赖、配置软件源，并在容器启动时从 SQLite 清单恢复依赖。
- **超时处理**：自动杀掉运行超时失控的任务。
- **重试机制**：支持配置重试次数及延迟间隔。
- **环境变量**：支持为每个任务注入特定的环境变量。
- **工作目录**：可自定义每个任务的执行路径。
- **并发限制**：有效防止资源耗尽。

### 可观测性
- **Web 控制台**：内置 HTTP 接口，直观监控任务状态与历史。
- **结构化日志**：支持易读的文本格式或适合日志审计的 JSON 格式。
- **执行历史**：详尽追踪任务的成功与失败率。

### Linux 支持
- **仅支持 Linux**：发布 x86_64 GNU、x86_64 musl 和 aarch64 GNU 二进制。

---

## 安装指南

### Docker 运行（推荐）

```bash
docker pull ghcr.io/antonchen/flashcron:latest
```

### 预编译版

请前往 [GitHub Releases](https://github.com/antonchen/flashcron/releases) 页面下载 Linux 预编译二进制文件。

每个受支持的 Linux 目标提供三种二进制：默认资产名（如 `flashcron-linux-x86_64.tar.gz`）是纯守护进程版；`-web` 后缀版包含 Web 控制台与 API；`-dep-manager` 后缀版还包含依赖管理和启动恢复能力。目标包括 x86_64 GNU、x86_64 musl 和 aarch64 GNU。

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
sql_file = "flashcron.db"
script_dir = "scripts"
```

Cargo 默认构建启用 `web`：任务通过 Dashboard 创建和编辑，配置中的 `[jobs.*]` 会被忽略并在日志中警告。源码 Dockerfile 默认启用 `dep-manager`，包含 Web 和依赖管理功能。要使用 TOML 管理任务，请使用 `--no-default-features` 构建；core 版本保持 TOML jobs 的加载和热重载。

配置只接受当前 feature profile 定义的字段；已删除或不支持的配置项会导致校验失败，不会被静默忽略。

Web 界面和依赖恢复页跟随浏览器的首选语言：中文环境显示简体中文，其它语言显示英文。任务名、脚本内容和命令输出保留原文。

### 3. 校验配置

```bash
flashcron validate -c flashcron.toml
```

### 4. 启动守护进程

```bash
flashcron run -c flashcron.toml
```

默认即为前台运行。若需要后台模式：

```bash
flashcron run -c flashcron.toml --daemon
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
| `flashcron run -c config.toml` | 前台启动守护进程 |
| `flashcron run -c config.toml --daemon` | 后台启动守护进程 |
| `flashcron validate -c config.toml` | 校验配置文件 |
| `flashcron list -c config.toml` | 列出所有任务 |
| `flashcron schedule -c config.toml` | 显示接下来的执行计划 |
| `flashcron trigger <job>` | 通过本地 IPC 手动触发指定任务 |
| `flashcron status` | 通过本地 IPC 查看守护进程状态 |
| `flashcron stop` | 通过本地 IPC 请求守护进程优雅停止 |
| `flashcron history` | 通过本地 IPC 查看执行历史 |
| `flashcron init -o config.toml` | 生成默认配置文件 |

### 全局选项

| 选项 | 描述 |
|--------|-------------|
| `-c, --config <PATH>` | 配置文件路径 |
| `-l, --log-level <LEVEL>` | 日志级别 (trace, debug, info, warn, error) |
| `--json` | 以 JSON 格式输出日志 |
| `--daemon` | 后台运行 |

### 本地控制通道

`trigger`、`status`、`stop` 和 `history` 都通过本地 Unix socket 与守护进程通信，因此在默认前台模式和 `--daemon` 后台模式下都可用。

---

## 配置参考

### 全局设置 (Global Settings)

`api_host`、`api_port`、`api_token`、`script_dir`、`job_history_size` 和 `max_history_size` 仅适用于启用 `web` 的构建；core 配置应省略这些字段。

```toml
[settings]
working_dir = "/app"        # 可选，所有任务的默认工作目录
log_level = "info"           # trace, debug, info, warn, error
json_logs = false            # 适配日志采集器的 JSON 格式
# Web only
api_host = "0.0.0.0"       # Web 控制台 / API 主机
api_port = 8080              # Web 控制台 / API 端口
api_token = "secret"         # API 鉴权 Token (若未设置将随机生成并写入配置文件)
# failure_hook = "/usr/local/bin/flashcron-failure-hook"  # 最终失败时调用，JSON 从 stdin 传入
sql_file = "flashcron.db"    # SQLite 数据库文件路径
script_dir = "scripts"       # Web 脚本目录；新建/编辑的文件设为 0755
max_concurrent_jobs = 10     # 0 = 不限制
shell = "/bin/sh"            # 默认 Shell
job_history_size = 100         # 单个任务保留的最大历史条数
max_history_size = 10000       # 全局保留的最大历史总条数，0 = 不保存执行历史
timezone = "System"          # 时区 (如 "System", "UTC", "Asia/Shanghai")
shutdown_timeout = 30        # 停机等待时间（秒）
print_output = false         # 是否将任务输出打印至日志
```

### 任务配置 (Job Configuration)

以下 `[jobs.*]` 配置仅用于不启用 `web` 的 core 版本。Web 模式的任务元数据保存在 SQLite，并通过 Dashboard 管理。

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
user = "deploy"              # 可选；FlashCron 以 root 运行时指定任务用户，留空则使用 nobody；非 root 运行时忽略
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

---

## 部署说明

### Docker Compose

```yaml
services:
  flashcron:
    image: antonhub/flashcron:latest
    container_name: flashcron
    restart: always
    environment:
      - TZ=Asia/Shanghai
    volumes:
      - ./flashcron.toml:/data/config.toml:ro
      - /storage/flashcron:/data
    logging:
      driver: json-file
      options:
        max-size: "10m"
        max-file: "3"
```

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

**Linux 容器中的周期任务调度**

</div>
