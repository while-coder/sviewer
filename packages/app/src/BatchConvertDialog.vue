<script setup lang="ts">
/**
 * 批量转换窗口（整页，运行在独立的 Tauri 窗口 label='batch'，由 batch.ts 挂载）。
 * 左侧文件列表（拖入 / 点击空态区添加文件），右侧目标格式与质量。
 * 「转换并保存」选一次目录后逐个调用 encodeTo（单文件命令），进度逐项更新、
 * 单项失败不影响后续；重名由 Rust unique_dest 自动 -2 后缀。
 */
import { ref, computed, onMounted, onUnmounted } from 'vue'
import { getCurrentWindow } from '@tauri-apps/api/window'
import { open as openDialog } from '@tauri-apps/plugin-dialog'
import { join } from '@tauri-apps/api/path'
import { convertFileSrc } from '@tauri-apps/api/core'
import {
  encodeTo,
  uniqueDest,
  decodeThumb,
  isWebNative,
  extOf,
  humanSize,
  type SaveFormat,
} from './viewer'

const win = getCurrentWindow()

interface BatchItem {
  id: number
  path: string
  name: string
  thumb: string | null
  status: 'pending' | 'running' | 'done' | 'failed'
  message: string
}

let nextId = 1
const items = ref<BatchItem[]>([])

const FORMATS: { value: Exclude<SaveFormat, 'original'>; label: string }[] = [
  { value: 'jpeg', label: 'JPEG' },
  { value: 'png', label: 'PNG' },
  { value: 'webp', label: 'WebP（无损）' },
  { value: 'tiff', label: 'TIFF' },
  { value: 'bmp', label: 'BMP' },
  { value: 'gif', label: 'GIF（静态首帧）' },
  { value: 'ico', label: 'ICO（缩至 256）' },
  { value: 'tga', label: 'TGA' },
  { value: 'ppm', label: 'PPM' },
  { value: 'qoi', label: 'QOI' },
  { value: 'avif', label: 'AVIF（较慢）' },
  { value: 'ff', label: 'Farbfeld' },
]
const format = ref<Exclude<SaveFormat, 'original'>>('jpeg')
const quality = ref(85)

const running = ref(false)
const cancelFlag = ref(false)
const doneCount = computed(() => items.value.filter((i) => i.status === 'done').length)

/** Rust 支持的扩展名（与 lib.rs SUPPORTED_EXT 一致，svg 矢量不在转换范围）。 */
const SUPPORTED = new Set([
  'jpg', 'jpeg', 'png', 'gif', 'webp', 'bmp', 'ico', 'tiff', 'tif', 'avif', 'heic', 'heif',
])

function loadThumb(it: BatchItem) {
  if (isWebNative(it.path)) {
    it.thumb = convertFileSrc(it.path)
    return
  }
  decodeThumb(it.path, 88)
    .then((u) => (it.thumb = u))
    .catch(() => {}) // 失败降级为图标，不阻塞
}

/** 批量加入文件（拖入 / 文件对话框 / 当前文件夹共用）：过滤、去重、加载缩略图。 */
function addFiles(paths: string[]) {
  const seen = new Set(items.value.map((i) => i.path))
  for (const p of paths) {
    if (!SUPPORTED.has(extOf(p)) || seen.has(p)) continue
    seen.add(p)
    const it: BatchItem = {
      id: nextId++,
      path: p,
      name: p.split(/[\\/]/).pop() ?? p,
      thumb: null,
      status: 'pending',
      message: '',
    }
    items.value.push(it)
    loadThumb(it)
  }
}

async function addViaDialog() {
  const selected = await openDialog({ multiple: true })
  if (Array.isArray(selected)) addFiles(selected)
}

function removeItem(id: number) {
  if (running.value) return
  items.value = items.value.filter((i) => i.id !== id)
}
function clearAll() {
  if (running.value) return
  items.value = []
}

/** 目标格式 → 输出扩展名。 */
function outExt(f: Exclude<SaveFormat, 'original'>): string {
  return f === 'jpeg' ? 'jpg' : f
}

