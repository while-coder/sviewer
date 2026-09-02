import { defineConfig } from 'vite'
import vue from '@vitejs/plugin-vue'

// 端口 8386 = "S"(83) + "V"(86) 的 ASCII，避开 sbox 的 1421，方便两个项目同时 dev
export default defineConfig({
  plugins: [vue()],
  clearScreen: false,
  server: {
    port: 8386,
    strictPort: true,
  },
  envPrefix: ['VITE_', 'TAURI_'],
  build: {
    target: 'esnext',
    minify: !process.env.TAURI_DEBUG ? 'esbuild' : false,
    sourcemap: !!process.env.TAURI_DEBUG,
    // 多页入口：index.html = 主查看窗口，batch.html = 批量转换，edit.html = 编辑窗口
    rollupOptions: {
      input: {
        main: 'index.html',
        batch: 'batch.html',
        edit: 'edit.html',
      },
    },
  },
})
