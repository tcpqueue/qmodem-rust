# 迁移缺口核对

核对日期：2026-09-12。Rust 基线：`1191e0a`；QModem 基线：`86102c2a6f620264b6308bff29a92ecde6a2ea2f`。范围为移远系列、TD Tech MT5700 与 luci-app-qmodem-next 对应功能。

当前已经完成原生 AT 传输、事务队列、队列监控页面、服务基础设置和一批厂商操作，**距离完整替代原项目还有较多业务工作**。源码或型号资料已保存、通用串口底层已具备、SQLite 已建表，都不能作为对应业务已迁移的依据。

## 入口核对

本次不仅核对原来的主 RPC 清单，还检查了短信服务、AT daemon、前端菜单与表单、初始化和拨号脚本。逐项记录见 [migration-inventory.json](migration-inventory.json)，每项包含源文件位置、当前替代路径及缺失内容。

| 上游入口组 | 入口数 | 已实现 | 已由新架构替代 | 部分迁移 | 待迁移 |
| --- | ---: | ---: | ---: | ---: | ---: |
| qmodem | 56 | 13 | 1 | 5 | 37 |
| qmodem.sms | 17 | 0 | 0 | 0 | 17 |
| at-daemon | 10 | 0 | 6 | 3 | 1 |

这是接口核对数量，不是项目完成率，也不代表有 83 项独立用户功能。接口之间存在重叠，工作量差异很大。进程内事务替代 lease_acquire 等底层入口，并不意味着依赖这些入口的短信、初始化或重拨已经完成。主接口中的版权查询由 `/licenses` 替代；前端页面覆盖另行计算。

## 需要纠正的完成状态

1. **SIM 切换只有 AT 层完成，切卡后的自动重拨未完成。** 上游主 RPC 在厂商切换成功后调用 `qmodem_network redial`，两步均成功才返回 success。目前 `/actions` 的 set_sim_slot 只确认厂商切卡，不恢复网络连接。必须补上拨号状态机，并在新 API 中清楚区分卡槽结果与网络恢复结果。
2. **MT5700 软件卡槽的离线异常顺序仍有差别。** 上游先写软件状态，再尝试发送 AT。当前 worker 中也先写状态，但 HTTP 会先打开端口；端口打不开或 worker 已隔离时，不会执行写状态。正常切换、模组 ERROR 和服务重启后的记录已有测试，离线语义还不能称为完全一致。
3. **队列可视化不等于完整模组后台。** 当前页面仅有队列监控及访问令牌登录。概览、拨号、SIM 操作、锁频、小区、短信、模组配置、AT 控制台等操作页面都还没有接入。
4. **型号目录不等于设备发现。** 当前模组需要手工写入 TOML，没有自动扫描、端口探测、插拔处理及设备配置生成。
5. **事件流不等于短信 URC 业务。** 已有串口行/SSE 和断档提示，但没有按型号设置短信通知、识别新短信、断档补同步和可靠入库。
6. **断开或隔离的端口不会自动重建。** PortPool 缓存已创建的 Port；缺少显式关闭、重开和热插拔恢复流程，目前需要重启服务。

