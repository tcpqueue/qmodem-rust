<script setup lang="ts">
import { computed, onMounted, ref } from "vue";
import {
  ElButton,
  ElForm,
  ElFormItem,
  ElSwitch,
  ElSelect,
  ElOption,
  ElInput,
  ElInputNumber,
  ElDivider,
  ElAlert,
  ElTable,
  ElTableColumn,
  ElTag,
} from "element-plus";
const props = defineProps<{ token: string; modem: string }>();
const config = ref<any>(null),
  history = ref<any[]>([]),
  busy = ref(false),
  error = ref(""),
  notice = ref("");
async function api(path: string, method = "GET", data?: unknown) {
  const r = await fetch(`/api/v1/modems/${props.modem}/${path}`, {
    method,
    headers: {
      Authorization: `Bearer ${props.token}`,
      ...(data ? { "Content-Type": "application/json" } : {}),
    },
    body: data ? JSON.stringify(data) : undefined,
  });
  const body = await r.json();
  if (!r.ok) throw new Error(body.error?.message);
  return body.data;
}
async function run(fn: () => Promise<void>) {
  if (busy.value) return;
  busy.value = true;
  error.value = "";
  notice.value = "";
  try {
    await fn();
  } catch (e) {
    error.value = e instanceof Error ? e.message : String(e);
  } finally {
    busy.value = false;
  }
}
async function load() {
  config.value = await api("maintenance");
  history.value = (await api("traffic/history")).items;
}
onMounted(() => run(load));
const rates = computed(() =>
  history.value
    .slice()
    .reverse()
    .slice(-120)
    .map((v, i, a) => {
      const old = a[i - 1];
      const seconds = old ? v.timestamp - old.timestamp : 0;
      return {
        timestamp: v.timestamp,
        rx:
          old && seconds > 0 && old.source === v.source
            ? Math.max(0, v.rx_bytes - old.rx_bytes) / seconds
            : 0,
        tx:
          old && seconds > 0 && old.source === v.source
            ? Math.max(0, v.tx_bytes - old.tx_bytes) / seconds
            : 0,
      };
    }),
);
const maximum = computed(() =>
  Math.max(1, ...rates.value.flatMap((v) => [v.rx, v.tx])),
);
const points = (key: "rx" | "tx") =>
  rates.value
    .map(
      (v, i) =>
        `${20 + (i * 760) / Math.max(1, rates.value.length - 1)},${150 - (v[key] / maximum.value) * 125}`,
    )
    .join(" ");
