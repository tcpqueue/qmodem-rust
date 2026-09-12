import { defineConfig } from "vite";
import vue from "@vitejs/plugin-vue";
import { viteSingleFile } from "vite-plugin-singlefile";
export default defineConfig({
  plugins: [vue(), viteSingleFile()],
  server: { proxy: { "/api": "http://127.0.0.1:8088" } },
  build: { target: "es2022", cssCodeSplit: false, sourcemap: false },
});
