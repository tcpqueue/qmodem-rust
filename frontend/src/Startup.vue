<script setup lang="ts">
import { onMounted, ref } from "vue";
import {
  ElButton,
  ElInput,
  ElInputNumber,
  ElSwitch,
  ElForm,
  ElFormItem,
  ElAlert,
  ElTag,
  ElMessageBox,
} from "element-plus";
const props = defineProps<{ token: string; modem: string }>();
const config = ref<any>(null),
  runtime = ref<any>(null),
  busy = ref(false),
  error = ref(""),
  notice = ref("");
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
  if (!r.ok) throw Error(body.error?.message || "HTTP " + r.status);
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
    if (e !== "cancel" && e !== "close")
      error.value = e instanceof Error ? e.message : String(e);
  } finally {
    busy.value = false;
  }
}
async function load() {
  const data = await api("startup");
  config.value = data.config;
  runtime.value = data.runtime;
}
async function save() {
  for (const key of ["gpio_value_path", "sim_led", "network_led"])
    if (!config.value[key]) config.value[key] = null;
  await api("startup", "PUT", config.value);
  notice.value = "已保存，设备初始化将应用新设置";
}
async function reboot() {
  await ElMessageBox.confirm("重启模组会暂时断网。", "重启模组", {
    confirmButtonText: "重启",
    cancelButtonText: "取消",
    type: "warning",
  });
  const result = await api("reboot", "POST", {});
  if (!result.success) throw Error("模组拒绝重启命令");
  notice.value = "重启命令已执行";
}
onMounted(() => run(load));
const labels: Record<string, string> = {
  pending: "初始化中",
  ready: "初始化完成",
  ready_degraded: "部分初始化命令未成功",
  failed: "初始化失败，等待重试",
};
</script>
<template>
  <div>
    <ElAlert v-if="error" :title="error" type="error" :closable="false" />
    <ElAlert v-if="notice" :title="notice" type="success" :closable="false" />
    <template v-if="config">
      <p>
        初始化状态：<ElTag>{{ labels[runtime?.state] || "等待设备" }}</ElTag>
        <ElButton text :disabled="busy" @click="run(load)">刷新</ElButton>
      </p>
      <ElForm label-position="top" class="startup-form">
        <ElFormItem label="开机初始化延迟（秒）"
          ><ElInputNumber v-model="config.delay_seconds" :min="0" :max="120"
        /></ElFormItem>
        <ElFormItem label="锁小区恢复延迟（秒）"
          ><ElInputNumber
            v-model="config.cell_lock_delay_seconds"
            :min="0"
            :max="120"
        /></ElFormItem>
        <ElFormItem label="路由器关机时重启模组"
          ><ElSwitch v-model="config.shutdown_reboot"
        /></ElFormItem>
        <ElFormItem label="GPIO 复位节点（留空使用 AT 软重启）"
          ><ElInput
            v-model="config.gpio_value_path"
            placeholder="/sys/class/gpio/gpioN/value"
        /></ElFormItem>
        <ElFormItem label="GPIO 高电平有效"
          ><ElSwitch v-model="config.gpio_active_high"
        /></ElFormItem>
        <ElFormItem label="SIM 就绪指示灯"
          ><ElInput
            v-model="config.sim_led"
            placeholder="/sys/class/leds/.../brightness"
        /></ElFormItem>
        <ElFormItem label="网络连接指示灯"
          ><ElInput
            v-model="config.network_led"
            placeholder="/sys/class/leds/.../brightness"
        /></ElFormItem>
        <ElFormItem label="初始化 AT 命令（每行一条）"
          ><ElInput
            type="textarea"
            :rows="5"
            :model-value="config.commands.join('\n')"
            @update:model-value="
              (v) => (config.commands = v.split('\n').filter(Boolean))
            "
        /></ElFormItem>
      </ElForm>
      <p>
        开机锁小区：{{
          config.cell_lock
            ? config.cell_lock.rat.toUpperCase() +
              " · ARFCN " +
              config.cell_lock.arfcn
            : "未设置"
        }}。在“邻区与锁小区”中设置并保存。
      </p>
      <ElButton type="primary" :loading="busy" @click="run(save)"
        >保存初始化设置</ElButton
      >
      <ElButton type="danger" plain :disabled="busy" @click="run(reboot)"
        >重启模组</ElButton
      >
    </template>
  </div>
</template>
<style scoped>
.startup-form {
  display: grid;
  grid-template-columns: repeat(auto-fit, minmax(240px, 1fr));
  gap: 0 20px;
}
p {
  line-height: 1.8;
}
</style>
