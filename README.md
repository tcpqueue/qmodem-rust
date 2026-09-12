# QModem Rust

面向 OpenWrt 的独立模组管理服务。以 FUjr/QModem 的功能为迁移基线，目标架构为 Rust 后端、内嵌 art-design-pro 管理页面，以及轻量 LuCI 服务控制插件。

**当前处于初始开发阶段，不能替代 QModem。** 已建立配置校验、HTTP 健康检查、SQLite 初始结构和 procd 启动脚本。厂商适配、拨号、短信业务、完整 Web 页面尚未完成，LuCI 控制页已编写但尚未上机验证。设备资料的保留不代表 Rust 驱动已经实现。

## 部署目标

- ARM64 和 x86 系列路由器；先构建 aarch64、x86_64，32 位 x86 单独验证。
- 保留上游 USB、PCIe 模组及全部已有厂商的功能支持。
- Rust 二进制内嵌 Web 静态资源，路由器无需 Node.js、Python 或 PHP。
- 使用一个 TOML 文件管理服务和业务配置。
- SQLite 保存短信、流量统计等运行数据，可自动生成数据库及 WAL 文件。
- LuCI 负责启停、开机自启、基本设置和独立后台入口。
- 按需复用 OpenWrt 内核驱动、netifd、ubus、QMI/MBIM 等系统设施。

## 本地验证

在 Linux 原生目录或 WSL 的 `~/projects/` 下执行：

```sh
git submodule update --init
cargo test --workspace
cargo run -- --config config/qmodem.example.toml check
cargo run -- --config config/qmodem.example.toml service-info
```

运行健康检查服务前，复制示例配置并将 `storage.sqlite` 改成当前用户可写的**绝对路径**，再执行：

```sh
cargo run -- --config config/local.toml serve
curl http://127.0.0.1:8088/api/health
```

初始版本仅开放回环地址上的健康检查；管理认证完成后再开放局域网管理接口。`check` 不创建数据库，也不操作模组。

## 目录

| 路径 | 用途 |
| --- | --- |
| `crates/qmodemd` | Rust 服务 |
| `config` | TOML 配置示例 |
| `data` | 从固定上游版本保留的模组识别资料和 AT 快捷命令 |
| `web` | art-design-pro 前端源码基线 |
| `packaging/openwrt` | OpenWrt 服务与 LuCI 集成 |
| `docs/architecture.md` | 架构与配置边界 |
| `docs/parity.md` | 迁移与验收清单 |
| `docs/upstream.md` | 上游版本和许可 |

## 许可

新编写的 Rust 服务与 LuCI 控制代码使用 MIT。上游 QModem 数据和 art-design-pro 保留各自许可，范围与原文见 [NOTICE.md](NOTICE.md)，不能将整个仓库的第三方资料统称为 MIT。
