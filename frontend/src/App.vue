<script setup lang="ts">
import { computed, onBeforeUnmount, onMounted, ref } from "vue";
import {
  ElButton,
  ElInput,
  ElSelect,
  ElOption,
  ElTag,
  ElDialog,
  ElTable,
  ElTableColumn,
  ElAlert,
  ElEmpty,
  ElProgress,
} from "element-plus";
import {
  Connection,
  Cpu,
  Refresh,
  Timer,
  CircleCheck,
  ArrowRight,
  Lock,
  SwitchButton,
  List,
  Search,
} from "@element-plus/icons-vue";
import type { Snapshot, Port, Modem } from "./types";
import Devices from "./Devices.vue";
import { savedToken, rememberToken } from "./browser-utils";
const page = ref("queues");
const pageTitle = computed(
  () =>
    ({ queues: "AT 队列", devices: "模组管理", discovery: "设备发现" })[
      page.value
    ],
);
const inputToken = ref(""),
  token = ref(""),
  snapshot = ref<Snapshot | null>(null),
  error = ref(""),
  busy = ref(false);
const connected = ref(false),
  updated = ref<Date | null>(null),
  interval = ref(1000),
  filter = ref("");
const detail = ref<{ modem: string; path: string } | null>(null);
let timer: ReturnType<typeof setTimeout> | undefined,
  controller: AbortController | undefined;
const operations: Record<string, string> = {
  discovery: "识别设备",
  discovery_model: "识别型号",
  post_init: "初始化模组",
  status: "读取运行状态",
  network_connect: "连接网络",
  network_modem_status: "读取模组联网配置",
  network_disconnect: "断开网络",
  at: "AT 事务",
  raw_at: "AT 调试",
  get_imei: "读取 IMEI",
  set_imei: "设置 IMEI",
  get_mode: "读取工作模式",
  set_mode: "设置工作模式",
  get_network_prefer: "读取网络偏好",
  set_network_prefer: "设置网络偏好",
  get_5g_lan: "读取 5G LAN",
  set_5g_lan: "设置 5G LAN",
  get_sim_slot: "读取 SIM 卡槽",
  get_sim_capabilities: "查询切卡能力",
  set_sim_slot: "切换 SIM 卡槽",
  get_band_lock: "读取锁定频段",
  set_band_lock: "设置锁定频段",
  soft_reboot: "重启模组",
};
const states: Record<string, string> = {
  idle: "空闲",
  running: "执行中",
  waiting: "等待重试",
  recovering: "超时恢复",
  quarantined: "端口已隔离",
  closed: "端口已断开",
};
const outcomes: Record<string, string> = {
  completed: "事务完成",
  completed_with_modem_error: "包含模组拒绝",
  cancelled_before_start: "启动前取消",
  timeout: "超时",
  overflow: "响应超限",
  transport_error: "传输失败",
  program_error: "状态处理失败",
  unsynchronized: "未恢复同步",
  port_closed: "端口已关闭",
};
const duration = (ms: number | null) =>
  ms === null ? "—" : ms < 1000 ? `${ms} ms` : `${(ms / 1000).toFixed(1)} s`;
const name = (operation: string) => operations[operation] ?? operation;
const modems = computed(
  () =>
    snapshot.value?.modems.filter((m) =>
      `${m.name} ${m.id} ${m.model}`
        .toLowerCase()
        .includes(filter.value.toLowerCase()),
    ) ?? [],
);
const queues = computed(() => {
  const unique = new Map<string, Port>();
  for (const m of snapshot.value?.modems ?? [])
    for (const p of m.ports) unique.set(p.canonical_path ?? p.path, p);
  return [...unique.values()];
});
const running = computed(
  () => queues.value.filter((p) => p.queue?.current).length,
);
const pending = computed(() =>
  queues.value.reduce((sum, p) => sum + (p.queue?.waiting_count ?? 0), 0),
);
const completed = computed(() =>
  queues.value.reduce((sum, p) => sum + (p.queue?.completed ?? 0), 0),
);
const failed = computed(() =>
  queues.value.reduce((sum, p) => sum + (p.queue?.failed ?? 0), 0),
);
const selected = computed(() =>
  snapshot.value?.modems
    .find((m) => m.id === detail.value?.modem)
    ?.ports.find((p) => p.path === detail.value?.path),
);
const dialogOpen = computed({
  get: () => detail.value !== null,
  set: (open: boolean) => {
    if (!open) detail.value = null;
  },
});
const shared = (port: Port) =>
  snapshot.value?.modems.reduce(
    (sum, m) =>
      sum +
      m.ports.filter(
        (p) =>
          (p.canonical_path ?? p.path) === (port.canonical_path ?? port.path),
      ).length,
    0,
  ) ?? 0;
