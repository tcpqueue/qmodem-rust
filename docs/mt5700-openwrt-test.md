# MT5700 在 GL-MT3600BE 上的查询测试

日期：2026-09-12。源代码基线：8baaf12，加本次 AT 通知分流修复。

## 环境

- GL.iNet GL-MT3600BE，aarch64，mediatek/filogic。
- ImmortalWrt 25.12-SNAPSHOT，Linux 6.12.103，apk 包管理器。
- TD Tech MT5700M-CN，固件 V200R001C20B024，USB ID 3466:3301。
- USB 连接用于 AT，用户确认数据端口使用转网口模式。
- 当前 USB configuration 1，四个 option 串口，无模组 USB 网卡。
  描述符另有 configuration 2，含 ECM 接口；测试未切换配置。

WSL Linux 原生目录编译 aarch64 musl 静态二进制，通过 SSH 传输到路由器临时目录。
查询由路由器上的 Rust 原生串口传输与队列执行，不经过 Windows COM 转接。
串口测试前检查了 /proc 中的打开文件，未发现占用。

## 查询结果

| 端口 | 查询 | 结果 |
|---|---|---|
| ttyUSB1 | AT | OK |
| ttyUSB1 | ATI | 型号、厂商及固件匹配，OK |
| ttyUSB1 | AT+CGMM | MT5700M-CN，OK |
| ttyUSB1 | AT+CPIN? | READY，OK |
| ttyUSB1 | AT^CHIPTEMP? | 第六字段 330，对应 33 °C，OK |
| ttyUSB1 | AT^SETMODE? | 2，OK |
| ttyUSB1 | AT^HCSQ? | NR 信号响应，OK |
| ttyUSB0 | AT+CGMM | 2 秒超时，本次不作为可用 AT 端口 |

ttyUSB2、ttyUSB3 未测试，不能据此断言不可用。

## 实测修复

首条 AT 响应混入积压的 ^RSSI、^CERSSI、^HCSQ 及网络注册通知。
传输层现在把已知单行通知发布为 unsolicited 事件，不拼入无关命令的 response。
直接查询相同前缀时仍保留响应；未知厂商行也保留。
修复后再次执行 AT、CPIN?、HCSQ?，返回分别为终止行、SIM 状态和信号内容，均为 OK。

新增两项回归测试覆盖分片交错通知、短信索引通知、注册通知、同前缀查询及未知行保留。
89 项 Rust 测试和严格 Clippy 检查通过，ARM64 静态链接检查通过。
共享同一前缀的查询和异步通知仍可能在协议层存在歧义；本次不是完整 URC 字典实现。

## 服务验证与边界

使用单独的临时配置，不登记模组、关闭自动发现，避免触发初始化和短信后台任务。
服务仅监听本机回环地址，验证健康接口、鉴权设备清单、嵌入网页以及 SQLite 创建。
设备清单读取真实 sysfs；HTTP 服务接收 SIGTERM 后正常退出，退出码为 0。

未安装软件包、替换系统服务、修改 UCI 或 USB 配置。未执行拨号、重启、切卡、
锁频或短信读写。路由器临时文件在测试后清理，串口释放。
尚未验证 LuCI/procd 安装、实际数据通路和短信；也不能用本机测试代替 OpenWrt 24.10 验收。
账号凭据、IMEI、USB 序列号和位置标识不写入测试报告或提交夹具。