async function convertAll() {
  if (!items.value.length || running.value) return
  const dir = await openDialog({ directory: true, title: '选择保存目录' })
  if (!dir || typeof dir !== 'string') return
  running.value = true
  cancelFlag.value = false
  try {
    for (const it of items.value) {
      if (cancelFlag.value) break
      if (it.status === 'done') continue // 重跑时跳过已完成项
      it.status = 'running'
      it.message = ''
      try {
        const stem = it.name.replace(/\.[^.]+$/, '')
        const dest = await uniqueDest(await join(dir, `${stem}.${outExt(format.value)}`))
        const out = await encodeTo(
          it.path,
          dest,
          format.value,
          format.value === 'jpeg' ? quality.value : null,
          null,
        )
        it.status = 'done'
        it.message = `完成 · ${humanSize(out.size)} · ${dest.split(/[\\/]/).pop()}`
      } catch (e) {
        it.status = 'failed'
        it.message = String(e)
      }
    }
  } finally {
    running.value = false
  }
}

/** 关闭窗口（× / Escape）。转换中先置取消标志，尽快停下。 */
function closeWindow() {
  cancelFlag.value = true
  void win.close()
}

function onKey(e: KeyboardEvent) {
  if (e.key === 'Escape') closeWindow()
}

// 本窗口的文件拖入：加入列表（主窗口的拖入仍由主窗口自己打开）
let unlistenDrop: (() => void) | null = null

onMounted(async () => {
  window.addEventListener('keydown', onKey)
  unlistenDrop = await win.onDragDropEvent((e) => {
    if (e.payload.type === 'drop' && e.payload.paths.length > 0) addFiles(e.payload.paths)
  })
})

onUnmounted(() => {
  window.removeEventListener('keydown', onKey)
  unlistenDrop?.()
})
</script>

<template>
  <div class="batch-window">
    <header class="batch-titlebar">
      <span>批量转换</span>
      <button class="win-close" title="关闭 (Esc)" @click="closeWindow">×</button>
    </header>

    <div class="batch-layout">
      <!-- 左栏：文件列表（空态区可点击添加文件，随时可拖入） -->
      <div class="batch-main">
        <div v-if="items.length === 0" class="batch-empty" title="添加文件…" @click="addViaDialog">
          <p>拖入图片，或点击添加</p>
        </div>
        <ul v-else class="batch-items">
          <li v-for="it in items" :key="it.id" :class="it.status">
            <img v-if="it.thumb" :src="it.thumb" alt="" />
            <span v-else class="ph">🖼️</span>
            <div class="meta">
              <div class="name" :title="it.path">{{ it.name }}</div>
              <div class="status">
                <template v-if="it.status === 'pending'">待转换</template>
                <template v-else-if="it.status === 'running'">转换中…</template>
                <template v-else-if="it.status === 'done'">{{ it.message }}</template>
                <template v-else>{{ it.message }}</template>
              </div>
            </div>
            <button class="rm" title="移除" :disabled="running" @click="removeItem(it.id)">×</button>
          </li>
        </ul>
      </div>

      <!-- 右栏：选项 -->
      <div class="batch-side">
        <label class="bs-label">目标格式</label>
        <select v-model="format" class="bs-select">
          <option v-for="f in FORMATS" :key="f.value" :value="f.value">{{ f.label }}</option>
        </select>
        <label v-if="format === 'jpeg'" class="bs-quality">质量 {{ quality }}
          <input v-model.number="quality" type="range" min="1" max="100" />
        </label>

        <div class="bs-actions">
          <button class="ep-btn" :disabled="!items.length || running" @click="clearAll">移除全部</button>
        </div>
      </div>
    </div>

    <footer class="batch-foot">
      <span class="stat">共 {{ items.length }} 项<template v-if="doneCount"> · 已完成 {{ doneCount }}</template></span>
      <div class="foot-btns">
        <button v-if="running" class="ep-btn" @click="cancelFlag = true">取消</button>
        <button class="btn-primary" :disabled="!items.length || running" @click="convertAll">
          {{ running ? '转换中…' : '转换并保存' }}
        </button>
      </div>
    </footer>
  </div>
