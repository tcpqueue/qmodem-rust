# QModem Rust

面向 OpenWrt 的独立模组管理服务。将 QModem 的 Shell/C 后端业务逐步迁移为 Rust，使用内嵌的 art-design-pro 管理界面和轻量 LuCI 服务控制插件。

适配范围：**移远系列、TD Tech MT5700**；目标设备：ARM64 和 x86 系列，包含 USB 与 PCIe。其他厂商不在本次适配范围内。

**当前是开发版本，尚不能完整替代 QModem。** 以 OpenWrt 24.10 为最低目标版本。
当前实现及未完成项见 [迁移进度](docs/migration-progress.md) 和 [接口核对表](docs/migration-inventory.json)。

## 当前功能

- Rust 原生串口收发、事务队列、响应恢复；按模组及端口可视化。
- USB/PCIe sysfs 发现、型号探测、端口规则与 TOML 自动登记。
- 移远与 MT5700 状态查询、SIM 切换、模式、IMEI；移远锁频和锁小区。
- 原生 AT 拨号与 netifd USB QMI/MBIM，动态接口加入指定 firewall4 区域。
- GSM7/UCS2/PDU 短信、长短信、SQLite 历史、幂等发送、SIM 索引核对删除、旧 JSON 导入。
- Telegram、Webhook、ServerChan、PushDeer、飞书及自定义转发，持久任务与限次重试。
- 开机初始化、锁小区恢复、GPIO/LED、关机重启、连通性监测、流量历史和定时清零。
- 内嵌 WebUI、TOML 原子配置、日志等级、IP/端口/网卡绑定、轻量 LuCI 管理。

已通过 87 项 Rust 测试、前端构建及服务集成检查。ARM64、x86_64、i686 静态构建成功。
COM34 上的 MT5700 已做六条只读 AT 实测，见 [测试记录](docs/mt5700-com34-test.md)；
OpenWrt 拨号与安装仍未实机验证。MHI、桥接直通、5G Ethernet 和部分状态边界仍需迁移。

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

`web/` 为 art-design-pro 上游源码子模块；实际页面在 `frontend/`，复用其 Element Plus 主题，提供模组、网络、短信、维护、调试和队列页面。`frontend/dist/` 是纳入版本管理的单页产物，已嵌入 Rust 二进制，支持 gzip 和脚本 CSP 哈希；路由器不需要 Node.js。访问服务根路径 `/` 或 `/queues`，输入访问令牌即可查看。OpenWrt 构建规则见 `packaging/openwrt`，支持 SDK 源码构建或打包预构建静态二进制，见 [OpenWrt 部署](docs/openwrt.md)。SDK 编译和实际安装待验证。

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
