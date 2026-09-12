<script setup lang="ts">
import { onMounted, ref } from "vue";
import {
  ElButton,
  ElSelect,
  ElOption,
  ElInput,
  ElSwitch,
  ElInputNumber,
  ElForm,
  ElFormItem,
  ElAlert,
  ElTable,
  ElTableColumn,
  ElTag,
} from "element-plus";
const props = defineProps<{ token: string; modem: string }>();
const settings = ref<any>(null),
  jobs = ref<any[]>([]),
  busy = ref(false),
  error = ref(""),
  notice = ref("");
const names: Record<string, string> = {
  telegram: "Telegram",
  webhook: "Webhook",
  serverchan: "Server 酱",
  pushdeer: "PushDeer",
  feishu: "飞书",
  custom: "自定义程序",
};
const templates: Record<string, any> = {
  telegram: { bot_token: "", chat_id: "" },
  webhook: { url: "", method: "POST", headers: {}, format: "" },
  serverchan: { token: "", channel: "", noip: "", openid: "" },
  pushdeer: { pushkey: "", endpoint: "https://api2.pushdeer.com" },
  feishu: { webhook_key: "" },
  custom: { path: "", args: [] },
};
const labels: Record<string, string> = {
  bot_token: "Bot Token",
  chat_id: "Chat ID",
  url: "请求地址",
  format: "正文模板（{SENDER} / {TIME} / {CONTENT}）",
  token: "SendKey",
  channel: "推送通道",
  noip: "隐藏 IP",
  openid: "OpenID",
  pushkey: "PushKey",
  endpoint: "服务地址",
  webhook_key: "Webhook Key",
  path: "可执行文件绝对路径",
};
async function api(path: string, method = "GET", data?: unknown) {
  const r = await fetch(`/api/v1/modems/${props.modem}/sms/${path}`, {
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
  settings.value = (await api("config")).config;
  jobs.value = (await api("deliveries")).items;
}
function addSink() {
  settings.value.forwarding.push({
    id: `sink_${settings.value.forwarding.length + 1}`,
    enabled: false,
    max_attempts: 5,
    target: { type: "webhook", ...structuredClone(templates.webhook) },
  });
}
function change(sink: any, type: string) {
  sink.target = { type, ...structuredClone(templates[type]) };
}
onMounted(() => run(load));
</script>
<template>
  <div v-if="settings">
    <ElAlert
      v-if="error"
      :title="error"
      type="error"
      :closable="false"
    /><ElAlert v-if="notice" :title="notice" type="success" :closable="false" />
    <p class="description">
      启用后，新接收的完整短信会转发到所选服务。历史短信不会自动补发。
    </p>
    <article v-for="(sink, i) in settings.forwarding" :key="i" class="sink">
      <div class="sink-head">
        <ElInput v-model="sink.id" placeholder="通道标识" /><ElSelect
          :model-value="sink.target.type"
          @change="(v) => change(sink, v)"
          ><ElOption
            v-for="(label, value) in names"
            :key="value"
            :label="label"
            :value="value" /></ElSelect
        ><ElSwitch v-model="sink.enabled" active-text="启用" /><ElButton
          type="danger"
          plain
          @click="settings.forwarding.splice(i, 1)"
          >移除</ElButton
        >
      </div>
      <ElForm label-position="top" class="sink-form"
        ><ElFormItem
          v-for="key in Object.keys(templates[sink.target.type]).filter(
            (k) => !['headers', 'args', 'method'].includes(k),
          )"
          :key="key"
          :label="labels[key] || key"
          ><ElInput
            v-model="sink.target[key]"
            :type="
              ['bot_token', 'token', 'pushkey', 'webhook_key'].includes(key)
                ? 'password'
                : key === 'format'
                  ? 'textarea'
                  : 'text'
            "
            :show-password="
              ['bot_token', 'token', 'pushkey', 'webhook_key'].includes(key)
            " /></ElFormItem
        ><ElFormItem v-if="sink.target.type === 'webhook'" label="HTTP 方法"
          ><ElSelect v-model="sink.target.method"
            ><ElOption
              v-for="v in ['GET', 'POST', 'PUT']"
              :key="v"
              :value="v"
              :label="v" /></ElSelect></ElFormItem
        ><ElFormItem
          v-if="sink.target.type === 'custom'"
          label="程序参数（每行一个参数）"
          ><ElInput
            type="textarea"
            :model-value="sink.target.args.join('\n')"
            @update:model-value="
              (v) => (sink.target.args = v.split('\n').filter(Boolean))
            " /></ElFormItem
        ><ElFormItem
          v-if="sink.target.type === 'webhook'"
          label="自定义 HTTP 请求头"
          ><div>
            <div
              v-for="(_, key) in sink.target.headers"
              :key="key"
              class="header-row"
            >
              <ElInput
                :model-value="String(key)"
                @change="
                  (v) => {
                    if (v && v !== String(key)) {
                      sink.target.headers[v] = sink.target.headers[key];
                      delete sink.target.headers[key];
                    }
                  }
                "
                placeholder="名称"
              /><ElInput
                v-model="sink.target.headers[key]"
                type="password"
                show-password
                placeholder="值"
              /><ElButton @click="delete sink.target.headers[key]"
                >移除</ElButton
              >
            </div>
            <ElButton @click="sink.target.headers['X-Custom-Header'] = ''"
              >添加请求头</ElButton
            >
          </div></ElFormItem
        ><ElFormItem label="最多尝试次数"
          ><ElInputNumber
            v-model="sink.max_attempts"
            :min="1"
            :max="20" /></ElFormItem
      ></ElForm>
    </article>
    <div class="forward-tools">
      <ElButton @click="addSink">添加通道</ElButton
      ><ElButton
        type="primary"
        :loading="busy"
        @click="
          run(async () => {
            await api('config', 'PUT', settings);
            notice = '转发配置已保存';
          })
        "
        >保存转发配置</ElButton
      ><ElButton
        :disabled="busy"
        @click="
          run(async () => {
            jobs = (await api('deliveries')).items;
          })
        "
        >刷新转发记录</ElButton
      >
    </div>
    <ElTable :data="jobs" empty-text="暂无转发任务"
      ><ElTableColumn prop="message_id" label="短信" width="85" /><ElTableColumn
        prop="sink_id"
        label="通道"
        min-width="120"
      /><ElTableColumn label="状态" min-width="115"
        ><template #default="{ row }"
          ><ElTag
            :type="
              row.state === 'delivered'
                ? 'success'
                : row.state === 'failed'
                  ? 'danger'
                  : 'info'
            "
            >{{ row.state }}</ElTag
          ></template
        ></ElTableColumn
      ><ElTableColumn
        prop="attempts"
        label="尝试次数"
        width="100"
      /><ElTableColumn
        prop="last_error"
        label="结果"
        min-width="180"
      /><ElTableColumn label="操作" width="90"
        ><template #default="{ row }"
          ><ElButton
            v-if="row.state === 'failed'"
            text
            :disabled="busy"
            @click="
              run(async () => {
                await api(`deliveries/${row.id}/retry`, 'POST', {});
                jobs = (await api('deliveries')).items;
              })
            "
            >重试</ElButton
          ></template
        ></ElTableColumn
      ></ElTable
    >
  </div>
</template>
<style scoped>
.sink {
  padding: 20px;
  border: 1px solid #e6edf5;
  border-radius: 12px;
  margin: 18px 0;
}
.sink-head,
.forward-tools {
  display: flex;
  gap: 12px;
  align-items: center;
  flex-wrap: wrap;
  margin-bottom: 18px;
}
.sink-head > .el-input,
.sink-head > .el-select {
  width: 190px;
}
.sink-form {
  display: grid;
  grid-template-columns: repeat(2, minmax(0, 1fr));
  gap: 0 20px;
}
.description {
  color: #758196;
  font-size: 13px;
}
.el-alert {
  margin: 14px 0;
}
@media (max-width: 760px) {
  .sink-form {
    grid-template-columns: 1fr;
  }
  .sink {
    padding: 12px;
  }
}
</style>
