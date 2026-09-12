# OpenWrt 集成

服务包为 `qmodem-rust`，安装 `/usr/sbin/qmodemd`、`/etc/qmodem-rust.toml` 和 procd init 脚本。LuCI 包为 `luci-app-qmodem-rust`，提供启停、自启、日志等级、监听网卡/IP/端口和首个访问令牌初始化。

LuCI 通过 `rc.list`/`rc.init` 和 `service.list` 管理进程，通过受限的 CLI 调用读取或原子修改 TOML，不依赖 HTTP 服务运行。自启由标准 init 软链接控制，不重复写入 UCI。日志由 procd 收进 OpenWrt 系统日志。

构建需要包含 `feeds/packages/lang/rust/rust-package.mk` 的 OpenWrt SDK/buildroot，并需要满足本项目 Rust 工具链版本。将此仓库中的两个包目录链接到 SDK 的 package 目录：

```sh
ln -s /absolute/path/qmodem-rust-daemon/packaging/openwrt/qmodem-rust package/qmodem-rust
ln -s /absolute/path/qmodem-rust-daemon/packaging/openwrt/luci-app-qmodem-rust package/luci-app-qmodem-rust
make menuconfig
make package/qmodem-rust/compile package/luci-app-qmodem-rust/compile V=s QMODEM_SOURCE_DIR=/absolute/path/qmodem-rust-daemon
```

`QMODEM_SOURCE_DIR` 指向完整仓库的 Linux 原生目录。默认值只适用于包文件直接位于仓库原目录的情况，链接到 SDK 后应显式传入。

**构建规则与 LuCI 文件尚未通过 SDK/路由器验证。** 已完成的是主机 Rust、HTTP、网卡绑定及配置测试；不能将这些文件作为已验证安装包发布。接下来需验证 Rust 工具链适配、依赖、rpcd 命令 ACL、procd 生命周期和 LuCI 实际表现。