const tagType = (p: Port) =>
  !p.opened
    ? "info"
    : ["closed", "quarantined"].includes(p.queue?.state ?? "")
      ? "danger"
      : ["waiting", "recovering"].includes(p.queue?.state ?? "")
        ? "warning"
        : p.queue?.state === "running"
          ? "primary"
          : "success";
function schedule() {
  clearTimeout(timer);
  if (token.value && interval.value && !document.hidden)
    timer = setTimeout(refresh, interval.value);
}
async function refresh() {
  if (busy.value || !token.value) return;
  clearTimeout(timer);
  busy.value = true;
  controller = new AbortController();
  const active = controller,
    timeout = setTimeout(() => active.abort(), 8000);
  try {
    const response = await fetch("/api/v1/queues", {
      headers: { Authorization: `Bearer ${token.value}` },
      signal: active.signal,
      cache: "no-store",
    });
    if (active.signal.aborted) return;
    if (response.status === 401) {
      rememberToken("");
      token.value = "";
      connected.value = false;
      snapshot.value = null;
      updated.value = null;
      throw new Error("访问令牌无效，请重新输入。");
    }
    if (!response.ok) throw new Error(`读取失败（HTTP ${response.status}）`);
    const body = await response.json();
    if (active.signal.aborted) return;
    snapshot.value = body.data;
    connected.value = true;
    rememberToken(token.value);
    error.value = "";
    updated.value = new Date();
  } catch (e) {
    if (token.value)
      error.value =
        e instanceof Error && e.name === "AbortError"
          ? "连接超时，显示的是上次读取结果。"
          : e instanceof Error
            ? e.message
            : "无法连接服务。";
    else if (e instanceof Error && e.name !== "AbortError")
      error.value = e.message;
  } finally {
    clearTimeout(timeout);
    busy.value = false;
    schedule();
  }
}
function login() {
  if (!inputToken.value.trim()) return;
  token.value = inputToken.value.trim();
  inputToken.value = "";
  refresh();
}
function logout() {
  rememberToken("");
  clearTimeout(timer);
  controller?.abort();
  token.value = "";
  connected.value = false;
  snapshot.value = null;
  updated.value = null;
  error.value = "";
  detail.value = null;
}
function visibility() {
  if (document.hidden) clearTimeout(timer);
  else if (token.value) refresh();
}
onMounted(() => {
  token.value = savedToken();
  if (token.value) refresh();
});
document.addEventListener("visibilitychange", visibility);
onBeforeUnmount(() => {
  clearTimeout(timer);
  controller?.abort();
  document.removeEventListener("visibilitychange", visibility);
});
function inspect(modem: Modem, port: Port) {
  detail.value = { modem: modem.id, path: port.path };
}
</script>
<template>
  <div class="shell">
    <aside class="sidebar">
      <a class="brand" href="/" aria-label="QModem 首页"
        ><span class="brand-icon"><Connection /></span
        ><strong>QModem<span>模组管理</span></strong></a
      >
      <div class="nav-caption">运行监控</div>
      <button
        v-for="[key, label] in [
          ['devices', '模组管理'],
          ['discovery', '设备发现'],
          ['queues', 'AT 队列'],
        ]"
        :key="key"
        class="nav-item"
        :class="{ 'nav-active': page === key }"
        @click="page = key"
      >
        <List /><span>{{ label }}</span
        ><ArrowRight />
      </button>
      <div class="sidebar-footer">
        <span class="service-dot"></span>Rust 原生服务<small
          >开发版本 · 功能迁移中</small
        >
      </div>
    </aside>
    <div class="workspace">
      <header class="topbar">
        <div>
          运行监控 <span>/</span> <strong>{{ pageTitle }}</strong>
        </div>
        <div class="topbar-right">
          <span class="live" :class="{ stale: !!error }">{{
            error && connected
              ? "数据未更新"
              : connected
                ? "服务已连接"
                : "等待连接"
          }}</span
          ><ElButton v-if="connected" text :icon="SwitchButton" @click="logout"
            >退出</ElButton
          >
        </div>
      </header>
      <main>
        <div class="page-heading">
          <div>
            <div class="eyebrow">MODEM OPERATIONS</div>
            <h1>{{ pageTitle }}</h1>
            <p>
              {{
                page === "queues"
                  ? "查看每个模组、每个端口的任务执行情况。"
                  : page === "discovery"
                    ? "识别设备与端口，建立模组配置。"
                    : "查看连接状态、信号与流量，调整模组设置。"
              }}
            </p>
          </div>
          <div class="heading-note">
            <Connection /><span
              >同端口串行<br /><strong>不同端口并行</strong></span
            >
          </div>
        </div>
        <section v-if="!connected" class="login-card">
          <div class="lock-icon"><Lock /></div>
          <h2>连接模组管理服务</h2>
          <p>使用 LuCI 中生成的访问令牌管理模组。</p>
          <form @submit.prevent="login">
            <label for="token">访问令牌</label
            ><ElInput
              id="token"
              v-model="inputToken"
              type="password"
              show-password
              placeholder="输入访问令牌"
              autocomplete="off"
              :disabled="busy"
            /><ElButton
              native-type="submit"
              type="primary"
              :loading="busy"
              :disabled="!inputToken.trim()"
              >连接服务</ElButton
            >
          </form>
          <ElAlert
            v-if="error"
            :title="error"
            type="error"
            :closable="false"
          /><small>当前标签页会记住登录状态，刷新无需重输；退出登录或令牌失效后清除。</small>
        </section>
        <Devices
          v-else-if="page !== 'queues'"
          :token="token"
          :page="page"
          @changed="refresh"
        />
        <template v-else>
          <ElAlert
            v-if="error"
            class="error-banner"
            type="error"
            :title="error"
            description="下方是上次成功读取的数据，请以更新时间为准。"
            :closable="false"
            show-icon
          />
          <div class="stats">
            <section class="stat">
              <span class="stat-icon blue"><Connection /></span>
              <div>
                <p>已打开端口</p>
                <strong
                  >{{ queues.filter((p) => p.opened).length
                  }}<small>/ {{ queues.length }}</small></strong
                >
              </div>
            </section>
            <section class="stat">
              <span class="stat-icon violet"><Cpu /></span>
              <div>
                <p>执行中</p>
                <strong>{{ running }}<small>个事务</small></strong>
              </div>
            </section>
            <section class="stat">
              <span class="stat-icon amber"><Timer /></span>
              <div>
                <p>排队等待</p>
                <strong>{{ pending }}<small>个事务</small></strong>
              </div>
            </section>
            <section class="stat">
              <span class="stat-icon green"><CircleCheck /></span>
              <div>
                <p>事务完成</p>
                <strong
                  >{{ completed }}<small>{{ failed }} 个含异常</small></strong
                >
              </div>
            </section>
          </div>
          <section class="content-card">
            <div class="toolbar">
              <div>
                <h2>
                  模组与端口
                  <span class="count">{{ snapshot?.modems.length ?? 0 }}</span>
                </h2>
                <p>端口别名共享实际设备的队列，统计自动去重。</p>
              </div>
              <div class="tools">
                <ElInput
                  v-model="filter"
                  placeholder="搜索模组"
                  :prefix-icon="Search"
                  clearable
                  aria-label="搜索模组"
                /><ElSelect
                  v-model="interval"
                  aria-label="刷新频率"
                  @change="schedule"
                  ><ElOption label="每秒刷新" :value="1000" /><ElOption
                    label="每 3 秒刷新"
                    :value="3000" /><ElOption
                    label="手动刷新"
                    :value="0" /></ElSelect
                ><ElButton
                  :icon="Refresh"
                  :loading="busy"
                  aria-label="立即刷新"
                  @click="refresh"
                />
              </div>
            </div>
            <ElEmpty
              v-if="!modems.length"
              :description="filter ? '没有匹配的模组' : '当前配置中没有模组'"
            />
            <article v-for="modem in modems" :key="modem.id" class="modem">
              <div class="modem-heading">
                <span class="modem-icon"><Cpu /></span>
                <div>
                  <h3>
                    {{ modem.name }}
                    <ElTag v-if="!modem.enabled" type="info" size="small"
                      >已禁用</ElTag
                    >
                  </h3>
                  <p>
                    {{ modem.manufacturer === "quectel" ? "移远" : "TD Tech" }}
                    · {{ modem.model || "未指定型号" }}
                    <span class="dot-separator">·</span> {{ modem.id }}
                  </p>
                </div>
                <ElTag class="bus" type="info" effect="plain">{{
                  modem.bus.toUpperCase()
                }}</ElTag>
              </div>
              <div
                v-for="port in modem.ports"
                :key="port.path"
                class="port-row"
              >
                <div class="port-name">
                  <div>
                    <span class="port-role">{{
                      port.roles.join(" / ").toUpperCase()
                    }}</span
                    ><code>{{ port.path }}</code>
                  </div>
                  <small
                    v-if="
                      port.canonical_path && port.canonical_path !== port.path
                    "
                    >实际设备 {{ port.canonical_path }}</small
                  ><small v-if="shared(port) > 1"
                    >与 {{ shared(port) - 1 }} 个端口配置共享队列</small
                  >
                </div>
                <div class="port-state">
                  <ElTag :type="tagType(port)" effect="light"
                    ><span class="status-dot"></span
                    >{{
                      port.opened
                        ? (states[port.queue?.state ?? ""] ?? port.queue?.state)
                        : "尚未打开"
                    }}</ElTag
                  ><small>{{
                    port.queue?.current
                      ? name(port.queue.current.operation)
                      : port.opened
                        ? "暂无执行任务"
                        : "首次使用时打开串口"
                  }}</small>
                </div>
                <div class="port-current">
                  <span class="field-label">当前耗时</span
                  ><strong>{{
                    duration(port.queue?.current?.elapsed_ms ?? null)
                  }}</strong
                  ><small v-if="port.queue?.current"
                    >已发
                    {{ port.queue.current.commands_started }} 条命令</small
                  >
                </div>
                <div class="queue-meter">
                  <div>
                    <span>等待队列</span
                    ><strong
                      >{{ port.queue?.waiting_count ?? 0
                      }}<small>
                        / {{ port.queue?.capacity ?? 32 }}</small
                      ></strong
                    >
                  </div>
                  <ElProgress
                    :percentage="
                      ((port.queue?.waiting_count ?? 0) /
                        (port.queue?.capacity ?? 32)) *
                      100
                    "
                    :show-text="false"
                    :stroke-width="5"
                  />
                </div>
                <ElButton
                  class="detail-button"
                  text
                  type="primary"
                  :disabled="!port.opened"
                  @click="inspect(modem, port)"
                  >详情<ArrowRight
                /></ElButton>
              </div>
            </article>
            <footer class="table-footer">
              <span
                >{{
                  error
                    ? "更新异常"
                    : interval
                      ? "自动刷新中"
                      : "已暂停自动刷新"
                }}<span class="dot-separator">·</span>最近更新
                {{
                  updated?.toLocaleTimeString("zh-CN", { hour12: false }) ?? "—"
                }}</span
              ><span>每端口保留最近 32 个事务</span>
            </footer>
          </section>
          <p class="footnote">
            耗时包含模组响应与事务内等待；完成数表示 AT
            事务执行结果，业务结果以对应操作返回为准。
          </p>
        </template>
      </main>
    </div>
    <ElDialog
      v-model="dialogOpen"
      :title="`端口队列 · ${selected?.path ?? ''}`"
      width="min(960px, 94vw)"
      ><template v-if="selected?.queue"
        ><div class="detail-current">
          <ElTag :type="tagType(selected)">{{
            states[selected.queue.state] ?? selected.queue.state
          }}</ElTag
          ><strong>{{
            selected.queue.current
              ? name(selected.queue.current.operation)
              : "暂无执行任务"
          }}</strong
          ><span v-if="selected.queue.current"
            >已执行 {{ duration(selected.queue.current.elapsed_ms) }} · 已发送
            {{ selected.queue.current.commands_started }} 条命令 · {{ selected.queue.current.last_command || "—" }}</span
          >
        </div>
        <ElAlert v-if="['quarantined', 'recovering'].includes(selected.queue.state)"
          type="warning" :closable="false" show-icon
          title="上一个命令未收到完整结束响应"
          description="端口会继续接收迟到响应，确认同步后自动恢复。超时不代表模组没有执行，请先核实连接或短信状态，避免重复操作。" />
        <h3 class="section-title">
          等待队列
          <span
            >{{ selected.queue.waiting_count }} /
            {{ selected.queue.capacity }}</span
          >
        </h3>
        <ElTable
          :data="selected.queue.waiting"
          empty-text="当前没有排队任务"
          max-height="240"
          ><ElTableColumn prop="id" label="事务" width="70" /><ElTableColumn
            prop="modem_id"
            label="所属模组"
            min-width="120"
          /><ElTableColumn label="操作" min-width="150"
            ><template #default="{ row }">{{
              name(row.operation)
            }}</template></ElTableColumn
          ><ElTableColumn label="已等待" min-width="100"
            ><template #default="{ row }">{{
              duration(row.queued_ms)
            }}</template></ElTableColumn
          ></ElTable
        >
        <h3 class="section-title">最近事务 <span>本次服务运行期间</span></h3>
        <ElTable
          :data="selected.queue.recent"
          empty-text="暂无已结束的事务"
          max-height="330"
          ><ElTableColumn prop="id" label="事务" width="65" /><ElTableColumn
            prop="modem_id"
            label="所属模组"
            min-width="110"
          /><ElTableColumn label="操作" min-width="140"
            ><template #default="{ row }"
              >{{ name(row.operation)
              }}<small v-if="row.caller_detached" class="detached"
                >请求方已离开</small
              ></template
            ></ElTableColumn
          ><ElTableColumn label="排队 / 执行" min-width="145"
            ><template #default="{ row }"
              >{{ duration(row.queued_ms) }} /
              {{ duration(row.elapsed_ms) }}</template
            ></ElTableColumn
          ><ElTableColumn prop="last_command" label="最后命令" min-width="145" /><ElTableColumn label="结果" min-width="150"
            ><template #default="{ row }"
              ><ElTag
                :type="
                  row.outcome === 'completed'
                    ? 'success'
                    : row.outcome === 'cancelled_before_start'
                      ? 'info'
                      : 'warning'
                "
                >{{ outcomes[row.outcome] ?? row.outcome }}</ElTag
              ></template
            ></ElTableColumn
          ></ElTable
        >
        <p class="detail-summary">
          队列满拒绝 {{ selected.queue.rejected_queue_full }} 次 · 启动前取消
          {{ selected.queue.cancelled_before_start }} 次
        </p></template
      ></ElDialog
    >
  </div>
</template>