</template>

<style>
/* 批量转换窗口：标题栏 + 左列表右选项 + 底部操作，铺满整个窗口 */
.batch-window { display: flex; flex-direction: column; height: 100vh; }
.batch-titlebar {
  display: flex; align-items: center; justify-content: space-between;
  padding: 10px 16px; border-bottom: 1px solid var(--border); font-weight: 600;
}
.win-close {
  background: none; border: none; color: var(--fg-muted); cursor: pointer;
  font-size: 16px; line-height: 1; padding: 2px 6px; border-radius: 6px;
}
.win-close:hover { color: var(--fg); background: var(--hover); }

.batch-layout { flex: 1; min-height: 0; display: grid; grid-template-columns: 1fr 210px; }
.batch-main { overflow-y: auto; padding: 10px 6px 10px 14px; }
/* 列表滚动时不显示滚动条（拖动/hover 已能感知，滚条占位难看） */
.batch-main { scrollbar-width: none; }
.batch-main::-webkit-scrollbar { display: none; }
.batch-empty {
  height: 100%; margin: 4px 8px 4px 0;
  display: flex; align-items: center; justify-content: center;
  border: 1px dashed var(--border); border-radius: 10px;
  color: var(--fg-muted); font-size: 13px;
  cursor: pointer;
}
.batch-empty:hover { border-color: var(--primary); color: var(--fg); }
.batch-items { list-style: none; margin: 0; padding: 0; }
.batch-items li {
  display: flex; align-items: center; gap: 10px;
  padding: 4px 6px; border-radius: 8px; margin-bottom: 2px;
}
.batch-items li:hover { background: var(--hover); }
.batch-items img, .batch-items .ph {
  width: 44px; height: 44px; flex-shrink: 0;
  border-radius: 6px; object-fit: cover;
  background: var(--hover); display: flex; align-items: center; justify-content: center;
  font-size: 18px;
}
.batch-items .meta { flex: 1; min-width: 0; }
.batch-items .name { font-size: 12px; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.batch-items .status { font-size: 11px; color: var(--fg-muted); overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.batch-items li.running .status { color: var(--primary); }
.batch-items li.done .status { color: #4caf7d; }
.batch-items li.failed .status { color: #cf6679; }
.batch-items .rm {
  background: none; border: none; color: var(--fg-muted); cursor: pointer;
  font-size: 14px; padding: 2px 6px; border-radius: 6px; opacity: 0;
}
.batch-items li:hover .rm { opacity: 1; }
.batch-items .rm:hover { color: var(--fg); background: var(--hover-strong); }
.batch-items .rm:disabled { opacity: 0.2 !important; cursor: default; }

.batch-side { border-left: 1px solid var(--border); padding: 14px; display: flex; flex-direction: column; gap: 8px; }
.bs-label { font-size: 12px; color: var(--fg-muted); }
.bs-select {
  width: 100%; background: none; border: 1px solid var(--border); border-radius: 6px;
  color: var(--fg); font-size: 12px; padding: 4px 6px;
}
.bs-select option { background: var(--bar); color: var(--fg); }
.bs-quality { font-size: 12px; color: var(--fg-muted); }
.bs-quality input { width: 100%; margin-top: 4px; accent-color: var(--primary); }
.bs-actions { display: flex; flex-direction: column; gap: 6px; margin-top: auto; }
.bs-actions .ep-btn { width: 100%; padding: 5px 0; }

.batch-foot {
  display: flex; align-items: center; justify-content: space-between;
  padding: 10px 16px 14px; border-top: 1px solid var(--border);
}
.batch-foot .stat { font-size: 12px; color: var(--fg-muted); }
.foot-btns { display: flex; align-items: center; gap: 8px; }
</style>
