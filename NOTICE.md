# 第三方代码与资料

本仓库原创 Rust 模块、LuCI 控制代码和文档使用 MIT 许可，明确标注其他许可的文件除外。这个许可不覆盖第三方代码、移植模块和资料，也不改变它们原有的使用条件。

`crates/qmodemd/src/vendor.rs` 及 `crates/qmodemd/src/vendor/` 将上游 quectel.sh 和 huawei.sh 中的 AT 行为移植为 Rust，保留原作者署名，按其 GPL v3 声明标注 GPL-3.0-only；全文见 `licenses/GPL-3.0.txt`。最终组合程序需要同时遵守适用组件的许可，不能因原创模块采用 MIT 就重新许可第三方内容。

`data/` 的原始和筛选 JSON，以及 `tests/fixtures/` 中保留的上游串口录制资料 来自 FUjr/QModem，保留该上游提交的根许可证原文，见 `data/LICENSE` 和 `licenses/QModem-upstream.txt`。其内容为 MPL 2.0 并附有禁止商业使用的额外限制；不能将这些资料标成纯 MIT 或纯 GPL。上游部分组件的 Makefile 和源码头另有 GPL v3 声明，后续如移植这些组件，需要逐文件保留并核对相应许可。

`web/` 是 Daymychen/art-design-pro 的源码子模块，使用 MIT 许可，保留其作者署名，副本见 `licenses/art-design-pro.txt`。`frontend/src/vendor/art-el-light.scss` 从该固定版本复制 Element Plus 主题配置，并在同目录保留 MIT LICENSE；`frontend/` 中的业务页面由本仓库维护。

内嵌前端的生产依赖许可声明见 `licenses/frontend-dependencies.txt`，服务也通过 `/licenses` 提供前端声明。
