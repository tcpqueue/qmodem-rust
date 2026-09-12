<script setup lang="ts">
import { computed, onMounted, ref, watch } from "vue";
import {
  ElButton,
  ElSelect,
  ElOption,
  ElInput,
  ElInputNumber,
  ElSwitch,
  ElTag,
  ElAlert,
  ElTable,
  ElTableColumn,
  ElTabs,
  ElTabPane,
  ElForm,
  ElFormItem,
  ElEmpty,
  ElDescriptions,
  ElDescriptionsItem,
  ElDialog,
  ElCheckboxGroup,
  ElCheckbox,
  ElMessageBox,
} from "element-plus";
import Sms from "./Sms.vue";
import Network from "./Network.vue";
import Maintenance from "./Maintenance.vue";
import Startup from "./Startup.vue";
import Debug from "./Debug.vue";
const props = defineProps<{ token: string; page: string }>();
const emit = defineEmits<{ changed: [] }>();
type RecordData = Record<string, any>;
const items = ref<RecordData[]>([]),
  selected = ref(""),
  devices = ref<RecordData[]>([]),
  status = ref<RecordData | null>(null),
  config = ref<RecordData | null>(null),
  error = ref(""),
  message = ref(""),
  busy = ref(false),
  result = ref<RecordData | null>(null),
  tab = ref("status"),
  editor = ref(false);
const mode = ref("qmi"),
  networks = ref(["4G", "5G"]),
  sim = ref(1),
  imei = ref(""),
  bandClass = ref("lte"),
  bands = ref(""),
  lock = ref({ rat: "lte", arfcn: 1850, pci: 0, scs: 1, band: 78 }),
  persistLock = ref(true),
  capabilities = ref<string[]>([]),
  neighbors = ref<RecordData | null>(null),
  usage = ref<RecordData | null>(null);
