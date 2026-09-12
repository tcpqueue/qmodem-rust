# OpenWrt 部署

最低目标版本为 OpenWrt 24.10，使用 procd、netifd、ubus 和 firewall4。
ARM64 与 x86_64 使用 musl 静态链接，SQLite、TLS、WebUI 编入 qmodemd。
无需在路由器安装 Node.js、Python、Zig 或 Rust。内核串口、USB/PCIe 网络驱动由系统提供。

## 构建二进制

在 Linux 原生文件系统内构建，不要使用 /mnt 路径。
安装 Rust 1.98 及 Zig 0.14.1 后：

    ZIG=/path/to/zig scripts/build-musl.sh
    # 32 位 x86 可单独构建：
    ZIG=/path/to/zig scripts/build-musl.sh i686

脚本使用 Zig 编译 ring 和 SQLite 的 C 代码，用 Rust 自带的 LLD 链接。
成品位于 target/<architecture>-unknown-linux-musl/release/qmodemd。
脚本检查 ELF 中没有解释器或动态库依赖。

## 通过 OpenWrt SDK 打包

24.10 SDK 自带的 Rust 工具链可能早于项目要求。可在 WSL 构建静态二进制，
然后让相应 SDK 只负责生成安装包。将 packaging/openwrt 中两个软件包放入 SDK 的
package 目录，在 menuconfig 选中 qmodem-rust 和 luci-app-qmodem-rust：

    make package/qmodem-rust/compile V=s       QMODEM_SOURCE_DIR=/home/user/projects/qmodem-rust-daemon       QMODEM_PREBUILT_BIN=/home/user/projects/qmodem-rust-daemon/target/aarch64-unknown-linux-musl/release/qmodemd

    make package/luci-app-qmodem-rust/compile V=s

QMODEM_PREBUILT_BIN 必须与 SDK 架构一致，打包时再次校验 ELF。
不传此变量时使用 SDK 的 Rust 工具链从源码构建，要求 Rust >= 1.98。
新版本 OpenWrt 的安装包格式由所用 SDK 决定。

USB QMI 可选 qmodem-rust-qmi（uqmi 和 QMI 驱动）；
USB MBIM 可选 qmodem-rust-mbim（umbim 和 MBIM 驱动）。
ECM/NCM/RNDIS 需要对应内核网络驱动，IPv6 DHCP 需要 odhcp6c。
PCIe MHI 的驱动和拨号路径仍在迁移核对中。

## 运行与配置

服务和 LuCI 共用 /etc/qmodem-rust.toml。运行前可执行：

    qmodemd --config /etc/qmodem-rust.toml check
    qmodemd --config /etc/qmodem-rust.toml init-auth
    /etc/init.d/qmodem-rust enable
    /etc/init.d/qmodem-rust start

LuCI 控制启停、自启动、监听地址、端口、网络接口、日志等级和格式。
WebUI 在配置的监听地址和端口提供服务。非回环监听需要访问令牌。
防火墙放行后台端口仍遵循路由器现有区域访问规则。

拨号动态接口默认加入 wan 区域，使用该区域已有的 NAT 和转发策略；
network.firewall_zone 可改为已存在的其他区域，空字符串表示不加入任何区域。
不会替换已有的静态逻辑接口。配置了 shutdown_reboot 的模组只在路由器关机路径重启，
普通服务重启不执行这个动作。

SQLite 路径可移至持久存储挂载点；流量采样间隔和保留天数可在后台设置。
短信转发采用持久任务和限次重试；收端是否按 Idempotency-Key 去重取决于服务商。

## 验证边界

已验证 WSL 单元测试、PTY 模拟器、前端构建和两种架构的静态链接。
尚未完成 OpenWrt SDK 安装、真实 netifd 拨号或 USB/PCIe 模组联调；
这些需要后续提供的测试设备，不能用静态构建成功代替。
