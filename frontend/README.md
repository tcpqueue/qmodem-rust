# QModem Web

独立管理页面使用 Vue 3 与 Element Plus，并沿用 art-design-pro 的主题配置。
实际业务入口为 `src/App.vue`，目前提供 AT 队列监控，其余模组业务页面待迁移。

`src/vendor/art-el-light.scss` 来自 Daymychen/art-design-pro 提交
`f3aaf58eec1a0e988f162352c33862327a484f95` 的同名样式文件，保留原 MIT 许可证。
`web/` 子模块作为该版本的完整参考；修改本目录不会修改上游子模块。

在 WSL 的 Linux 原生目录，使用 Node.js 22.12 以上：

```sh
npm ci
npm run build
```

构建产物在 `dist/`，包含 HTML、gzip 副本和脚本 CSP 哈希，随源码提交。
Rust 编译时嵌入这三份文件。路由器只运行 qmodemd，不需要 Node.js 或单独部署静态文件。

开发时可运行 `npm run dev`，`/api` 代理到本机 8088 端口的 Rust 服务。
访问令牌仅保留在页面内存中。页面不包含示例模组或模拟任务；只有后端返回的配置和队列才会显示。
