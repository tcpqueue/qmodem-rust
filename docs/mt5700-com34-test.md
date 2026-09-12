# MT5700 COM34 查询测试

设备：TD Tech MT5700M-CN，固件 V200R001C20B024。
主机端口：Windows COM34，115200、8N1、无流控。
日期：2026-09-12。

Windows 仅提供本地串口字节转接，WSL 中的 qmodemd 使用原生 Rust AT 队列、
收发和终止符识别执行查询。没有运行 OpenWrt 网络服务或自动初始化。

| 查询 | 结果 |
|---|---|
| AT | OK |
| ATI | 型号、固件信息正常，OK |
| AT+CGMM | MT5700M-CN，OK |
| AT+CPIN? | READY，OK |
| AT^CHIPTEMP? | 第六字段 320，对应 32 °C，OK |
| AT^MONSC | NR，RSRP -94、RSRQ -10、SINR 30，OK |

六条查询完成后关闭转接并释放 COM34。未执行拨号、断网、重启、SIM 切换、
锁频、锁小区、短信读写等操作。IMEI 和原始位置标识未写入本报告或提交的测试夹具。

实测发现 MT5700 温度解析缺少 /10 换算，已按上游 hisilicon 分支修正，
并使用固件、温度、SIM 和信号响应补充回归测试。回归夹具替换小区及位置标识。

本次证明该设备能够响应 Rust AT 查询通路；不代表完成 OpenWrt 驱动、
USB/PCIe 枚举、网络拨号、短信或故障恢复的实机验证。
