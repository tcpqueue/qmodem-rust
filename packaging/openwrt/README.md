# OpenWrt 集成

`qmodem-rust/files/qmodem-rust.init` 使用 procd 管理 `/usr/sbin/qmodemd`，读取 `/etc/qmodem-rust.toml`。自启使用标准 `/etc/init.d/qmodem-rust enable`，配置不复制到 UCI。

`luci-app-qmodem-rust` 包含 LuCI 服务页、菜单和 ACL。服务页使用 `rc.list`/`rc.init` 管理服务，通过 qmodemd 的 `service-info` 和 `set-service` CLI 读取或原子更新 TOML，不依赖 HTTP 服务处于运行状态。

目前尚未完成 OpenWrt Rust 交叉编译包规则，也未在路由器上验证 LuCI/rpcd/procd。这些文件不能作为已经通过实机测试的安装包发布。后续需加入 qmodem-rust 包的 Rust 构建规则、配置安装与 conffiles 声明，验证 rpcd 的命令 ACL 匹配、服务返回结构与不同 OpenWrt 版本兼容性。
