<script setup lang="ts">
import { onMounted, ref, computed } from "vue";
import {
  ElButton,
  ElInput,
  ElInputNumber,
  ElSelect,
  ElOption,
  ElForm,
  ElFormItem,
  ElAlert,
  ElDivider,
  ElEmpty,
} from "element-plus";
const props = defineProps<{ token: string; modem: string }>();
const busy = ref(false),
  error = ref(""),
  command = ref("AT"),
  timeout = ref(10000),
  port = ref("primary"),
  catalogue = ref<any>({}),
  logs = ref<any[]>([]),
  result = ref<any>(null);
const commands = computed(() =>
  [
    ...(catalogue.value.general || []),
    ...(catalogue.value.vendor || []),
  ].flatMap((v) =>
    Object.entries(v).map(([label, value]) => ({
      label,
      value: String(value),
    })),
  ),
);
async function api(path: string, method = "GET", data?: unknown) {
  const r = await fetch("/api/v1/modems/" + props.modem + "/" + path, {
    method,
    headers: {
      Authorization: "Bearer " + props.token,
      ...(data ? { "Content-Type": "application/json" } : {}),
    },
    body: data ? JSON.stringify(data) : undefined,
  });
  const body = await r.json();
  if (!r.ok) throw Error(body.error?.message);
  return body.data;
}
async function run(fn: () => Promise<void>) {
  if (busy.value) return;
  busy.value = true;
  error.value = "";
  try {
    await fn();
  } catch (e) {
    error.value = e instanceof Error ? e.message : String(e);
  } finally {
    busy.value = false;
  }
}
async function loadLogs() {
  logs.value = (await api("logs")).items;
}
onMounted(() =>
  run(async () => {
    catalogue.value = await api("debug/config");
    await loadLogs();
  }),
);
</script>
<template>
  <div>
    <ElAlert v-if="error" :title="error" type="error" :closable="false" />
    <ElForm label-position="top" class="debug-form">
      <ElFormItem label="命令快捷选择"
        ><ElSelect
          v-model="command"
          filterable
          allow-create
          default-first-option
          ><ElOption
            v-for="(c, i) in commands"
            :key="i"
            :label="c.label"
            :value="c.value" /></ElSelect
      ></ElFormItem>
      <ElFormItem label="目标端口"
        ><ElSelect v-model="port"
          ><ElOption
            value="primary"
            :label="'主 AT · ' + (catalogue.ports?.primary || '')" /><ElOption
            value="sms"
            :label="
              '短信 AT · ' +
              (catalogue.ports?.sms || catalogue.ports?.primary || '')
            " /></ElSelect
      ></ElFormItem>
      <ElFormItem label="AT 命令"
        ><ElInput v-model="command" :disabled="busy"
      /></ElFormItem>
      <ElFormItem label="超时（毫秒）"
        ><ElInputNumber v-model="timeout" :min="1" :max="120000"
      /></ElFormItem>
    </ElForm>
    <ElButton
      type="primary"
      :loading="busy"
      @click="
        run(async () => {
          result = await api('at', 'POST', {
            command,
            port,
            timeout_ms: timeout,
          });
        })
      "
      >发送命令</ElButton
    >
    <pre v-if="result">{{ JSON.stringify(result, null, 2) }}</pre>
    <ElDivider content-position="left">模组运行日志</ElDivider>
    <ElButton :disabled="busy" @click="run(loadLogs)">刷新日志</ElButton>
    <ElButton
      :disabled="busy"
      @click="
        run(async () => {
          await api('logs', 'DELETE');
          await loadLogs();
        })
      "
      >清除后台日志缓存</ElButton
    >
    <p class="muted">
      显示当前进程中与此模组关联的日志。等级在 LuCI 中设置，系统日志由 OpenWrt
      保留。
    </p>
    <pre v-if="logs.length">{{ logs.map((v) => v.line).join("\n") }}</pre>
    <ElEmpty v-else description="暂无此模组的运行日志" />
  </div>
</template>
<style scoped>
.debug-form {
  display: grid;
  grid-template-columns: repeat(auto-fit, minmax(260px, 1fr));
  gap: 0 20px;
}
pre {
  max-height: 400px;
  overflow: auto;
  white-space: pre-wrap;
  overflow-wrap: anywhere;
  background: var(--surface-muted, #f5f7fa);
  padding: 16px;
  border-radius: 8px;
}
.muted {
  color: #7c8597;
  font-size: 13px;
}
</style>
