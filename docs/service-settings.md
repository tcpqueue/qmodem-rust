# 服务与日志设置

```toml
[server]
listen = "0.0.0.0"
port = 8088
interface = "br-lan"

[logging]
level = "info"
format = "text"
```

`listen` 支持 IPv4 或 IPv6。`0.0.0.0` 表示 IPv4 任意地址，`::` 表示 IPv6 任意地址；IPv6 监听不会隐式接收 IPv4。当前是一个监听器，不能同时指定两个地址。

`interface` 是实际 Linux 网络设备名，如 `br-lan`、`eth0`、`eth0.10`，不是 UCI 的逻辑网络段名 `lan`。空字符串表示不限制网卡。绑定通过 `SO_BINDTODEVICE` 实现；绑定失败会停止启动，不降级成所有网卡可访问。接口最多 15 个 ASCII 字符。显式 IP 必须属于指定接口，IPv6 链路本地地址同时要求接口。

非回环监听需要先初始化访问令牌。LuCI 可以生成首个令牌、选择实际网卡、设置地址与端口，然后启动服务。此版本的独立 Web 页面仍在开发，HTTP API 可用。

日志级别从高到低为 `error`、`warn`、`info`、`debug`、`trace`，另有 `off`。选定等级后，只输出该等级及更严重的事件；`off` 关闭运行日志。`text` 适合直接阅读，`json` 适合检索。服务启动失败时 CLI 仍将错误写到标准错误流，便于发现配置问题。

日志分为 HTTP、AT、存储及服务生命周期。HTTP 记录状态、耗时和请求编号，不记录 URL 查询参数、令牌、请求体、短信正文或完整 AT 数据。procd 接收 stdout/stderr 后写到系统日志，可通过 `logread` 查询。

LuCI 保存时调用 Rust CLI，使用文件锁与同目录原子替换，保留其他 TOML 字段及注释；不依赖 HTTP 服务运行，不重复写 UCI。监听与日志设置在服务重启后生效。

```sh
qmodemd --config /etc/qmodem-rust.toml service-info
qmodemd --config /etc/qmodem-rust.toml interfaces
qmodemd --config /etc/qmodem-rust.toml set-service --listen 0.0.0.0 --port 8088 --interface br-lan --log-level debug --log-format json
```

CLI 的 `--interface any` 清除网卡限制。该关键字只存在于 CLI；TOML 使用 `interface = ""`。

SQLite 默认文件为 `/etc/qmodem-rust/data.sqlite3`，避免 OpenWrt 的 `/var` 临时目录在重启后丢失历史。可在 TOML 中改到外置存储的绝对路径；当前仅建立初始结构，短信和统计的写入策略仍在迁移。

## 令牌遗失与新安装菜单

LuCI 位于“服务 → QModem Rust”。安装后原有 LuCI 登录会话可能仍使用安装前的
access-group 权限，导致菜单被隐藏；退出 LuCI 后重新登录，再打开
/cgi-bin/luci/admin/services/qmodem-rust。清理菜单缓存不能替代会话重新认证。

首次部署使用“初始化访问令牌”。遗失时使用“重新生成访问令牌”，确认后生成新的
随机令牌并自动重启 QModem 服务。旧令牌失效，需要重新连接 WebUI，模组配置和短信
历史保留。新令牌在弹窗中显示一次；即使重启失败，弹窗也保留令牌和手动重启提示。
后台只保存哈希，不能再次显示已经关闭的原令牌。

命令行对应 qmodemd --config /etc/qmodem-rust.toml reset-auth。
命令只更新令牌哈希，需重启服务生效。此命令仅通过 LuCI 写权限的文件执行 ACL 开放，
service-info、日志和普通业务 API 不返回令牌。

2026-09-12 实机验证：新登录 LuCI 页面和菜单均返回 200；新会话包含 QModem
read/write access-group，reset-auth 文件执行权限通过，service-info 的 RPC 调用成功。
ARM64 reset-auth 在临时配置上通过验证；实际使用中的令牌没有被替换。

### 控件全部禁用的修复

LuCI 的 E()/dom.attr() 通过 setAttribute 写入非 null 属性，disabled=false 会生成
属性 disabled="false"，浏览器仍然禁用控件。selected=false 同样会把选项标成选中。
权限允许时现在传 null 以省略属性；只有确实禁用或选中时才传 true。
新增渲染回归测试覆盖管理员可操作、只读用户受限、首次生成令牌和默认选中项。
此问题与 ACL、旧会话问题不同，不能通过重新登录修复。
