import react from "@vitejs/plugin-react";
import { defineConfig } from "vite";
import { meshNodes } from "./vite.nodes";

const AGENT = process.env.MESH_AGENT ?? "http://127.0.0.1:10188";

// 本地控制台（agent 托管）。公开探针页面是另一份构建：vite.probe.config.ts。
export default defineConfig({
  plugins: [react(), meshNodes()],
  server: {
    port: 5188,
    proxy: { "/api": { target: AGENT, changeOrigin: true } },
  },
  build: { outDir: "dist", assetsInlineLimit: 0 },
});
