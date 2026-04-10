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

默认构建会将 `IP2Location` zip 压缩包内嵌进 binary，首次启动时自动解压出 BIN。

```bash
cargo build --release
```

默认会读取仓库内 `ip2location_data/ip2location.zip`。如果压缩包在其他位置，可在构建时指定：

```bash
IP2LOCATION_EMBED_ARCHIVE=/abs/path/ip2location.zip \
cargo build --release
```

兼容旧变量名 `IP2LOCATION_EMBED_BIN`，但现在它同样应指向 zip 压缩包。

如果构建时找不到压缩包，构建仍会成功，但不会内嵌该数据库。

## 运行

```bash
./tpn-pool
```

显式运行子命令也可以：

```bash
./tpn-pool run
```

查看帮助：

```bash
./tpn-pool --help
```

打印注册信息，按回车确认后再提交 burned registration：

```bash
./tpn-pool register
```

打印当前私钥派生出的 hotkey/coldkey 地址、固定钱包路径和链配置：

```bash
./tpn-pool doctor
```

打印配置文件内容；如果 `.env` 不存在会先自动生成：

```bash
./tpn-pool config
```

首次启动时会自动创建：

- 配置文件：`$HOME/.config/tpn-pool/.env`
- Python shim：`$HOME/.config/tpn-pool/miner_shim.py`
- Sybil 协议包：`$HOME/.config/tpn-pool/sybil/`（内嵌，自动释放）
- 数据库文件：`$HOME/.config/tpn-pool/tpn.db`
- IP2Location 数据目录：`$HOME/.config/tpn-pool/ip2location_data/`

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
| `IP2LOCATION_DOWNLOAD_TOKEN` | IP2Location ASN IPv6 数据下载令牌（下载 zip，运行时解压到 `ip2location_data/`，未配置则仅使用 MaxMind ASN 判定） |
| `IP2LOCATION_EMBED_ARCHIVE` | 可选。构建时指定要内嵌进 binary 的 `ip2location.zip` 路径 |
| `IP2LOCATION_EMBED_BIN` | 兼容旧变量名；当前也应指向 `ip2location.zip` 路径 |

### 矿池配置

| 变量 | 说明 |
|------|------|
| `MINING_POOL_URL` | 矿池 URL |
| `MINING_POOL_NAME` | 矿池名称 |
| `MINING_POOL_WEBSITE_URL` | 矿池官网 |
| `MINING_POOL_REWARDS` | 奖励页面地址 |
| `BROADCAST_MESSAGE` | `/` 端点对外广播消息 |
| `CONTACT_METHOD` | `/` 端点对外联系方式 |

### 版本信息覆盖

`/` 健康检查端点返回的 `version` / `branch` / `hash` 默认取编译时值（`CARGO_PKG_VERSION`、硬编码 `main`、构建时从 GitHub API 拉取的 `taofu-labs/tpn-subnet` main HEAD 前 7 位）。可通过 `.env` 覆盖：

| 变量 | 说明 |
|------|------|
| `VERSION` | 覆盖 `/` 返回的 `version` |
| `BRANCH` | 覆盖 `/` 返回的 `branch` |
| `HASH` | 覆盖 `/` 返回的 `hash` |
| `TPN_SUBNET_GIT_HASH` | 构建时覆盖自动拉取的 hash（构建环境变量，非运行时） |

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

### Python Axon Shim

当 validator 侧协议不能改动时，可以启用单命令托管模式：用户只运行 `tpn-pool`，Rust 主进程会在后台自动拉起一个极薄 Python `bt.Axon` shim。`sybil.protocol` 模块已内嵌在 binary 中，启动时自动释放到配置目录，无需额外 clone `tpn-subnet` 仓库。

当前项目默认面向主网 `finney` 的 subnet `65`。

钱包目录名固定为：

- wallet name: `tpn_pool`
- hotkey name: `default`

程序启动时会根据 `.env` 中的私钥自动覆盖写入：

- `~/.bittensor/wallets/tpn_pool/hotkeys/default`
- `~/.bittensor/wallets/tpn_pool/coldkey`
- `~/.bittensor/wallets/tpn_pool/coldkeypub.txt`

| 变量 | 默认值 | 说明 |
|------|--------|------|
| `PYTHON_SHIM_ENABLED` | `false` | 是否由 Rust 自动托管 Python shim |
| `PYTHON_BIN` | `python3` | Python 可执行文件 |
| `BT_NETUID` | `65` | miner 注册所在 subnet uid |
| `BT_SUBTENSOR_NETWORK` | `finney` | subtensor network |
| `BT_SUBTENSOR_CHAIN_ENDPOINT` | `wss://entrypoint-finney.opentensor.ai:443` | chain websocket endpoint |
| `BT_HOTKEY_MNEMONIC` | (空) | hotkey 助记词，和 `BT_HOTKEY_SEED_HEX` 二选一 |
| `BT_HOTKEY_SEED_HEX` | (空) | hotkey 32 字节十六进制私钥，和 `BT_HOTKEY_MNEMONIC` 二选一 |
| `BT_COLDKEY_MNEMONIC` | (空) | coldkey 助记词，和 `BT_COLDKEY_SEED_HEX` 二选一 |
| `BT_COLDKEY_SEED_HEX` | (空) | coldkey 32 字节十六进制私钥，和 `BT_COLDKEY_MNEMONIC` 二选一 |
| `BT_AXON_PORT` | `8091` | shim 对外 axon 端口 |
| `BT_EXTERNAL_IP` | 自动探测 | 可选显式外网 IP；未设置时默认执行 `curl 3.0.3.0` 并读取返回 JSON 中的 `ip` 字段 |
| `BT_FORCE_VALIDATOR_PERMIT` | `true` | 仅允许 validator permit 请求 |
| `BT_ALLOW_NON_REGISTERED` | `false` | 是否允许未注册 hotkey |
| `PYTHON_SHIM_RESTART_DELAY_SECONDS` | `5` | shim 崩溃后的重启间隔 |

