# QModem Rust

面向 OpenWrt 的独立模组管理服务。将 QModem 的 Shell/C 后端业务逐步迁移为 Rust，使用内嵌的 art-design-pro 管理界面和轻量 LuCI 服务控制插件。

适配范围：**移远系列、TD Tech MT5700**；目标设备：ARM64 和 x86 系列，包含 USB 与 PCIe。其他厂商不在本次适配范围内。

**当前是开发版本，尚不能完整替代 QModem。** 已有原生串口调度、首批厂商操作、HTTP API、鉴权、TOML 设置、日志等级、网卡绑定和 SQLite 初始结构。已接入队列监控页面和频段锁定/SIM 切换；其余后台页面、拨号、短信业务、锁小区及部分适配仍在迁移，见 [功能清单](docs/parity.md) 和 [迁移缺口核对](docs/migration-audit.md)。

## 当前功能

- Rust 直接操作串口，不调用原 QModem Shell/C 服务；同端口串行、不同端口并发。
- 命令分段响应、终止符、短信提示符/PDU事务、超时后迟到响应恢复与事件流。
- `/api/v1` 提供模组配置列表、原始 AT、结构化操作、串口 SSE 事件与队列监控。
- 队列页面按模组与 AT/SMS 端口展示执行、等待、耗时和最近 32 个事务；别名共享实际串口队列。
- 移远频段锁定、卡槽能力/切换和 IMEI 写入回读；MT5700 使用专有命令及明确标注的软件卡槽记录。
- TOML 保存配置，原子更新保留其他字段与注释；SQLite 保存后续运行数据。
- 日志等级支持 error、warn、info、debug、trace、off；格式支持文本或 JSON。
- 可配置监听 IP、端口及 Linux 网络设备，通过 SO_BINDTODEVICE 限定网卡。
- LuCI 已编写启停、自启、访问令牌初始化、日志和监听设置页面，含简体中文翻译；尚未实机联调。

## WSL / Linux 开发

项目在 Linux 原生目录构建，例如 `~/projects/qmodem-rust-daemon`，不在 `/mnt/` 中执行开发操作。当前验证工具链为 Rust 1.98.1。

```sh
git submodule update --init
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo build --workspace
./scripts/smoke.sh
./scripts/test-service.sh
```

最后一项需要 Linux 的 `unshare`、`ip` 和 Node.js，验证使用临时网络命名空间。Node.js 只用于开发测试，路由器运行时不需要。

## 配置与运行

复制 `config/qmodem.example.toml` 为 `config/local.toml`，将 `storage.sqlite` 改成当前用户可写的绝对路径，然后运行：

```sh
cargo run -- --config config/local.toml check
cargo run -- --config config/local.toml init-auth
cargo run -- --config config/local.toml serve
```

首次生成的令牌只显示一次，TOML 保存哈希。非回环监听必须先配置认证。监听和日志设置修改后重启服务生效。

```toml
[server]
listen = "0.0.0.0"
port = 8088
interface = "br-lan"

[logging]
level = "info"
format = "text"
```

完整说明见 [服务设置](docs/service-settings.md) 与 [API 文档](docs/api.md)。

## 部署形态

最终业务程序由一个内嵌 Web 资源的二进制和一个 TOML 文件部署。SQLite、WAL、日志及 LuCI/procd 集成文件是允许的运行数据和系统集成文件。按需要复用 OpenWrt 内核驱动、netifd、ubus 等系统能力。

`web/` 为 art-design-pro 上游源码子模块；实际页面在 `frontend/`，复用其 Element Plus 主题，当前提供队列监控。`frontend/dist/` 是纳入版本管理的单页产物，已嵌入 Rust 二进制，支持 gzip 和脚本 CSP 哈希；路由器不需要 Node.js。访问服务根路径 `/` 或 `/queues`，输入访问令牌即可查看。OpenWrt 构建规则见 `packaging/openwrt`，SDK 编译和实际安装待验证。

## 上游与许可

版本来源见 [upstream.md](docs/upstream.md)，第三方许可范围见 [NOTICE.md](NOTICE.md)。上游资料保留原始版权声明，不将第三方组件重新标成统一的宽松许可。

## 前端开发

在 WSL 的项目目录内执行（Node.js 22.12 以上）：

```sh
cd frontend
npm ci
npm run build
cd ..
cargo build --workspace
```

修改页面后重新构建前端及 Rust，提交更新后的 `frontend/dist/`。仅构建 Rust 或 OpenWrt 包时无需安装前端依赖。队列刷新不发送 AT 查询；页面隐藏时暂停，默认每秒更新，支持手动刷新。
