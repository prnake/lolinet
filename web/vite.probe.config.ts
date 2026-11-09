import react from "@vitejs/plugin-react";
import { defineConfig } from "vite";
import { baseUrl, meshNodes } from "./vite.nodes";

const JOIN = process.env.MESH_JOIN ?? baseUrl();

// 公开探针页面（准入站首页）。单独构建到 dist-probe/，不带控制台的代码和演示快照；
// join/deploy.py site 把它传到 R2 site/。开发：npm run dev:probe（/v1 代理到线上 join 站）。
export default defineConfig({
  plugins: [react(), meshNodes({ public: true })],
  server: {
    port: 5189,
    open: "/probe.html",
    proxy: { "/v1": { target: JOIN, changeOrigin: true } },
  },
  build: {
    outDir: "dist-probe",
    assetsInlineLimit: 0,
    rollupOptions: { input: "probe.html" },
  },
});
