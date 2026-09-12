<script setup lang="ts">
import { ElAlert, ElButton, ElSelect, ElOption, ElFormItem, ElDescriptions, ElDescriptionsItem, ElTable, ElTableColumn } from "element-plus";
const mode = defineModel<string>({ required: true });
defineProps<{ busy: boolean; state: any }>();
defineEmits<{ refresh: [] }>();
const modes: Record<string, string> = { internal: '模组内部拨号', usb: 'USB 数据连接', ethernet: '转接板网口数据连接', ndis: '手动 NDIS 拨号（兼容模式）' };
const numbers: Record<number, string> = { 0: '模组内部', 1: 'USB 数传', 2: '转接板网口数传' };
const usb: Record<number, string> = {0:'ECM',2:'ECM 调试',4:'NCM',5:'NCM 调试',6:'RNDIS',7:'MBIM',8:'PPP'};
</script>
<template>
  <section class="balong-config">
    <h3>巴龙 MT5700 · 数据连接</h3>
    <p>选择实际的数据出口；AT 控制端口始终可以通过 USB 连接。</p>
    <ElFormItem label="模组拨号场景">
      <ElSelect v-model="mode" :disabled="busy">
        <ElOption v-for="(label, value) in modes" :key="value" :value="value" :label="label" />
      </ElSelect>
    </ElFormItem>
    <ElAlert v-if="mode === 'ndis'" type="warning" :closable="false" title="兼容模式使用 NDISDUP，请先确认模组未开启自动拨号。" />
    <ElAlert v-else type="info" :closable="false" title="使用模组自动拨号，APN 与认证通过同一条命令配置。" description="下方“自动联网”表示路由器服务启动时执行连接；连接操作会开启模组自动拨号。APN 和认证全部留空时，保留模组已有的 APN 设置。" />
    <ElButton class="read-modem" :loading="busy" @click="$emit('refresh')">读取模组当前配置</ElButton>
    <template v-if="state">
      <ElDescriptions :column="2" border>
        <ElDescriptionsItem label="模组自动拨号">{{ state.autodial ? (state.autodial.enabled ? '已开启' : '已关闭') : '无法读取' }}</ElDescriptionsItem>
        <ElDescriptionsItem label="当前数据出口">{{ numbers[state.autodial?.mode] || '未提供' }}</ElDescriptionsItem>
        <ElDescriptionsItem label="USB 模式">{{ usb[state.usb_mode] || (state.usb_mode ?? '未提供') }}</ElDescriptionsItem>
        <ElDescriptionsItem label="地址协议">{{ state.autodial?.protocol || '未提供' }}</ElDescriptionsItem>
        <ElDescriptionsItem label="当前 APN">{{ state.autodial?.apn || '运营商默认' }}</ElDescriptionsItem>
      </ElDescriptions>
      <details style="margin-top: 16px"><summary>模组 PDP 配置（{{ state.pdp_contexts?.length || 0 }} 项）</summary>
      <ElTable :data="state.pdp_contexts || []" empty-text="未读取到 PDP 配置">
        <ElTableColumn prop="cid" label="PDP" width="80" />
        <ElTableColumn prop="protocol" label="地址协议" min-width="110" />
        <ElTableColumn prop="apn" label="APN" min-width="140" />
      </ElTable></details>
      <p v-if="state.unavailable?.length">部分查询不受当前固件支持：{{ state.unavailable.join('、') }}</p>
    </template>
  </section>
</template>
<style scoped>
.balong-config { padding: 20px; border: 1px solid #dce5f1; border-radius: 12px; background: #fafcff; margin: 16px 0; }
h3 { margin: 0 0 10px; } p { color: #64748b; line-height: 1.7; } .read-modem { margin: 16px 0; }
</style>
