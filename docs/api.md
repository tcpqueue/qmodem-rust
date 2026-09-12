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
| GET | `/api/v1/queues` | 按模组与端口读取真实队列元数据 |
| GET | `/api/v1/catalog` | 本次适配范围内的上游设备资料 |

## AT 调试

```json
{"command":"AT+CSQ","timeout_ms":10000}
```

超时范围为 1–120000 毫秒，命令最大 4096 字节；允许 AT 大小写，不接受 CR/LF/NUL 等控制字符。端口由模组配置决定，HTTP 请求不能指定任意文件路径。

返回的 `replies` 每项包含 `status`、`terminal`、`modem_success`、`response`。保持上游 transport 语义：收到 `ERROR` 等终止行时 `status=0`，但 `modem_success=false`，不会把模组拒绝误报为成功。

已知单行主动上报通过 SSE 的 unsolicited 事件返回，不混入无关命令的 response。查询相同前缀时保留响应；未知厂商行也保留。同前缀的查询回复与主动上报可能存在协议歧义。

原始 AT 数据只向已认证调用者返回，不写入日志。AT 调试可执行写命令；如用于日常状态查询，应优先调用结构化接口。

## 结构化操作

```json
{"operation":"get_mode"}
```

```json
{"operation":"set_network_prefer","networks":["4G","5G"]}
```

当前共同操作：`get_imei`、`get_mode`、`set_mode`（字段 `mode`）、`get_network_prefer`、`set_network_prefer`（字段 `networks`）、`soft_reboot`。

移远另有 `get_5g_lan`、`set_5g_lan`（字段 `enabled`）。此操作表仅说明软件已接入，不表示所有固件均具备硬件能力；固件拒绝会返回 `modem_rejected`。TD Tech MT5700 的卡槽状态沿用上游软件记录语义，详见下文。

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

## 频段与 SIM 操作

共同增加 `get_sim_slot`、`get_sim_capabilities`、`set_sim_slot`、`set_imei`。移远增加 `get_band_lock`、`set_band_lock`。

```json
{"operation":"set_sim_slot","slot":2}
```

**当前 set_sim_slot 只执行厂商切卡，不自动重拨。** 上游主 RPC 还要求切卡后重拨成功；这部分尚未迁移，当前响应成功不能解释为网络已恢复。MT5700 端口无法打开时，当前不会预先写软件卡槽记录，与上游无条件先写记录仍有差别。

移远允许槽位 1/2。设置返回 OK 后立即回读，失败时每次等待一秒，最多五次回读（第五次失败后仍等待一秒）。整个过程不释放端口队列；HTTP 调用者断开也会完成已开始的事务。未确认切换时返回 HTTP 502 `sim_switch_unconfirmed`，`error.details.data` 包含请求槽位、最后读到的槽位和回读次数。无可解析槽位时为 null。

MT5700 允许槽位 0/1。卡槽读取和能力信息不发送 AT，返回 `source=software`、`hardware_verified=false`；切换发送 `AT^SCICHG=0,1` 或 `AT^SCICHG=1,0`。兼容上游在 AT 前记录请求值的顺序，即使模组拒绝也保留请求值。软件记录目录由 `storage.runtime_dir` 指定，默认 `/tmp/qmodem-rust`，必须位于易失存储，已有目录须为服务用户所有的私有目录。父目录必须已存在。

```json
{"operation":"get_band_lock"}
```

```json
{"operation":"set_band_lock","band_class":"NR","bands":[41,78,79]}
```

类别为 `UMTS`、`LTE`、`NR_NSA`、`NR`。LTE 平台使用任意宽度十六进制位掩码，其他平台使用冒号列表，保留上游平台分支、列表顺序与重复项行为。数组最多 1024 项，频段范围 1–1024；空数组对应上游空列表（LTE 掩码为 0），不会自动改成“全部支持频段”。固件是否支持某个频段仍由模组判定。