切卡外层行为依据：[qmodem RPC set_sim_slot](https://github.com/FUjr/QModem/blob/86102c2a6f620264b6308bff29a92ecde6a2ea2f/application/qmodem/files/usr/libexec/rpcd/qmodem#L1047)。MT5700 软件记录依据：[huawei.sh set_sim_slot](https://github.com/FUjr/QModem/blob/86102c2a6f620264b6308bff29a92ecde6a2ea2f/application/qmodem/files/usr/share/qmodem/vendor/huawei.sh#L846)。

## 未迁移业务与容易遗漏的细项

| 业务 | 尚缺内容 | 上游依据 |
| --- | --- | --- |
| 模组概览 | 型号/固件、驱动、温度、电压、SIM 状态、IMSI/ICCID、运营商、注册状态、信号、网络速率和连接状态；原查询缓存 | vendor/quectel.sh、vendor/huawei.sh、generic.sh、modem_ctrl.sh |
| 小区与当前频段 | 服务小区、邻区扫描、载波聚合、PCC/SCC、SA/NSA、带宽和 SCS；锁 PCI/ARFCN、解锁、平台差异及锁定状态读取 | quectel.sh 的 cell_info、get_current_band、get_neighborcell、set_neighborcell |
| 拨号 | QMI/MBIM/ECM/NCM/MHI 对应路径、连接/断开/重拨、自动拨号、失败重试、CFUN 恢复、拨号日志、IP 变更处理 | modem_dial.sh、qmodem_network |
| 网络配置 | PDP 类型、双 APN、APN 自动选择/强制设置、PAP/CHAP 等认证、PIN 解锁、路由 metric、自定义 DNS、DNS 写入策略、IPv6 RA/前缀扩展 | next/network_config.js、modem_dial.sh |
| 桥接与网口 | 桥接直通、桥端口迁移及恢复、管理接口、防火墙区域、移远不启用 NAT、5G Ethernet 关联和不同平台处理 | modem_dial.sh、next/settings.js、next/network_config.js |
| 设备发现 | USB/PCIe 枚举、支持型号匹配、AT/SMS/数据端口识别、预设端口规则、探测超时/并发、插入延迟与重试、自动移除及重连 | modem_scan、hotplug.d、modem_port_rule.json、next/settings.js |
| 模组与槽位设置 | 增删改设备、端口覆盖、支持模式覆盖、全局/设备禁用功能、USB/PCIe 槽位关联、别名及默认路由配置 | next/settings.js、generic.sh |
| 短信编解码与收发 | GSM 7-bit/UCS2、长短信分段/合并、PDU 收发、发送结果、SIM 消息删除和存储容量；当前只有传输提示符测试 | libqmodem-sms、qmodem_smsd、generic.sh |
| 短信数据库业务 | 会话列表/详情、分页、已读、SIM 源索引、幂等导入、落库后安全删除、旧历史导入、状态与同步进度、发送记录 | qmodem_smsd/src/main.c、sms_db.c、legacy_migrate.c |
| 短信模式与初始化 | 直接读取 SIM、数据库轮询、数据库 URC 模式；CPMS 存储设置、固件匹配、通知注册、重新初始化后补同步 | qmodem-settings、qmodem_smsd、next/sms.js、sms_sim.js |
| 短信转发 | Telegram、Webhook、ServerChan、PushDeer、飞书、自定义扩展；配置、轮询、投递领取/完成、失败重试及去重 | next/sms_forward.js、sms_forwarder、qmodem.sms.delivery_* |
| 流量 | 模组累计计数、RX/TX 平台次序、读取/清零/保存、持久化调度、每日/每周/每月清零配置和页面 | quectel.sh get_usage_stats/write_usage_stats/clear_usage_stats、usage_stats.sh、主 RPC |
| 自动恢复 | 按 IP/网关/DNS ping 或 HTTP 检查、连续失败阈值、初始化宽限、冷却、自动切卡及恢复动作 | qmodem_monitor、next/sim_switch.js |
| 开机和拨号钩子 | post_init/pre_dial 的延迟与 AT 列表、开机恢复锁小区、初始化 ready/degraded 状态、拔插后重新应用 | modem_hook.sh、modem_util.sh、qmodem-settings |
| GPIO 与关机 | GPIO 断电/上电、槽位/设备配置、缺少 GPIO 时软重启回退、硬重启能力、关机阶段按配置软重启及重试 | generic.sh、qmodem_reboot |
| LED 联动 | LED 枚举、SIM/联网指示灯、槽位映射及设备预设脚本行为 | qmodem_led_watch.sh、led_scripts、hotplug.d/iface |
| 调试与辅助 | 已发现/有效端口切换、按语言/厂商平台分类的快捷 AT 指令、调试控制台、拨号日志读取/清理及旧状态页入口 | next/debug.js、get_at_cfg、next/status/include/11_modem.js |
| 构建与部署 | OpenWrt SDK 包构建、ARM64/x86 产物、真实 USB/PCIe 模组、LuCI ACL 与 procd 联调 | packaging/openwrt、测试环境记录 |

不是所有厂商都支持表内每个动作。MT5700 的 LockBand/NeighborCell 在上游禁用，当前频段与流量查询也要保留其通用“不支持/不可用”语义；不应为凑齐界面而发送移远命令。其他厂商、独立 VoIP 产品不因共享代码文件自动进入当前适配范围。

自定义脚本入口属于用户扩展能力，需要在新接口中安排替代或显式扩展边界；不能把原来的第一方业务脚本保留下来充当 Rust 实现。系统驱动、netifd、ubus 和必要网络工具仍可复用。

## 建议后续顺序

1. 设备发现、端口恢复和初始化状态机，使设备可以自行识别并在拔插后恢复。
2. 概览与状态读取，同时建立查询缓存，减少重复 AT 轮询。
3. 拨号与网络配置，接通切卡后的重拨，完成基本联网闭环。
4. 锁小区及开机恢复，验证移远各平台命令和异常路径。
5. 短信数据库/收发/同步，再接转发及可靠投递。
6. 流量、自动恢复、GPIO/LED 和其余后台页面，随后完成跨架构与 OpenWrt 实机验收。

本次是覆盖核对与文档修正，没有新增业务实现；现有 52 项测试不能用来证明上表待迁移功能已具备。