const current = computed(() =>
  items.value.find((m) => m.id === selected.value),
);
const supported = (name: string) => capabilities.value.includes(name);
async function api(path: string, method = "GET", body?: unknown) {
  const response = await fetch(`/api/v1/${path}`, {
    method,
    headers: {
      Authorization: `Bearer ${props.token}`,
      ...(body ? { "Content-Type": "application/json" } : {}),
    },
    body: body ? JSON.stringify(body) : undefined,
  });
  const value = await response.json();
  if (!response.ok) {
    result.value = value.error?.details ?? null;
    throw new Error(value.error?.message ?? `HTTP ${response.status}`);
  }
  return value.data;
}
async function run(fn: () => Promise<void>) {
  if (busy.value) return;
  busy.value = true;
  error.value = "";
  message.value = "";
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
  items.value = (await api("modems")).items;
  if (!items.value.some((m) => m.id === selected.value))
    selected.value = items.value[0]?.id ?? "";
}
async function select() {
  status.value = null;
  neighbors.value = null;
  usage.value = null;
  result.value = null;
  capabilities.value = [];
  if (selected.value)
    capabilities.value = (
      await api(`modems/${selected.value}/capabilities`)
    ).operations;
}
watch(selected, () => run(select));
onMounted(() =>
  run(async () => {
    await load();
    await select();
    if (props.page === "discovery") await scan();
  }),
);
watch(
  () => props.page,
  () => {
    if (props.page === "discovery") run(scan);
  },
);
async function scan() {
  devices.value = (await api("discovery")).items;
}
async function probe(device: RecordData) {
  const identified = await api(`discovery/${device.id}/probe`, "POST", {});
  devices.value = devices.value.map((d) =>
    d.id === device.id ? identified : d,
  );
}
async function register(device: RecordData) {
  config.value = structuredClone(device.modem);
  editor.value = true;
}
async function edit() {
  config.value = await api(`modems/${selected.value}/config`);
  editor.value = true;
}
function create() {
  config.value = {
    id: "modem1",
    name: "新模组",
    enabled: true,
    manufacturer: "quectel",
    model: "",
    platform: "qualcomm",
    bus: "usb",
    at_port: "/dev/ttyUSB2",
    sms_at_port: null,
    interface: null,
    pdp_index: 1,
    apn: "",
    bands: {},
  };
  editor.value = true;
}
async function save() {
  if (!config.value) return;
  await api(
    `modems/${encodeURIComponent(config.value.id)}/config`,
    "PUT",
    config.value,
  );
  selected.value = config.value.id;
  editor.value = false;
  await load();
  await select();
  emit("changed");
  message.value = "模组配置已保存并生效";
}
async function remove() {
  await ElMessageBox.confirm(
    "删除该模组的配置？已保存的历史数据仍然保留。",
    "删除模组",
    { type: "warning", confirmButtonText: "删除", cancelButtonText: "取消" },
  );
  await api(`modems/${selected.value}/config`, "DELETE");
  await load();
  await select();
  emit("changed");
}
async function action(operation: string, args: RecordData = {}) {
  result.value = null;
  const value = await api(`modems/${selected.value}/actions`, "POST", {
    operation,
    ...args,
  });
  result.value = value;
  message.value = "操作完成";
  return value.data;
}
async function change(operation: string, args: RecordData = {}) {
  await ElMessageBox.confirm(
    "此操作会修改模组设置，可能暂时断网。",
    "应用设置",
    { confirmButtonText: "应用", cancelButtonText: "取消", type: "warning" },
  );
  await action(operation, args);
  status.value = null;
}
async function cellLock(value: RecordData | null) {
  await ElMessageBox.confirm(
    value ? "应用锁小区设置？" : "解除锁小区？",
    "锁小区",
    { confirmButtonText: "应用", cancelButtonText: "取消" },
  );
  result.value = await api("modems/" + selected.value + "/cell-lock", "PUT", {
    lock: value,
    persist: persistLock.value,
  });
  message.value = persistLock.value
    ? "已应用，并保存开机恢复设置"
    : "已应用，仅本次生效";
}
const metrics = [
  ["model", "型号"],
  ["firmware", "固件"],
  ["sim_status", "SIM 状态"],
  ["operator", "运营商"],
  ["network_type", "网络类型"],
  ["temperature_c", "温度 °C"],
  ["voltage_mv", "电压 mV"],
  ["rssi_dbm", "RSSI dBm"],
  ["imei", "IMEI"],
  ["imsi", "IMSI"],
  ["iccid", "ICCID"],
  ["phone_number", "号码"],
];
</script>
<template>
  <section class="device-workbench">
    <ElAlert
      v-if="error"
      :title="error"
      type="error"
      :closable="false"
      show-icon
    />
    <ElAlert v-if="message" :title="message" type="success" :closable="false" />
    <template v-if="page === 'discovery'">
      <div class="workbench-bar">
        <div>
          <h2>发现设备</h2>
          <p>扫描 USB / PCIe，识别移远与 TD Tech MT5700。</p>
        </div>
        <ElButton :loading="busy" @click="run(scan)">重新扫描</ElButton>
      </div>
      <ElEmpty v-if="!devices.length" description="未发现支持范围内的设备" />
      <article
        v-for="device in devices"
        :key="device.id"
        class="device-scan-card"
      >
        <div class="workbench-bar">
          <h3>
            {{ device.modem?.model || device.id }}
            <ElTag>{{ device.bus.toUpperCase() }}</ElTag>
          </h3>
          <ElTag v-if="device.modem" type="success">型号已识别</ElTag>
        </div>
        <p>
          {{ device.vendor_id }}:{{ device.product_id }} ·
          {{ device.network_interfaces.join(" / ") || "网卡未就绪" }}
        </p>
        <p>AT 候选端口：{{ device.at_candidates.join(" / ") || "无" }}</p>
        <p v-if="device.voice_pcm_port">
          语音 PCM：{{ device.voice_pcm_port }}
        </p>
        <ElAlert
          v-for="issue in device.errors"
          :key="issue"
          :title="issue"
          type="warning"
          :closable="false"
        />
        <div class="workbench-actions">
          <ElButton
            :disabled="busy || !device.at_candidates.length"
            @click="run(() => probe(device))"
            >识别型号</ElButton
          ><ElButton
            v-if="device.needs_option_binding"
            :disabled="busy"
            @click="
              run(async () => {
                await api(`discovery/${device.id}/bind`, 'POST', {});
                await scan();
              })
            "
            >加载串口绑定</ElButton
          ><ElButton
            v-if="device.modem"
            type="primary"
            :disabled="busy"
            @click="register(device)"
            >添加到配置</ElButton
          >
        </div>
      </article>
    </template>
    <template v-else>
      <div class="workbench-bar">
        <ElSelect v-model="selected" placeholder="选择模组" :disabled="busy"
          ><ElOption
            v-for="m in items"
            :key="m.id"
            :label="m.name || m.id"
            :value="m.id"
        /></ElSelect>
        <div class="workbench-actions">
          <ElButton @click="create">添加模组</ElButton
          ><ElButton :disabled="!selected || busy" @click="run(edit)"
            >编辑配置</ElButton
          ><ElButton
            :disabled="!selected || busy"
            type="danger"
            plain
            @click="run(remove)"
            >删除</ElButton
          >
        </div>
      </div>
      <ElEmpty v-if="!selected" description="请先发现设备或添加模组配置" />
      <ElTabs v-else v-model="tab">
        <ElTabPane label="运行状态" name="status">
          <div class="workbench-bar">
            <span
              >{{ current?.name }} <ElTag>{{ current?.bus }}</ElTag></span
            ><ElButton
              type="primary"
              :loading="busy"
              @click="
                run(async () => {
                  status = await api(`modems/${selected}/status`);
                })
              "
              >刷新状态</ElButton
            >
          </div>
          <template v-if="status"
            ><ElAlert
              v-if="status.partial"
              title="部分查询失败，缺失字段显示为 —"
              type="warning"
              :closable="false"
            /><ElDescriptions :column="2" border
              ><ElDescriptionsItem
                v-for="[key, label] in metrics"
                :key="key"
                :label="label"
                >{{ status[key] ?? "—" }}</ElDescriptionsItem
              ><ElDescriptionsItem label="PDP 地址">{{
                status.addresses.join(" / ") || "—"
              }}</ElDescriptionsItem></ElDescriptions
            >
            <h3>服务小区</h3>
            <ElTable empty-text="暂无数据" :data="status.cells"
              ><ElTableColumn
                v-for="[key, label] in [
                  ['rat', '网络'],
                  ['band', '频段'],
                  ['arfcn', '频点'],
                  ['pci', 'PCI'],
                  ['rsrp', 'RSRP'],
                  ['rsrq', 'RSRQ'],
                  ['sinr', 'SINR'],
                ]"
                :key="key"
                :prop="key"
                :label="label"
                min-width="95"
            /></ElTable>
            <details>
              <summary>查询响应</summary>
              <pre>{{ JSON.stringify(status.queries, null, 2) }}</pre>
            </details></template
          >
          <ElEmpty v-else description="点击刷新读取模组状态" />
        </ElTabPane>
        <ElTabPane label="联网配置" name="network" lazy
          ><Network :key="selected" :token="token" :modem="selected"
        /></ElTabPane>
        <ElTabPane label="模组设置" name="settings">
          <div class="settings-grid">
            <section>
              <h3>USB 工作模式</h3>
              <ElSelect v-model="mode"
                ><ElOption
                  v-for="v in current?.manufacturer === 'tdtech'
                    ? ['ecm', 'ncm']
                    : ['qmi', 'ecm', 'mbim', 'rndis', 'ncm']"
                  :key="v"
                  :value="v"
                  :label="v.toUpperCase()"
              /></ElSelect>
              <div class="workbench-actions">
                <ElButton
                  :disabled="busy"
                  @click="
                    run(() =>
                      action('get_mode').then((v) => {
                        mode = v.mode;
                      }),
                    )
                  "
                  >读取</ElButton
                ><ElButton
                  :disabled="busy"
                  @click="run(() => change('set_mode', { mode }))"
                  >应用</ElButton
                >
              </div>
            </section>
            <section>
              <h3>网络偏好</h3>
              <ElCheckboxGroup v-model="networks"
                ><ElCheckbox
                  v-for="v in ['3G', '4G', '5G']"
                  :key="v"
                  :value="v"
                  >{{ v }}</ElCheckbox
                ></ElCheckboxGroup
              ><ElButton
                :disabled="busy"
                @click="run(() => change('set_network_prefer', { networks }))"
                >应用偏好</ElButton
              >
            </section>
            <section>
              <h3>SIM 卡槽</h3>
              <ElSelect v-model="sim"
                ><ElOption
                  v-for="slot in current?.manufacturer === 'tdtech'
                    ? [0, 1]
                    : [1, 2]"
                  :key="slot"
                  :value="slot"
                  :label="`卡槽 ${slot}`"
              /></ElSelect>
              <div class="workbench-actions">
                <ElButton
                  :disabled="busy"
                  @click="
                    run(() =>
                      action('get_sim_slot').then((v) => {
                        sim = v.sim_slot;
                      }),
                    )
                  "
                  >读取</ElButton
                ><ElButton
                  :disabled="busy"
                  @click="run(() => change('set_sim_slot', { slot: sim }))"
                  >切换卡槽</ElButton
                >
              </div>
            </section>
            <section>
              <h3>IMEI</h3>
              <ElInput v-model="imei" maxlength="15" placeholder="15 位 IMEI" />
              <div class="workbench-actions">
                <ElButton
                  :disabled="busy"
                  @click="
                    run(() =>
                      action('get_imei').then((v) => {
                        imei = v.imei;
                      }),
                    )
                  "
                  >读取</ElButton
                ><ElButton
                  :disabled="busy"
                  @click="run(() => change('set_imei', { imei }))"
                  >写入</ElButton
                >
              </div>
            </section>
            <section v-if="supported('set_band_lock')">
              <h3>频段锁定</h3>
              <ElSelect v-model="bandClass"
                ><ElOption
                  v-for="v in ['umts', 'lte', 'nr_nsa', 'nr']"
                  :key="v"
                  :value="v"
                  :label="v.toUpperCase()" /></ElSelect
              ><ElInput
                v-model="bands"
                placeholder="例如 1,3,8,41；留空解除限制"
              />
              <div class="workbench-actions">
                <ElButton
                  :disabled="busy"
                  @click="
                    run(async () => {
                      await action('get_band_lock');
                    })
                  "
                  >读取频段</ElButton
                ><ElButton
                  :disabled="busy"
                  @click="
                    run(() =>
                      change('set_band_lock', {
                        band_class: bandClass,
                        bands: bands.trim()
                          ? bands.split(/[ ,/]+/).map(Number)
                          : [],
                      }),
                    )
                  "
                  >应用</ElButton
                >
              </div>
            </section>
            <section>
              <h3>维护</h3>
              <div class="workbench-actions">
                <ElButton
                  :disabled="busy"
                  @click="run(() => change('soft_reboot'))"
                  >重启模组</ElButton
                ><ElButton
                  :disabled="busy"
                  @click="
                    run(async () => {
                      await api(`modems/${selected}/ports/close`, 'POST', {
                        role: 'at',
                      });
                      message = 'AT 端口已关闭，下次请求将重新打开';
                    })
                  "
                  >重开 AT 端口</ElButton
                ><ElButton
                  v-if="supported('set_5g_lan')"
                  :disabled="busy"
                  @click="run(() => change('set_5g_lan', { enabled: true }))"
                  >启用 5G LAN</ElButton
                ><ElButton
                  v-if="supported('set_5g_lan')"
                  :disabled="busy"
                  @click="run(() => change('set_5g_lan', { enabled: false }))"
                  >关闭 5G LAN</ElButton
                >
              </div>
            </section>
          </div>
        </ElTabPane>
        <ElTabPane
          v-if="supported('get_neighborcell')"
          label="邻区与锁小区"
          name="cells"
        >
          <ElButton
            :loading="busy"
            @click="
              run(async () => {
                neighbors = await action('get_neighborcell');
              })
            "
            >扫描邻区</ElButton
          >
          <template v-if="neighbors"
            ><h3>锁定状态</h3>
            <ElTable empty-text="暂无数据" :data="neighbors.locks"
              ><ElTableColumn
                v-for="key in [
                  'rat',
                  'known',
                  'locked',
                  'arfcn',
                  'pci',
                  'scs',
                  'band',
                ]"
                :key="key"
                :prop="key"
                :label="key"
                min-width="90"
            /></ElTable>
            <h3>邻区</h3>
            <ElTable empty-text="暂无数据" :data="neighbors.neighbors"
              ><ElTableColumn
                v-for="key in [
                  'relation',
                  'rat',
                  'arfcn',
                  'pci',
                  'rsrp',
                  'rsrq',
                ]"
                :key="key"
                :prop="key"
                :label="key"
                min-width="90" /></ElTable
          ></template>
          <ElForm label-position="top" class="lock-form"
            ><ElFormItem label="网络"
              ><ElSelect v-model="lock.rat"
                ><ElOption value="lte" label="LTE" /><ElOption
                  value="nr"
                  label="NR" /></ElSelect></ElFormItem
            ><ElFormItem label="ARFCN"
              ><ElInputNumber
                v-model="lock.arfcn"
                :min="0"
                :max="3279165" /></ElFormItem
            ><ElFormItem label="PCI"
              ><ElInputNumber
                v-model="lock.pci"
                :min="0"
                :max="1007" /></ElFormItem
            ><ElFormItem v-if="lock.rat === 'nr'" label="子载波间隔"
              ><ElSelect v-model="lock.scs"
                ><ElOption
                  v-for="(v, i) in [15, 30, 60, 120, 240, 480]"
                  :key="i"
                  :value="i"
                  :label="`${v} kHz`" /></ElSelect></ElFormItem
            ><ElFormItem v-if="lock.rat === 'nr'" label="NR 频段"
              ><ElInputNumber
                v-model="lock.band"
                :min="1"
                :max="1024" /></ElFormItem
          ></ElForm>
          <div class="workbench-actions">
            <ElCheckbox v-model="persistLock">开机恢复此设置</ElCheckbox
            ><ElButton
              :disabled="busy"
              type="primary"
              @click="run(() => cellLock(lock))"
              >锁定小区</ElButton
            ><ElButton :disabled="busy" @click="run(() => cellLock(null))"
              >解除锁定</ElButton
            >
          </div>
        </ElTabPane>
        <ElTabPane label="流量统计" name="usage"
          ><div class="workbench-actions">
            <ElButton
              :loading="busy"
              @click="
                run(async () => {
                  usage = await action('get_usage_stats');
                })
              "
              >读取流量</ElButton
            ><ElButton
              v-if="supported('write_usage_stats')"
              :disabled="busy"
              @click="
                run(async () => {
                  await action('write_usage_stats');
                })
              "
              >保存计数</ElButton
            ><ElButton
              v-if="supported('clear_usage_stats')"
              :disabled="busy"
              @click="run(() => change('clear_usage_stats'))"
              >清零计数</ElButton
            >
          </div>
          <ElDescriptions v-if="usage?.available" :column="2" border
            ><ElDescriptionsItem label="下载"
              >{{
                (usage.total_rx_bytes / 1024 ** 3).toFixed(3)
              }}
              GiB</ElDescriptionsItem
            ><ElDescriptionsItem label="上传"
              >{{
                (usage.total_tx_bytes / 1024 ** 3).toFixed(3)
              }}
              GiB</ElDescriptionsItem
            ></ElDescriptions
          ><ElEmpty
            v-else
            :description="usage ? '该模组未提供可用流量计数' : '点击读取流量'"
        /></ElTabPane>
        <ElTabPane label="短信" name="sms" lazy
          ><Sms :key="selected" :token="token" :modem="selected"
        /></ElTabPane>
        <ElTabPane label="开机与硬件" name="startup" lazy
          ><Startup :key="selected" :token="token" :modem="selected"
        /></ElTabPane>
        <ElTabPane label="自动维护" name="maintenance" lazy
          ><Maintenance :key="selected" :token="token" :modem="selected"
        /></ElTabPane>
        <ElTabPane label="AT 调试与日志" name="debug" lazy
          ><Debug :key="selected" :token="token" :modem="selected"
        /></ElTabPane>
      </ElTabs>
      <details v-if="result" open class="operation-result">
        <summary>操作结果</summary>
        <pre>{{ JSON.stringify(result, null, 2) }}</pre>
      </details>
    </template>
    <ElDialog v-model="editor" title="模组配置" width="min(660px,94vw)"
      ><ElForm v-if="config" label-position="top" class="config-form"
        ><ElFormItem
          v-for="[key, label] in [
            ['id', '唯一标识'],
            ['name', '显示名称'],
            ['model', '型号'],
            ['at_port', 'AT 端口'],
            ['sms_at_port', '短信端口（可留空）'],
            ['interface', '数据网卡（可留空）'],
            ['apn', 'APN'],
          ]"
          :key="key"
          :label="label"
          ><ElInput v-model="config[key]" /></ElFormItem
        ><ElFormItem label="厂商"
          ><ElSelect
            v-model="config.manufacturer"
            @change="
              () => {
                if (config?.manufacturer === 'tdtech') {
                  config.platform = 'hisilicon';
                  config.model = 'mt5700m-cn';
                }
              }
            "
            ><ElOption value="quectel" label="移远" /><ElOption
              value="tdtech"
              label="TD Tech MT5700" /></ElSelect></ElFormItem
        ><ElFormItem label="平台"
          ><ElSelect v-model="config.platform"
            ><ElOption
              v-for="v in config.manufacturer === 'tdtech'
                ? ['hisilicon']
                : ['qualcomm', 'unisoc', 'lte12', 'lte', 'hisilicon']"
              :key="v"
              :value="v"
              :label="v" /></ElSelect></ElFormItem
        ><ElFormItem label="连接总线"
          ><ElSelect v-model="config.bus"
            ><ElOption value="usb" label="USB" /><ElOption
              value="pcie"
              label="PCIe" /></ElSelect></ElFormItem
        ><ElFormItem label="PDP 索引"
          ><ElInputNumber
            v-model="config.pdp_index"
            :min="1"
            :max="16" /></ElFormItem
        ><ElFormItem label="启用"
          ><ElSwitch v-model="config.enabled" /></ElFormItem></ElForm
      ><template #footer
        ><ElButton @click="editor = false">取消</ElButton
        ><ElButton type="primary" :loading="busy" @click="run(save)"
          >保存并生效</ElButton
        ></template
      ></ElDialog
    >
  </section>