启用后，Rust 会：

- 先启动本地 HTTP backend
- 等待 backend 就绪
- 自动运行 `miner_shim.py`
- 汇总 shim stdout/stderr 到同一日志流
- shim 连续异常退出 3 次后，整体进程返回失败

启动前还会做一轮 shim 依赖自检，失败会直接退出：

- `PYTHON_BIN --version` 可执行
- `import bittensor` 成功
- 内嵌的 `sybil.protocol` 可正常导入
- 自动生成后的 wallet 根目录、wallet 目录、`hotkeys/default`、`coldkeypub.txt` 存在

### Register 子命令

`tpn-pool register` 会使用 `.env` 中的 `BT_*` 配置执行 subnet 注册。

当前默认配置是：

- `BT_SUBTENSOR_NETWORK=finney`
- `BT_SUBTENSOR_CHAIN_ENDPOINT=wss://entrypoint-finney.opentensor.ai:443`
- `BT_NETUID=65`

执行流程：

- 读取 `BT_SUBTENSOR_NETWORK` / `BT_SUBTENSOR_CHAIN_ENDPOINT`
- 先把 `.env` 中的 hotkey/coldkey 私钥写入固定默认 wallet
- 从 `~/.bittensor/wallets/tpn_pool/` 加载 wallet
- 查询当前 hotkey 是否已经注册到 `BT_NETUID`
- 打印 network、endpoint、wallet、hotkey、当前状态
- 等待你按回车确认
- 回车后调用链上 `burned_register`

必需配置：

- `BT_NETUID`，当前项目默认是 `65`
- `BT_HOTKEY_MNEMONIC` 或 `BT_HOTKEY_SEED_HEX`
- `BT_COLDKEY_MNEMONIC` 或 `BT_COLDKEY_SEED_HEX`
- 可选 `BT_SUBTENSOR_CHAIN_ENDPOINT`，默认是 `wss://entrypoint-finney.opentensor.ai:443`

### Doctor 子命令

`tpn-pool doctor` 会：

- 先按 `.env` 私钥重建固定默认钱包文件
- 打印当前使用的 network / endpoint / netuid
- 打印派生出的 hotkey ss58 和 coldkey ss58
- 打印 `~/.bittensor/wallets/tpn_pool/` 下关键文件路径及是否存在

## API 端点

### 公共端点

| 方法 | 路径 | 说明 |
|------|------|------|
| GET | `/` | 健康检查 |
| GET | `/ping` | IP 回显 |
| GET | `/dashboard` | Web 管理面板 |
| GET | `/api/stats` | 节点状态 |
| GET | `/api/lease/new` | 向矿池申请 Worker Lease |

`/api/lease/new` 关键参数与限制：

- `type=wireguard|socks5`
- `format=json|text`
- `connection_type=any|datacenter|residential`
- `geo=ANY|<ISO 国家码>`，不可用国家会直接返回 `400`
- `whitelist` / `blacklist` 必须是合法 IPv4 列表
- `lease_token` 续租要求已配置 `LEASE_TOKEN_SECRET`

### Dashboard API

| 方法 | 路径 | 说明 |
|------|------|------|
| GET | `/api/auth/check` | 检查是否需要认证 |
| POST | `/api/login` | 密码登录 |
| GET | `/api/dashboard` | 面板数据（需认证） |

### Miner Pool 端点

| 方法 | 路径 | 说明 |
|------|------|------|
| POST | `/miner/broadcast/worker` | Worker 注册。只有通过 mining pool membership、版本、WireGuard/SOCKS5 连通性、IP 冲突检查的 Worker 才会标记为 `up` |
| POST | `/miner/broadcast/worker/feedback` | Validator 反馈。记录 validator uid/ip、composite scores、每个 worker 的 status/failure_code/error 以及 up/down/cheat 汇总 |

## 日志

### 矿池注册

启动时会向每个 validator 分别调用 `/validator/broadcast/mining_pool` 和 `/validator/broadcast/workers`，区分 HTTP 非 2xx、响应体 `error`、`success != true` 三种失败形态，单条失败不会阻塞其他 validator：

```
Registering mining pool with N validators: {...}
Registered mining pool with validator <uid>@<ip> at <url>
Failed to register mining pool with validator <uid>@<ip> at <url>: HTTP 403: {"error":"Requester not a known miner"}
Registered mining pool successfully with X validators, failed: Y
Registered N workers with validators, successful: X, failed: Y
```

如果全部 validator 都回 `HTTP 403: Requester not a known miner`，通常说明 hotkey 的 axon IP 没有提交到链上 metagraph，需要检查 `BT_EXTERNAL_IP` 与 axon serve 状态。

### Validator 评分反馈

`/miner/broadcast/worker/feedback` 被 validator 调用时日志形如：

```
Received worker feedback from validator <uid> (<ip>)
Validator <uid> composite scores: score=.. stability=.. size=.. performance=.. geo=..
Validator <uid> scored worker <worker_ip>: status=up failure_code= error=
Updated N workers from validator <uid> feedback (U up, D down, C cheat)
```

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
