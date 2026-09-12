<script setup lang="ts">
import { onMounted, ref } from "vue";
import {
  ElButton,
  ElInput,
  ElSwitch,
  ElSelect,
  ElOption,
  ElInputNumber,
  ElForm,
  ElFormItem,
  ElAlert,
  ElDivider,
  ElMessageBox,
} from "element-plus";
const props = defineProps<{ token: string; modem: string }>();
const config = ref<any>(null),
  busy = ref(false),
  error = ref(""),
  notice = ref(""),
  result = ref<any>(null),
  dns = ref(""),
  hooks = ref("");
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
  if (!r.ok) throw new Error(body.error?.message || `HTTP ${r.status}`);
  return body.data;
}
async function run(fn: () => Promise<void>) {
  if (busy.value) return;
  error.value = "";
  notice.value = "";
  busy.value = true;
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
  config.value = await api("config");
  dns.value = config.value.network.dns.join("\n");
  hooks.value = config.value.network.pre_dial_commands.join("\n");
}
async function save() {
  config.value.network.dns = dns.value.split(/[\s,]+/).filter(Boolean);
  config.value.network.pre_dial_commands = hooks.value
    .split("\n")
    .map((s) => s.trim())
    .filter(Boolean);
  if (config.value.network.control_port === "")
    config.value.network.control_port = null;
  if (config.value.network.logical_interface === "")
    config.value.network.logical_interface = null;
  await api("config", "PUT", config.value);
  notice.value = "联网配置已保存";
}
async function operate(operation: string) {
  if (["connect", "disconnect", "redial"].includes(operation))
    await ElMessageBox.confirm("此操作会改变模组连接状态。", "联网操作", {
      confirmButtonText: "执行",
      cancelButtonText: "取消",
    });
  result.value = await api("network", "POST", { operation });
}
onMounted(() => run(load));
</script>
<template>
  <div v-if="config" class="network-panel">
    <ElAlert
      v-if="error"
      :title="error"
      type="error"
      :closable="false"
    /><ElAlert v-if="notice" :title="notice" type="success" :closable="false" />
    <div class="network-buttons">
      <ElButton
        type="primary"
        :loading="busy"
        @click="run(() => operate('connect'))"
        >连接</ElButton
      ><ElButton :disabled="busy" @click="run(() => operate('redial'))"
        >重新拨号</ElButton
      ><ElButton :disabled="busy" @click="run(() => operate('disconnect'))"
        >断开</ElButton
      ><ElButton :disabled="busy" @click="run(() => operate('status'))"
        >读取连接状态</ElButton
      >
    </div>
    <ElForm label-position="top" class="network-form">
      <ElFormItem label="自动联网"
        ><ElSwitch v-model="config.network.auto_connect" /></ElFormItem
      ><ElFormItem label="拨号方式"
        ><ElSelect v-model="config.network.driver"
          ><ElOption
            value="at"
            label="AT 拨号 + DHCP（ECM / NCM / RNDIS）" /><ElOption
            value="qmi"
            label="QMI（netifd）" /><ElOption
            value="mbim"
            label="MBIM（netifd）" /></ElSelect></ElFormItem
      ><ElFormItem label="数据网卡"
        ><ElInput
          v-model="config.interface"
          placeholder="例如 wwan0、usb0" /></ElFormItem
      ><ElFormItem
        v-if="config.network.driver !== 'at'"
        label="QMI / MBIM 控制端口"
        ><ElInput
          v-model="config.network.control_port"
          placeholder="例如 /dev/cdc-wdm0" /></ElFormItem
      ><ElFormItem label="逻辑接口名称（留空自动生成）"
        ><ElInput v-model="config.network.logical_interface" /></ElFormItem
      ><ElFormItem label="防火墙区域（留空不加入区域）"
        ><ElInput
          v-model="config.network.firewall_zone"
          placeholder="wan" /></ElFormItem
      ><ElFormItem label="地址类型"
        ><ElSelect v-model="config.network.pdp_type"
          ><ElOption value="ip" label="IPv4" /><ElOption
            value="ipv6"
            label="IPv6" /><ElOption
            value="ipv4v6"
            label="IPv4 + IPv6" /></ElSelect></ElFormItem
      ><ElFormItem label="PDP 索引"
        ><ElInputNumber
          v-model="config.pdp_index"
          :min="1"
          :max="16" /></ElFormItem
      ><ElFormItem label="路由优先级 metric"
        ><ElInputNumber v-model="config.network.metric" :min="0" /></ElFormItem
      ><ElFormItem label="模组 NAT"
        ><ElSwitch v-model="config.network.modem_nat" /></ElFormItem
      ><ElFormItem label="默认路由"
        ><ElSwitch v-model="config.network.default_route" /></ElFormItem
      ><ElFormItem label="使用运营商 DNS"
        ><ElSwitch v-model="config.network.peer_dns" /></ElFormItem
      ><ElFormItem label="IPv6 前缀委派"
        ><ElSwitch v-model="config.network.delegate" /></ElFormItem
      ><ElFormItem label="自定义 DNS（每行一个）"
        ><ElInput v-model="dns" type="textarea" :rows="3" /></ElFormItem
      ><ElFormItem label="拨号前 AT 命令（每行一条）"
        ><ElInput v-model="hooks" type="textarea" :rows="3"
      /></ElFormItem> </ElForm
    ><ElDivider content-position="left">主卡认证</ElDivider
    ><ElForm label-position="top" class="network-form"
      ><ElFormItem
        v-for="[key, label] in [
          ['apn', 'APN'],
          ['username', '用户名'],
          ['password', '密码'],
          ['pin', 'SIM PIN'],
        ]"
        :key="key"
        :label="label"
        ><ElInput
          v-model="config.network.credentials[key]"
          :type="['pin', 'password'].includes(key) ? 'password' : 'text'"
          :show-password="['pin', 'password'].includes(key)" /></ElFormItem
      ><ElFormItem label="认证方式"
        ><ElSelect v-model="config.network.credentials.auth"
          ><ElOption
            v-for="[key, label] in [
              ['', '默认'],
              ['none', '无认证'],
              ['pap', 'PAP'],
              ['chap', 'CHAP'],
              ['both', 'PAP / CHAP'],
            ]"
            :key="key"
            :value="key"
            :label="label" /></ElSelect></ElFormItem
    ></ElForm>
    <ElDivider content-position="left">第二张卡</ElDivider
    ><ElSwitch
      :model-value="!!config.network.sim2"
      @change="
        (v) => {
          config.network.sim2 = v
            ? { apn: '', username: '', password: '', auth: '', pin: '' }
            : null;
        }
      "
      active-text="为第二张卡设置独立 APN 和认证"
    /><ElForm
      v-if="config.network.sim2"
      label-position="top"
      class="network-form"
      ><ElFormItem
        v-for="[key, label] in [
          ['apn', 'APN'],
          ['username', '用户名'],
          ['password', '密码'],
          ['pin', 'SIM PIN'],
        ]"
        :key="key"
        :label="label"
        ><ElInput
          v-model="config.network.sim2[key]"
          :type="
            ['pin', 'password'].includes(key) ? 'password' : 'text'
          " /></ElFormItem
      ><ElFormItem label="认证方式"
        ><ElSelect v-model="config.network.sim2.auth"
          ><ElOption
            v-for="v in ['', 'none', 'pap', 'chap', 'both']"
            :key="v"
            :value="v"
            :label="v || '继承主卡'" /></ElSelect></ElFormItem
    ></ElForm>
    <div class="network-buttons">
      <ElButton type="primary" :loading="busy" @click="run(save)"
        >保存联网配置</ElButton
      ><ElButton :disabled="busy" @click="run(() => operate('plan'))"
        >查看生效参数</ElButton
      >
    </div>
    <details v-if="result" open>
      <summary>连接信息</summary>
      <pre>{{ JSON.stringify(result, null, 2) }}</pre>
    </details>
  </div>
</template>
<style scoped>
.network-buttons {
  display: flex;
  gap: 12px;
  flex-wrap: wrap;
  margin: 18px 0;
}
.network-buttons .el-button {
  margin-left: 0;
}
.network-form {
  display: grid;
  grid-template-columns: repeat(2, minmax(0, 1fr));
  gap: 0 24px;
  margin-top: 18px;
}
.el-alert {
  margin-bottom: 16px;
}
pre {
  white-space: pre-wrap;
  overflow-wrap: anywhere;
  padding: 16px;
  background: #f7f9fc;
  border-radius: 10px;
  font-size: 12px;
}
@media (max-width: 760px) {
  .network-form {
    grid-template-columns: 1fr;
  }
}
</style>
