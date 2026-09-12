<script setup lang="ts">
import { onMounted, ref, watch } from "vue";
import {
  ElButton,
  ElInput,
  ElSelect,
  ElOption,
  ElTable,
  ElTableColumn,
  ElTabs,
  ElTabPane,
  ElTag,
  ElAlert,
  ElForm,
  ElFormItem,
  ElInputNumber,
  ElMessageBox,
} from "element-plus";
import { requestId as newRequestId } from "./browser-utils";
import Forwarding from "./Forwarding.vue";
const props = defineProps<{ token: string; modem: string }>();
const historyFile = ref<HTMLInputElement | null>(null);
const tab = ref("history"),
  busy = ref(false),
  error = ref(""),
  notice = ref(""),
  messages = ref<any[]>([]),
  conversations = ref<any[]>([]),
  simMessages = ref<any[]>([]),
  peer = ref(""),
  recipient = ref(""),
  content = ref(""),
  rawPdu = ref(""),
  memory = ref("SM"),
  cursor = ref<number | null>(null),
  requestId = ref(newRequestId()),
  settings = ref<any>({
    mode: "manual",
    poll_interval_seconds: 30,
    memories: ["SM", "SM", "SM"],
  }),
  runtime = ref<any>(null);
const statuses: Record<string, string> = {
  sending: "发送中",
  submitted: "模组已接受",
  unknown: "发送结果未知",
  failed: "发送失败",
  received: "已接收",
  incomplete: "等待剩余分段",
};
async function api(path: string, method = "GET", data?: unknown) {
  const response = await fetch(`/api/v1/modems/${props.modem}/sms${path}`, {
    method,
    headers: {
      Authorization: `Bearer ${props.token}`,
      ...(data ? { "Content-Type": "application/json" } : {}),
    },
    body: data ? JSON.stringify(data) : undefined,
  });
  const result = await response.json();
  if (!response.ok)
    throw new Error(result.error?.message || `HTTP ${response.status}`);
  return result.data;
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
async function load(more = false) {
  const params = new URLSearchParams({ limit: "50" });
  if (peer.value) params.set("peer", peer.value);
  if (more && cursor.value) params.set("before", String(cursor.value));
  const data = await api(`?${params}`);
  messages.value = more ? [...messages.value, ...data.items] : data.items;
  cursor.value = data.next_cursor;
  conversations.value = (await api("/conversations")).items;
}
async function config() {
  const data = await api("/config");
  settings.value = data.config;
  runtime.value = data.runtime;
  memory.value = data.config.memories[0];
}
async function sync() {
  const result = await api("/sync", "POST", { memory: memory.value });
  notice.value = `同步完成：读取 ${result.listed} 条，新增 ${result.imported} 个分段`;
  if (result.errors.length) error.value = result.errors.join("；");
  await load();
}
async function send() {
  await ElMessageBox.confirm(
    `向 ${recipient.value} 发送短信？长短信会分多条发送，资费由运营商收取。`,
    "发送短信",
    { confirmButtonText: "发送", cancelButtonText: "取消" },
  );
  const result = await api("/send", "POST", {
    request_id: requestId.value,
    peer: recipient.value,
    content: content.value,
  });
  notice.value = statuses[result.delivery_status] || result.delivery_status;
  if (result.delivery_status === "unknown")
    error.value = "发送结果未知，请先查询模组或运营商记录，避免重复发送。";
  if (result.delivery_status === "submitted") {
    content.value = "";
    requestId.value = newRequestId();
  }
  await load();
}
watch([recipient, content, rawPdu], () => {
  requestId.value = newRequestId();
});
onMounted(() =>
  run(async () => {
    await config();
    await load();
  }),
);
async function remove(row: any) {
  await ElMessageBox.confirm(
    "删除本地历史记录？SIM 中的短信不受影响，下次同步可能再次导入。",
    "删除记录",
    { confirmButtonText: "删除", cancelButtonText: "取消" },
  );
  await api(`/${row.id}`, "DELETE");
  await load();
}
async function listSim() {
  const data = await api(`/sim?memory=${memory.value}`);
  simMessages.value = data.items;
  if (data.errors.length) error.value = data.errors.join("；");
}
async function deleteSim(row: any) {
  await ElMessageBox.confirm(
    "从模组存储中删除这条短信？本地历史记录会保留。",
    "删除模组短信",
    { confirmButtonText: "删除", cancelButtonText: "取消" },
  );
  await api("/sim", "DELETE", {
    index: row.index,
    expected_pdu: row.pdu,
    memory: memory.value,
  });
  await listSim();
}
const date = (n: number) => new Date(n * 1000).toLocaleString();
async function importHistory(event: Event) {
  const file = (event.target as HTMLInputElement).files?.[0];
  if (!file) return;
  await run(async () => {
    if (file.size > 30 * 1024 * 1024) throw Error("文件不能超过 30 MB");
    const document = JSON.parse(await file.text());
    const data = await api("/import", "POST", { source: file.name, document });
    notice.value =
      "已导入 " + data.imported + " 条，跳过重复 " + data.skipped + " 条";
  });
  (event.target as HTMLInputElement).value = "";
}
async function sendPdu() {
  await ElMessageBox.confirm("发送此 SMS-SUBMIT PDU？", "发送原始 PDU", {
    confirmButtonText: "发送",
    cancelButtonText: "取消",
  });
  const result = await api("/send-pdu", "POST", {
    request_id: requestId.value,
    pdu: rawPdu.value.replace(/\s+/g, ""),
  });
  notice.value = statuses[result.delivery_status] || result.delivery_status;
  if (result.delivery_status === "submitted") {
    rawPdu.value = "";
    requestId.value = newRequestId();
  }
  await load();
}
</script>
<template>
  <input
    ref="historyFile"
    type="file"
    accept=".json,application/json"
    hidden
    @change="importHistory"
  />
  <ElButton :disabled="busy" @click="historyFile?.click()"
    >导入原项目短信历史 JSON</ElButton
  >
  <div class="sms-workspace">
    <ElAlert
      v-if="error"
      :title="error"
      type="error"
      :closable="false"
    /><ElAlert v-if="notice" :title="notice" type="success" :closable="false" />
    <ElTabs v-model="tab">
      <ElTabPane label="短信历史" name="history"
        ><div class="sms-tools">
          <ElSelect
            v-model="peer"
            clearable
            placeholder="全部会话"
            @change="run(() => load())"
            ><ElOption
              v-for="c in conversations"
              :key="c.peer"
              :value="c.peer"
              :label="`${c.peer} · ${c.unread} 未读`" /></ElSelect
          ><ElButton :disabled="busy" @click="run(() => load())"
            >刷新历史</ElButton
          ><ElButton :loading="busy" type="primary" @click="run(sync)"
            >从模组同步</ElButton
          >
        </div>
        <ElTable :data="messages" empty-text="暂无短信" style="width: 100%"
          ><ElTableColumn label="联系人" min-width="135"
            ><template #default="{ row }"
              ><strong>{{ row.peer }}</strong>
              <div class="muted">
                {{ row.direction === "sent" ? "发出" : "收到" }}
              </div></template
            ></ElTableColumn
          ><ElTableColumn prop="content" label="内容" min-width="260"
            ><template #default="{ row }"
              ><div class="sms-content">{{ row.content || "二进制短信" }}</div>
              <ElTag v-if="row.delivery_status === 'incomplete'" type="warning"
                >长短信分段未齐</ElTag
              ></template
            ></ElTableColumn
          ><ElTableColumn label="时间 / 状态" min-width="175"
            ><template #default="{ row }"
              >{{ date(row.timestamp) }}
              <div class="muted">
                {{ statuses[row.delivery_status] || row.delivery_status }}
              </div></template
            ></ElTableColumn
          ><ElTableColumn label="操作" width="155"
            ><template #default="{ row }"
              ><ElButton
                text
                :disabled="busy"
                @click="
                  run(async () => {
                    await api(`/${row.id}`, 'PATCH', { is_read: !row.is_read });
                    await load();
                  })
                "
                >{{ row.is_read ? "设为未读" : "设为已读" }}</ElButton
              ><ElButton
                text
                type="danger"
                :disabled="busy || row.delivery_status === 'sending'"
                @click="run(() => remove(row))"
                >删除</ElButton
              ></template
            ></ElTableColumn
          ></ElTable
        ><ElButton
          v-if="cursor"
          class="more"
          :loading="busy"
          @click="run(() => load(true))"
          >加载更多</ElButton
        >
      </ElTabPane>
      <ElTabPane label="原始 PDU" name="pdu"
        ><ElForm label-position="top"
          ><ElFormItem label="SMS-SUBMIT PDU（包含 SMSC 长度字节）"
            ><ElInput
              type="textarea"
              :rows="6"
              v-model="rawPdu"
              :disabled="busy"
              placeholder="十六进制 PDU" /></ElFormItem
          ><ElButton
            type="primary"
            :disabled="!rawPdu"
            :loading="busy"
            @click="run(sendPdu)"
            >发送 PDU</ElButton
          ></ElForm
        ></ElTabPane
      >
      <ElTabPane label="发送短信" name="send"
        ><ElForm label-position="top" class="compose"
          ><ElFormItem label="收件号码"
            ><ElInput
              v-model="recipient"
              :disabled="busy"
              placeholder="手机号码或服务号码"
              maxlength="21" /></ElFormItem
          ><ElFormItem label="短信内容"
            ><ElInput
              v-model="content"
              :disabled="busy"
              type="textarea"
              :rows="7"
              maxlength="32768"
              show-word-limit
          /></ElFormItem>
          <p class="muted">自动选择 GSM7 或 Unicode 编码，长短信按标准分段。</p>
          <ElButton
            type="primary"
            :loading="busy"
            :disabled="!recipient || !content"
            @click="run(send)"
            >发送</ElButton
          ></ElForm
        ></ElTabPane
      >
      <ElTabPane label="模组存储" name="sim"
        ><div class="sms-tools">
          <ElSelect v-model="memory"
            ><ElOption
              v-for="m in ['SM', 'ME', 'MT']"
              :key="m"
              :value="m"
              :label="
                m === 'SM'
                  ? 'SIM 卡（SM）'
                  : m === 'ME'
                    ? '模组（ME）'
                    : '综合存储（MT）'
              " /></ElSelect
          ><ElButton :loading="busy" @click="run(listSim)">读取存储</ElButton
          ><ElButton :disabled="busy" @click="run(sync)">导入历史</ElButton>
        </div>
        <ElTable :data="simMessages" empty-text="点击读取存储查看短信"
          ><ElTableColumn prop="index" label="索引" width="80" /><ElTableColumn
            prop="decoded.peer"
            label="联系人"
            min-width="130"
          /><ElTableColumn
            prop="decoded.content"
            label="内容"
            min-width="260"
          /><ElTableColumn label="操作" width="90"
            ><template #default="{ row }"
              ><ElButton
                type="danger"
                text
                :disabled="busy"
                @click="run(() => deleteSim(row))"
                >删除</ElButton
              ></template
            ></ElTableColumn
          ></ElTable
        ></ElTabPane
      >
      <ElTabPane label="短信转发" name="forwarding" lazy
        ><Forwarding :key="modem" :token="token" :modem="modem"
      /></ElTabPane>
      <ElTabPane label="接收设置" name="config"
        ><ElForm label-position="top" class="compose"
          ><ElFormItem label="接收方式"
            ><ElSelect v-model="settings.mode"
              ><ElOption value="manual" label="手动同步" /><ElOption
                value="poll"
                label="定时同步到历史" /><ElOption
                value="urc"
                label="模组通知触发同步" /><ElOption
                value="sim_only"
                label="仅管理模组存储" /></ElSelect></ElFormItem
          ><ElFormItem label="同步 / 故障重试间隔（秒）"
            ><ElInputNumber
              v-model="settings.poll_interval_seconds"
              :min="5"
              :max="86400" /></ElFormItem
          ><ElFormItem
            v-for="(label, i) in ['读取存储', '写入存储', '接收存储']"
            :key="i"
            :label="label"
            ><ElSelect v-model="settings.memories[i]"
              ><ElOption
                v-for="m in ['SM', 'ME', 'MT']"
                :key="m"
                :value="m"
                :label="m" /></ElSelect></ElFormItem
          ><ElAlert
            v-if="settings.mode === 'urc'"
            title="通知模式按原项目校验型号与固件；不匹配时会显示故障，不会擅自更改通知命令。"
            type="info"
            :closable="false"
          />
          <div class="sms-tools">
            <ElButton
              type="primary"
              :loading="busy"
              @click="
                run(async () => {
                  await api('/config', 'PUT', settings);
                  notice = '接收设置已保存';
                  await config();
                })
              "
              >保存设置</ElButton
            ><ElButton :disabled="busy" @click="run(config)"
              >刷新接收状态</ElButton
            >
          </div>
          <ElAlert
            v-if="runtime?.error"
            :title="runtime.error"
            type="warning"
            :closable="false"
          />
          <p v-if="runtime" class="muted">
            接收状态：{{ runtime.state }} · 上次同步
            {{ runtime.last_sync_at ? date(runtime.last_sync_at) : "—" }}
          </p></ElForm
        ></ElTabPane
      >
    </ElTabs>
  </div>
</template>
<style scoped>
.sms-tools {
  display: flex;
  gap: 12px;
  flex-wrap: wrap;
  margin: 16px 0;
}
.sms-tools .el-select {
  width: 230px;
}
.sms-tools .el-button {
  margin-left: 0;
}
.muted {
  font-size: 12px;
  color: #8390a3;
  line-height: 1.7;
}
.sms-content {
  white-space: pre-wrap;
  overflow-wrap: anywhere;
}
.compose {
  max-width: 640px;
  padding: 16px 0;
}
.more {
  margin-top: 20px;
}
.el-alert {
  margin: 14px 0;
}
</style>
