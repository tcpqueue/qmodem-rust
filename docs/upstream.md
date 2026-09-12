# 上游基线

## QModem

- 仓库：https://github.com/FUjr/QModem
- 关注组件：`luci/luci-app-qmodem-next` 及其实际后端依赖
- 固定提交：`86102c2a6f620264b6308bff29a92ecde6a2ea2f`
- 根许可：MPL 2.0 加禁止商业使用的额外限制，原文见 `licenses/QModem-upstream.txt`；部分组件另有 GPL v3 声明，需按文件核对
- `data/` 中的四份 JSON 从此提交复制，未修改。

LuCI Next 的页面是 JavaScript；核心厂商适配是 Shell；短信和串口守护进程包含 C。不能将其视为一个待替换的 Python 应用。

上游 `vendor/dynamic_load.json` 包含 `tdtech.sh`、`nk.sh` 映射，但此基线的 vendor 目录中缺少对应文件。迁移时单独记录这两个缺口，不能声称其已有完整上游实现。

`data/supported-models.json` 从原数据库筛选移远及 MT5700M-CN，MT5700 规范为 tdtech，并记录原 huawei 归类。其他原始 JSON 仅作为核对资料，不代表本项目支持所有厂商。

## art-design-pro

- 仓库：https://github.com/Daymychen/art-design-pro
- 固定提交：`f3aaf58eec1a0e988f162352c33862327a484f95`
- 许可：MIT，副本位于 `licenses/art-design-pro.txt`
- 作为 `web/` 子模块保留前端基线；后续管理页面在确认资源布局后接入 Rust API。

不能直接使用上游示例环境中的远程 Mock API 作为本项目生产接口。构建产物必须离线可用并嵌入服务。
