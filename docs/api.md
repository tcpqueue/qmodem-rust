# HTTP API v1

业务 API 使用 `/api/v1`，不继承 ubus 的命名与参数封装。以下是**当前已经实现**的接口；未列出的短信、拨号、统计和完整设置接口尚在迁移中。

## 认证与返回

`qmodemd --config /etc/qmodem-rust.toml init-auth` 生成首个访问令牌，输出一次，并只将 SHA-256 保存到 TOML。已存在令牌时不会覆盖。LuCI 管理员也可通过“初始化访问令牌”创建。

请求使用 `Authorization: Bearer <token>`，不通过 URL 传递令牌。成功响应是 `{"data": ...}`，失败响应是 `{"error":{"code":"...","message":"..."}}`。响应头 `X-Request-Id` 可对应 HTTP 日志；服务重启后序号重置。所有 API 禁止缓存。

`GET /api/health` 是无需认证的进程健康检查，只返回程序版本与开发状态。它不表示模组已连接或完整业务已经迁移。

| 方法 | 路径 | 功能 |
| --- | --- | --- |
| GET | `/api/v1/system/service` | 监听与日志配置，不含令牌及哈希 |
| GET | `/api/v1/system/interfaces` | Linux 网络设备与 IPv4/IPv6 地址 |
| GET | `/api/v1/modems` | 已配置模组，限移远及 TD Tech MT5700 |
| GET | `/api/v1/modems/{id}/capabilities` | 已接入的控制操作及验证状态 |
| POST | `/api/v1/modems/{id}/at` | 原生 AT 请求，不调用 Shell 或原 QModem |
| POST | `/api/v1/modems/{id}/actions` | 结构化厂商操作 |
| GET | `/api/v1/modems/{id}/events` | 鉴权后的 SSE 串口行事件流 |
| GET | `/api/v1/catalog` | 本次适配范围内的上游设备资料 |

## AT 调试

```json
{"command":"AT+CSQ","timeout_ms":10000}
```

超时范围为 1–120000 毫秒，命令最大 4096 字节；允许 AT 大小写，不接受 CR/LF/NUL 等控制字符。端口由模组配置决定，HTTP 请求不能指定任意文件路径。

返回的 `replies` 每项包含 `status`、`terminal`、`modem_success`、`response`。保持上游 transport 语义：收到 `ERROR` 等终止行时 `status=0`，但 `modem_success=false`，不会把模组拒绝误报为成功。

原始 AT 数据只向已认证调用者返回，不写入日志。AT 调试可执行写命令；如用于日常状态查询，应优先调用结构化接口。

## 结构化操作

```json
{"operation":"get_mode"}
```

```json
{"operation":"set_network_prefer","networks":["4G","5G"]}
```

当前共同操作：`get_imei`、`get_mode`、`set_mode`（字段 `mode`）、`get_network_prefer`、`set_network_prefer`（字段 `networks`）、`soft_reboot`。

移远另有 `get_sim_slot`、`get_5g_lan`、`set_5g_lan`（字段 `enabled`）。此操作表仅说明软件已接入，不表示所有固件均具备硬件能力；固件拒绝会返回 `modem_rejected`。TD Tech MT5700 的卡槽状态沿用上游软件记录语义，其迁移未完成，不能使用移远查询命令代替。

结构化读取发生解析失败时返回 `invalid_modem_response`，不会用默认数值掩盖缺失信息。

## 错误约定

| 状态码 | code | 含义 |
| --- | --- | --- |
| 401 | `unauthorized` | 令牌缺失或无效 |
| 404 | `modem_not_found` / `not_found` | 模组或接口不存在 |
| 409 | `modem_disabled` / `at_unsynchronized` | 模组已禁用或端口仍未恢复同步 |
| 422 | `invalid_request` | 参数、操作或配置不适用 |
| 429 | `at_queue_full` | 单端口等待队列已满 |
| 502 | `modem_rejected` / `invalid_modem_response` / `at_response_too_large` | 模组拒绝、响应异常或超限 |
| 503 | `serial_unavailable` | 端口无法打开或已经断开 |
| 504 | `at_timeout` | 当前 AT 操作超时 |

## 串口事件

SSE 的 `serial` 事件包含 `correlation` 和 `line`，关联类型为 `unsolicited`、`response`、`terminal`、`recovery`，以及发生缓冲溢出时的 `overflow`。仅按响应事务归属分类，不声称已经完成所有厂商 URC 的语义解析。

慢消费者错过环形缓冲中的消息时收到 `gap` 事件和丢失条数，必须重新读取业务状态。事件缓冲不替代短信持久化队列。

## 后续接口原则

后台可重新布局，但操作不能无声丢失。分页使用明确的筛选、游标与数量；耗时扫描、拨号和短信同步将使用独立任务状态。未完成的接口不会返回模拟成功。