const date = (n: number) => new Date(n * 1000).toLocaleString();
</script>
<template>
  <div v-if="config">
    <ElAlert
      v-if="error"
      :title="error"
      type="error"
      :closable="false"
    /><ElAlert
      v-if="notice"
      :title="notice"
      type="success"
      :closable="false"
    /><ElDivider content-position="left">自动恢复连接</ElDivider
    ><ElForm label-position="top" class="maintenance-form"
      ><ElFormItem label="启用连通性监测"
        ><ElSwitch v-model="config.monitor.enabled" /></ElFormItem
      ><ElFormItem label="检测方式"
        ><ElSelect v-model="config.monitor.method"
          ><ElOption value="ping" label="Ping 指定 IP" /><ElOption
            value="gateway"
            label="Ping 网关" /><ElOption
            value="dns"
            label="Ping DNS" /><ElOption
            value="http"
            label="HTTP 请求" /></ElSelect></ElFormItem
      ><ElFormItem
        v-if="['ping', 'http'].includes(config.monitor.method)"
        label="检测目标"
        ><ElInput v-model="config.monitor.target" /></ElFormItem
      ><ElFormItem label="IP 版本"
        ><ElSelect v-model="config.monitor.ip_version"
          ><ElOption :value="4" label="IPv4" /><ElOption
            :value="6"
            label="IPv6" /></ElSelect></ElFormItem
      ><ElFormItem
        v-for="[key, label, min, max] in [
          ['interval_seconds', '检测间隔（秒）', 3, 86400],
          ['threshold', '连续失败阈值', 1, 1000],
          ['readiness_grace', '启动等待次数', 0, 1000],
          ['cooldown_seconds', '动作冷却（秒）', 0, 86400],
        ]"
        :key="key"
        :label="String(label)"
        ><ElInputNumber
          v-model="config.monitor[key]"
          :min="Number(min)"
          :max="Number(max)" /></ElFormItem
    ></ElForm>
    <div
      v-for="(action, i) in config.monitor.actions"
      :key="i"
      class="action-row"
    >
      <ElSelect
        v-model="action.action"
        @change="
          (v) => {
            config.monitor.actions[i] =
              v === 'at'
                ? { action: v, commands: [] }
                : v === 'exec'
                  ? { action: v, path: '', args: [] }
                  : { action: v };
          }
        "
        ><ElOption value="redial" label="重新拨号" /><ElOption
          value="switch_sim"
          label="切换 SIM 并重拨" /><ElOption
          value="at"
          label="发送 AT 命令" /><ElOption
          value="exec"
          label="运行自定义程序" /></ElSelect
      ><ElInput
        v-if="action.action === 'at'"
        :model-value="action.commands.join('\n')"
        type="textarea"
        placeholder="每行一条 AT 命令"
        @update:model-value="
          (v) => (action.commands = v.split('\n').filter(Boolean))
        "
      /><ElInput
        v-if="action.action === 'exec'"
        v-model="action.path"
        placeholder="可执行文件绝对路径"
      /><ElInput
        v-if="action.action === 'exec'"
        type="textarea"
        :model-value="action.args.join('\n')"
        @update:model-value="
          (v) => (action.args = v.split('\n').filter(Boolean))
        "
        placeholder="程序参数，每行一个"
      /><ElButton
        type="danger"
        plain
        @click="config.monitor.actions.splice(i, 1)"
        >移除</ElButton
      >
    </div>
    <ElButton @click="config.monitor.actions.push({ action: 'redial' })"
      >添加恢复动作</ElButton
    >
    <ElDivider content-position="left">流量历史</ElDivider
    ><ElForm label-position="top" class="maintenance-form"
      ><ElFormItem label="启用采样"
        ><ElSwitch v-model="config.traffic.enabled" /></ElFormItem
      ><ElFormItem label="同时保存模组计数"
        ><ElSwitch v-model="config.traffic.save_modem_counters" /></ElFormItem
      ><ElFormItem label="采样间隔（秒）"
        ><ElInputNumber
          v-model="config.traffic.interval_seconds"
          :min="10"
          :max="86400" /></ElFormItem
      ><ElFormItem label="历史保留天数（0 为不清理）"
        ><ElInputNumber
          v-model="config.traffic.retention_days"
          :min="0"
          :max="3650" /></ElFormItem
    ></ElForm>
    <ElDivider content-position="left">定时清零模组流量</ElDivider>
    <ElForm label-position="top" class="maintenance-form">
      <ElFormItem label="启用定时清零"
        ><ElSwitch v-model="config.traffic.reset.enabled"
      /></ElFormItem>
      <ElFormItem label="周期"
        ><ElSelect v-model="config.traffic.reset.kind"
          ><ElOption value="daily" label="每天" /><ElOption
            value="weekly"
            label="每周" /><ElOption value="monthly" label="每月" /></ElSelect
      ></ElFormItem>
      <ElFormItem label="时间（路由器本地小时）"
        ><ElInputNumber v-model="config.traffic.reset.hour" :min="0" :max="23"
      /></ElFormItem>
      <ElFormItem
        v-if="config.traffic.reset.kind !== 'daily'"
        :label="
          config.traffic.reset.kind === 'weekly'
            ? '星期（0 为周日）'
            : '日期（短月没有该日期时跳过）'
        "
        ><ElInputNumber
          v-model="config.traffic.reset.day"
          :min="config.traffic.reset.kind === 'weekly' ? 0 : 1"
          :max="config.traffic.reset.kind === 'weekly' ? 6 : 31"
      /></ElFormItem>
    </ElForm>
    <div class="maintenance-tools">
      <ElButton
        type="primary"
        :loading="busy"
        @click="
          run(async () => {
            await api('maintenance', 'PUT', {
              monitor: config.monitor,
              traffic: config.traffic,
            });
            notice = '自动维护设置已保存';
          })
        "
        >保存设置</ElButton
      ><ElButton :disabled="busy" @click="run(load)">刷新状态与历史</ElButton>
    </div>
    <ElAlert
      v-if="config.runtime?.watchdog"
      :title="`连通性：${config.runtime.watchdog.state} · 失败 ${config.runtime.watchdog.failures ?? 0} 次`"
      :type="config.runtime.watchdog.state === 'healthy' ? 'success' : 'info'"
      :closable="false"
    />
    <div v-if="rates.length > 1" class="traffic-chart">
      <p>
        <ElTag>下载</ElTag> <ElTag type="success">上传</ElTag> · 最近
        {{ rates.length }} 个采样 · 最高 {{ (maximum / 1024).toFixed(1) }} KiB/s
      </p>
      <svg viewBox="0 0 800 175" role="img" aria-label="上传和下载速率趋势">
        <path d="M20 25 V150 H780" fill="none" stroke="#dfe5ef" />
        <polyline
          :points="points('rx')"
          fill="none"
          stroke="#647cf4"
          stroke-width="2"
        />
        <polyline
          :points="points('tx')"
          fill="none"
          stroke="#32bfa1"
          stroke-width="2"
        />
      </svg>
    </div>
    <ElTable :data="history.slice(0, 100)" empty-text="尚无流量采样"
      ><ElTableColumn label="采样时间" min-width="180"
        ><template #default="{ row }">{{
          date(row.timestamp)
        }}</template></ElTableColumn
      ><ElTableColumn label="下载计数" min-width="130"
        ><template #default="{ row }"
          >{{ (row.rx_bytes / 1024 ** 3).toFixed(3) }} GiB</template
        ></ElTableColumn
      ><ElTableColumn label="上传计数" min-width="130"
        ><template #default="{ row }"
          >{{ (row.tx_bytes / 1024 ** 3).toFixed(3) }} GiB</template
        ></ElTableColumn
      ></ElTable
    >
  </div>
</template>
<style scoped>
.maintenance-form {
  display: grid;
  grid-template-columns: repeat(2, minmax(0, 1fr));
  gap: 0 24px;
}
.action-row,
.maintenance-tools {
  display: flex;
  gap: 12px;
  align-items: flex-start;
  margin: 18px 0;
  flex-wrap: wrap;
}
.action-row > .el-select {
  width: 200px;
}
.action-row > .el-input,
.action-row > .el-textarea {
  flex: 1;
  min-width: 200px;
}
.el-alert {
  margin: 14px 0;
}
.traffic-chart {
  background: #f9fbff;
  padding: 18px;
  border-radius: 12px;
  margin: 20px 0;
}
.traffic-chart svg {
  width: 100%;
  height: auto;
}
.traffic-chart p {
  font-size: 12px;
  color: #728095;
}
@media (max-width: 760px) {
  .maintenance-form {
    grid-template-columns: 1fr;
  }
}
</style>