查询结果 `data.data.bands` 按类别包含 `locked_bands`、`available_bands`、`available_source`、`state`。可选频段优先使用 `[modems.bands]` 的 `umts/lte/nr_nsa/nr` 数组，其次为型号资料和平台默认值。部分查询失败时 `partial=true`，失败类别为 `state=unknown`、`locked_bands=null`，保留错误信息及原始 `replies`；所有可见类别失败时返回 HTTP 502。不会将错误显示为空的锁定列表。

`set_imei` 使用 `imei` 字符串，必须恰好 15 个 ASCII 数字。移远在写入失败后仍回读，返回写入与读取两份结果；回读不一致返回 `imei_unconfirmed`，回读无法解析返回 `invalid_modem_response`。MT5700 只发送上游写入命令，并标记未硬件回读。

多步业务失败的 `error.details` 保留本次事务结果及响应，只有已认证调用者可读；这些内容不写入服务日志。`runtime_state_failed`（HTTP 500）表示软件状态存取或内部事务执行失败。

## 队列监控

`GET /api/v1/queues` 需要 Bearer 认证。按配置中的模组返回 `modems[].ports[]`，包含配置路径、AT/SMS 用途、规范化设备路径、是否已打开以及 `queue`。读取此接口不会打开串口或发送 AT；未打开端口的 `queue=null`。

每个实际串口具有一条队列，容量为 **32 个等待事务，加 1 个正在执行的事务**。不同模组或 AT/SMS 配置指向同一实际串口时共享调度器；返回中允许多个配置引用同一快照，页面统计按实际路径去重。

`queue` 包含：

- `state`：`idle`、`running`、`waiting`（事务内延迟）、`recovering`、`quarantined`、`closed`。
- `current`、`waiting`、`waiting_count`：当前事务、等待列表及数量。
- `recent`：最近 32 个结束事务，按结束时间倒序。
- `completed`、`failed`、`cancelled_before_start`、`rejected_queue_full`：本次服务运行的累计计数。

事务只包含 ID、所属模组、操作名、排队毫秒数、执行毫秒数、已发送命令数及结束原因，不包含 AT 参数、IMEI、短信内容或原始响应。历史中的 `caller_detached=true` 表示请求方已经离开；进行中该字段为 null。取消尚未开始的请求会跳过该事务，取消已开始的请求不会打断串口操作。

这些计数表示 AT 事务的传输结果；例如收到一组 OK 但业务验证未通过，传输仍可记为完成，具体业务是否成功以 action 响应为准。CLI 的独立 `at` 进程不属于 HTTP 服务队列，串口独占会阻止它与已打开设备同时读写。

内嵌页面位于 `/` 和 `/queues`。默认每秒读取一次队列，支持三秒或手动刷新；标签页隐藏时暂停，恢复可见后重新读取。网络错误时标明数据未更新及最后成功时间。

## Additional migration endpoints

All endpoints below require the same bearer token.

- `GET /api/v1/discovery`: sysfs inventory without AT transmission.
- `POST /api/v1/discovery/{id}/probe`: bounded native identification.
- `POST /api/v1/discovery/{id}/bind`: apply the matching upstream option-driver rule.
- `GET|PUT|DELETE /api/v1/modems/{id}/config`: complete modem TOML entry; writes apply live.
- `POST /api/v1/modems/{id}/ports/close`: `{ "role": "at" | "sms" }`; busy ports return 409.
- `GET /api/v1/modems/{id}/status`: coalesced status transaction, cache lifetime 3 seconds.
- `POST /api/v1/modems/{id}/network`: operation `plan`, `status`, `connect`, `disconnect`, `redial`.
  Mutation requires OpenWrt. `connecting` is not proof that an IP address was acquired.
- New vendor actions: `get_neighborcell`, `set_cell_lock` (`lock` object with `rat`,
  `arfcn`, `pci`, optional `scs` index and `band`), `unlock_cell`, `get_usage_stats`,
  `write_usage_stats`, `clear_usage_stats`.