</template>
<style scoped>
.device-workbench {
  background: #fff;
  border: 1px solid #e8edf5;
  border-radius: 16px;
  padding: 24px;
  min-width: 0;
}
.workbench-bar {
  display: flex;
  justify-content: space-between;
  align-items: center;
  gap: 16px;
  margin-bottom: 20px;
  flex-wrap: wrap;
}
.workbench-bar .el-select {
  width: 260px;
}
.workbench-actions {
  display: flex;
  flex-wrap: wrap;
  gap: 10px;
  margin: 14px 0;
}
.workbench-actions .el-button {
  margin-left: 0;
}
.device-scan-card {
  padding: 20px;
  background: #f8faff;
  border: 1px solid #e8edf5;
  border-radius: 12px;
  margin-top: 18px;
}
.settings-grid {
  display: grid;
  grid-template-columns: repeat(2, minmax(0, 1fr));
  gap: 24px;
}
.settings-grid > section {
  padding: 20px;
  border: 1px solid #e8edf5;
  border-radius: 12px;
}
.settings-grid .el-input {
  margin-top: 10px;
}
.lock-form,
.config-form {
  display: grid;
  grid-template-columns: repeat(2, minmax(0, 1fr));
  gap: 0 20px;
  margin-top: 20px;
}
.el-alert {
  margin-bottom: 16px;
}
pre {
  white-space: pre-wrap;
  overflow-wrap: anywhere;
  max-height: 420px;
  overflow: auto;
  background: #f6f8fb;
  padding: 16px;
  border-radius: 10px;
  font-size: 12px;
}
summary {
  cursor: pointer;
  margin: 20px 0;
  color: #68788d;
}
h3 {
  font-size: 15px;
  margin: 20px 0;
}
p {
  font-size: 13px;
  color: #758196;
}
@media (max-width: 760px) {
  .settings-grid,
  .lock-form,
  .config-form {
    grid-template-columns: 1fr;
  }
  .device-workbench {
    padding: 14px;
  }
  .workbench-bar .el-select {
    width: 100%;
  }
}
</style>
