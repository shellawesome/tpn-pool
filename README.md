# TPN Pool

TPN (Tao Private Network) miner pool 的 Rust 实现，编译为单一二进制文件，内嵌 SQLite 数据库和 Web 管理面板，零外部依赖。

## 特性

- 仅支持 miner pool 运行
- 内嵌 SQLite（WAL 模式），无需安装数据库
- 默认配置目录：`$HOME/.config/tpn-pool`
- 首次启动自动生成默认配置文件：`$HOME/.config/tpn-pool/.env`
- 内嵌 Web Dashboard，浏览器访问 `/dashboard`
- 可选密码认证（JWT），默认无密码即免登录
- MaxMind / IP2Location 地理定位
- HMAC-SHA256 Lease Token 签名与验证
- 单文件部署，约 13MB

## 构建

需要 Rust 1.70+。

```bash
chmod +x build.sh
./build.sh
```

产物：`./tpn-pool`

## 运行

```bash
./tpn-pool
```

首次启动时会自动创建：

- 配置文件：`$HOME/.config/tpn-pool/.env`
- 数据库文件：`$HOME/.config/tpn-pool/tpn.db`

如果设置了 `XDG_CONFIG_HOME`，则会优先使用 `$XDG_CONFIG_HOME/tpn-pool/`。

## Web 面板

启动后访问 `http://<host>:<port>/dashboard`。

- `LOGIN_PASSWORD` 为空（默认）：无需登录，直接访问
- `LOGIN_PASSWORD=yourpass`：需要密码登录

## 环境变量

### 基础配置

| 变量 | 默认值 | 说明 |
|------|--------|------|
| `SERVER_PUBLIC_PORT` | `3000` | HTTP 服务端口 |
| `SERVER_PUBLIC_PROTOCOL` | `http` | 公网协议 |
| `SERVER_PUBLIC_HOST` | `localhost` | 公网地址 |
| `DB_PATH` | `$HOME/.config/tpn-pool/tpn.db` | SQLite 数据库路径 |
| `LOG_LEVEL` | `info` | 日志级别 |

### Dashboard 认证

| 变量 | 默认值 | 说明 |
|------|--------|------|
| `LOGIN_PASSWORD` | (空) | 面板登录密码，空则免认证 |
| `JWT_SECRET` | `default-secret-change-me` | JWT 签名密钥 |

### 安全

| 变量 | 说明 |
|------|------|
| `LEASE_TOKEN_SECRET` | Lease Token HMAC 密钥 |
| `ADMIN_API_KEY` | 管理 API Key |

### 地理定位

| 变量 | 说明 |
|------|------|
| `MAXMIND_LICENSE_KEY` | MaxMind GeoLite2 许可证 |
| `IP2LOCATION_DOWNLOAD_TOKEN` | IP2Location 下载令牌 |

### 矿池配置

| 变量 | 说明 |
|------|------|
| `MINING_POOL_URL` | 矿池 URL |
| `MINING_POOL_NAME` | 矿池名称 |
| `MINING_POOL_WEBSITE_URL` | 矿池官网 |
| `MINING_POOL_REWARDS` | 奖励页面地址 |

### 支付地址

| 变量 | 说明 |
|------|------|
| `PAYMENT_ADDRESS_EVM` | EVM 收款地址 |
| `PAYMENT_ADDRESS_BITTENSOR` | Bittensor 收款地址 |

### 网络

| 变量 | 默认值 | 说明 |
|------|--------|------|
| `TPN_INTERNAL_SUBNET` | `10.13.13.0/24` | 内部子网 |
| `TPN_EXTERNAL_SUBNET` | `10.14.14.0/24` | 外部子网 |

### 守护进程

| 变量 | 默认值 | 说明 |
|------|--------|------|
| `DAEMON_INTERVAL_SECONDS` | `300` (CI 模式 `60`) | 后台任务间隔（秒） |

## API 端点

### 公共端点

| 方法 | 路径 | 说明 |
|------|------|------|
| GET | `/` | 健康检查 |
| GET | `/ping` | IP 回显 |
| GET | `/dashboard` | Web 管理面板 |
| GET | `/api/stats` | 节点状态 |
| GET | `/api/lease/new` | 向矿池申请 Worker Lease |

### Dashboard API

| 方法 | 路径 | 说明 |
|------|------|------|
| GET | `/api/auth/check` | 检查是否需要认证 |
| POST | `/api/login` | 密码登录 |
| GET | `/api/dashboard` | 面板数据（需认证） |

### Miner Pool 端点

| 方法 | 路径 | 说明 |
|------|------|------|
| POST | `/miner/broadcast/worker` | Worker 注册 |
| POST | `/miner/broadcast/worker/feedback` | Validator 反馈 |

## CI/CD

推送 `tpn-pool/` 目录变更到 `main` 分支时，GitHub Actions 自动构建 Ubuntu 22.04 amd64 二进制并发布 Release。

## 项目结构

```
tpn-pool/
  src/
    main.rs              # 入口、启动序列、守护进程
    config.rs            # 环境变量配置
    dashboard/           # Web 面板（HTML + 认证 + API）
    db/                  # SQLite 数据库层（9 张表）
    cache/               # TTL 内存缓存 + 磁盘持久化
    crypto/              # HMAC Lease Token、API Key 验证
    geo/                 # MaxMind + IP2Location 地理定位
    http/                # axum 路由 + 服务器
    scoring/             # 矿池评分 + Worker 验证
    networking/          # WireGuard / SOCKS5 / 节点通信
    api/                 # Miner pool 业务逻辑
    system/              # Shell 命令、信号处理
    locks.rs             # 命名 Mutex 注册表
    validations.rs       # 输入验证
    partnered_pools.rs   # 合作矿池检测
```