- SIM switch workflow now returns `sim_redial_failed` if switching succeeds but redial
  fails. Error details retain `sim_switched: true`; callers must not blindly retry.
- `GET /api/v1/modems/{id}/sms`: optional `peer`, `before` cursor and `limit` (1–200).
- `GET /api/v1/modems/{id}/sms/conversations`: conversation/unread summaries.
- `GET|PATCH|DELETE /api/v1/modems/{id}/sms/{message}`: history detail, `{ "is_read": true }`, deletion.
- `POST /api/v1/modems/{id}/sms/sync`: `{ "memory": "SM" }` imports without deleting SIM data.
- `POST /api/v1/modems/{id}/sms/send`: `request_id`, `peer`, `content`. Reuse the same
  request ID after uncertain HTTP delivery. `submitted` means modem accepted the SMS;
  it is not a recipient delivery confirmation. Transport uncertainty is stored as `unknown`.
- `GET|DELETE /api/v1/modems/{id}/sms/sim`: list with `memory`; deletion requires
  `index`, `memory`, `expected_pdu`, checked with CMGR immediately before CMGD.
- `GET|PUT /api/v1/modems/{id}/sms/storage`: query/set modem memory.
- `GET|PUT /api/v1/modems/{id}/sms/config`: mode `manual|poll|urc|sim_only`,
  `poll_interval_seconds`, three `memories` entries. URC mode requires the original
  model and firmware rule; mismatch remains degraded, without inventing CNMI commands.

## 初始化、维护和调试

| 方法 | 路径（省略 /api/v1/modems/{id}） | 用途 |
| --- | --- | --- |
| GET/PUT | /startup | 初始化延迟、AT 列表、锁小区恢复、GPIO/LED、关机重启 |
| PUT | /cell-lock | lock 为锁定参数或 null；persist 指定是否保存开机恢复 |
| POST | /reboot | GPIO 复位；未配置 GPIO 时软重启 |
| GET | /hardware | LED 列表和重启能力 |
| GET/PUT | /maintenance | monitor、traffic：连通性检查、恢复动作、流量采样与清零 |
| GET | /traffic/history | 最近 1000 条计数，带 source；来源变更不可直接计算速率 |
| GET | /debug/config?lang=zh | 厂商/平台快捷 AT 与主 AT/SMS 端口 |
| GET/DELETE | /logs | 此模组当前进程日志缓存；删除不清除 OpenWrt 系统日志 |
| POST | /sms/send-pdu | request_id、pdu：包含 SMSC 字段的 SMS-SUBMIT 十六进制 |
| POST | /sms/import | source、document：原项目 sent/received JSON，重复导入跳过 |
| GET | /sms/deliveries | 持久转发记录和重试状态 |
| POST | /sms/deliveries/{delivery}/retry | 重试已失败任务 |

原始 AT 请求可带 port: primary 或 sms，默认为主 AT；不能指定任意系统路径。
短信转发配置位于 sms.forwarding，由 /sms/config 或模组配置保存。
目标改动或停用时取消尚未完成的旧任务。支持的转发类型为
telegram、webhook、serverchan、pushdeer、feishu、custom；令牌字段仅通过已认证配置接口访问。
后台网络请求不自动跟随重定向。HTTP 接收端是否处理 Idempotency-Key 去重取决于接收方。

流量清零配置位于 traffic.reset：enabled、kind（daily/weekly/monthly）、hour、day。
每周 day=0 为周日，每月使用 1–31，短月不存在的日期跳过。
在配置小时内补执行一次，通过 SQLite 记录避免服务重启后重复清零。

network.firewall_zone 默认 wan；空字符串表示不加入区域，必须使用已存在的区域。
/network 的状态返回来自 netifd，不能把“拨号命令成功”当作已获取 IP。
