<script setup lang="ts">
import { ref, shallowRef, reactive, computed, nextTick, watch, watchEffect, onMounted, onUnmounted } from 'vue'
import { listen, emit } from '@tauri-apps/api/event'
import { getCurrentWindow } from '@tauri-apps/api/window'
import { WebviewWindow } from '@tauri-apps/api/webviewWindow'
import { getVersion } from '@tauri-apps/api/app'
import { convertFileSrc } from '@tauri-apps/api/core'
import { open as openDialog } from '@tauri-apps/plugin-dialog'
import {
  listSiblings,
  readImageInfo,
  getLaunchFile,
  saveEditsTo,
} from '../../lib/bridge'
import { resolveImage, preloadImage } from '../../lib/decode'
import { extOf, OPEN_FILTERS } from '../../lib/formats'
import { saveAsViaDialog } from '../../lib/save'
import { useImageView } from '../../composables/use-image-view'
import { useSaveability } from '../../composables/use-saveability'
import { useThemeSync } from '../../composables/use-theme-sync'
import { useAppMenu, type AppMenuAction } from '../../composables/use-app-menu'
import { humanSize, openExternal } from '../../lib/util'
import { exifLabel, pickCommonInfo, parseGpsCoord } from '../../lib/exif'
import { mapLinks, reverseGeocode } from '../../lib/geo'
import type { SaveFormat, ImageEdits, ImageInfo } from '../../lib/types'
import { settings } from '../../lib/settings'
import { UpdaterDialog, useTauriUpdater } from '@while-coder/tauri-updater-vue'
import SettingsDialog from './SettingsDialog.vue'

// ── 状态 ───────────────────────────────────────────────
const currentPath = ref<string | null>(null)
const imgSrc = ref<string>('')
// HEIC 解码出的位图（与 imgSrc 二选一显示）；shallowRef 避免 ImageBitmap 被 proxy 包裹
const bitmap = shallowRef<ImageBitmap | null>(null)
const canvasEl = ref<HTMLCanvasElement | null>(null)
const siblings = ref<string[]>([])
const info = ref<ImageInfo | null>(null)
const loadError = ref<string>('')
// 信息面板开关：映射到 settings.showInfo，改动即持久化（设置里记住勾选状态）
const showInfo = computed({
  get: () => settings.showInfo,
  set: (v) => (settings.showInfo = v),
})
const loading = ref(false)
// 关于 / 设置 弹窗（null = 关闭）
const modal = ref<'settings' | null>(null)
// 设置弹窗初始分类页（打开后页内切换由 SettingsDialog 自己维护）
type SettingsTab = 'general' | 'view' | 'assoc' | 'about'
const settingsTab = ref<SettingsTab>('general')
/** 打开设置弹窗并切到「关于」页（菜单/右键的「关于」入口）。 */
function openAbout() {
  settingsTab.value = 'about'
  modal.value = 'settings'
}
const appVersion = ref('')

// ── 应用内更新：UpdaterDialog 自动检查/下载/安装；「关于」页展示状态与手动检查 ──
// updater 只在这里创建一次，经 SettingsDialog 下传 AboutPage。
const updater = useTauriUpdater()

// ── 批量转换：独立窗口（只开一个，重复触发聚焦已有窗口）─────
// 批量窗口是单独的 WebviewWindow（label='batch'，入口 batch.html），
// 转换与文件列表都在那边，主窗口继续看图互不干扰。
async function openBatch() {
  // getByLabel 是 async（返回 Promise），不 await 会恒为 truthy，永远走「已存在」分支
  const existing = await WebviewWindow.getByLabel('batch')
  if (existing) {
    await existing.show()
    await existing.setFocus()
    return
  }
  new WebviewWindow('batch', {
    title: '批量转换 - 素阅',
    url: 'batch.html',
    width: 760,
    height: 540,
    minWidth: 560,
    minHeight: 380,
    resizable: true,
  })
    .once('tauri://error', (e) => {
      console.error('打开批量转换窗口失败', e)
      window.alert(`打开批量转换窗口失败：${e}`)
    })
}

// ── 编辑窗口：独立窗口（只开一个，重复触发聚焦并切图）─────
// 编辑窗口是单独的 WebviewWindow（label='edit'，入口 edit.html），
// 裁剪 / 标记 / 改尺寸都在那边做，保存后发 image-edited 事件让主窗口刷新。
async function openEdit() {
  const p = currentPath.value
  if (!p) return
  // getByLabel 是 async（返回 Promise），不 await 会恒为 truthy，永远走「已存在」分支
  const existing = await WebviewWindow.getByLabel('edit')
  if (existing) {
    await existing.show()
    await existing.setFocus()
    await emit('edit-file', p)
    return
  }
  new WebviewWindow('edit', {
    title: '编辑 - 素阅',
    url: `edit.html?path=${encodeURIComponent(p)}`,
    width: 980,
    height: 660,
    minWidth: 760,
    minHeight: 500,
    resizable: true,
  })
    .once('tauri://error', (e) => {
      console.error('打开编辑窗口失败', e)
      window.alert(`打开编辑窗口失败：${e}`)
    })
}

// ── 编辑：旋转 / 镜像（快捷操作；裁剪/标记/改尺寸在编辑窗口）──
const edit = reactive({ rotation: 0, flip: false })
// 输出偏好（另存为 JPEG 时的质量）
const editOutput = reactive({ format: 'original' as SaveFormat, quality: 85 })

const modified = computed(() => edit.rotation !== 0 || edit.flip)

function rotate() {
  if (!natural.w) return
  edit.rotation = (edit.rotation + 90) % 360
  if (view.fit) fitView() // 适应模式下旋转后重算缩放
}
function mirror() {
  if (!natural.w) return
  edit.flip = !edit.flip
}

/** 当前编辑 → Rust ImageEdits。 */
function editsFromState(): ImageEdits {
  return {
    rotation: edit.rotation,
    flip: edit.flip,
    crop: null,
    resize: null,
    quality: editOutput.format === 'jpeg' ? editOutput.quality : null,
  }
}

function resetEditState() {
  edit.rotation = 0
  edit.flip = false
  editOutput.format = 'original'
  editOutput.quality = 85
}

/** 「保存到原图」的可用性与提示（与编辑窗口共用）。 */
const { canSaveEdits, saveBtnTitle } = useSaveability({
  path: currentPath,
  modified: () => modified.value,
  outputFormat: () => editOutput.format,
  noEditHint: '保存（先旋转/镜像，更多编辑请右键打开编辑窗口）',
})

const savingEdits = ref(false)
async function saveEdits() {
  const p = currentPath.value
  if (!p || !canSaveEdits.value || savingEdits.value) return
  savingEdits.value = true
  try {
    await saveEditsTo(p, editsFromState())
  } catch (e) {
    console.error('保存修改失败', e)
    window.alert(`保存失败：${e}`)
    return
  } finally {
    savingEdits.value = false
  }
  resetEditState()
  // 加时间戳绕过 WebView 对 asset URL 的缓存；可编辑格式都是 web 原生，无需重新解码
  reloadCurrent()
}

/** 重新加载当前图片（编辑窗口保存后也走这里）。 */
function reloadCurrent() {
  const p = currentPath.value
  if (!p) return
  const seq = ++loadSeq
  imgSrc.value = `${convertFileSrc(p)}?v=${Date.now()}`
  readImageInfo(p)
    .then((i) => {
      if (seq === loadSeq) info.value = i
    })
    .catch(() => {})
}

const index = computed(() => (currentPath.value ? siblings.value.indexOf(currentPath.value) : -1))
// 详情抽屉（完整 EXIF）
const showDetail = ref(false)
const commonInfo = computed(() => (info.value ? pickCommonInfo(info.value.exif) : []))
// GPS 经纬度（十进制，WGS-84）；无 GPS 字段时为 null，详情抽屉不显示「位置」区块
const gps = computed(() => (info.value ? parseGpsCoord(info.value.exif) : null))
// GPS → 地名（逆地理编码）：打开详情抽屉且有 GPS 时才发请求，切换图片自动失效。
// seq 防竞态：慢请求回来时若已切到别的图/别的坐标就丢弃。
const geoName = ref<string | null>(null)
const geoLoading = ref(false)
let geoSeq = 0
watch(
  () => (showDetail.value ? gps.value : null),
  (g) => {
    geoName.value = null
    if (!g) return
    const seq = ++geoSeq
    geoLoading.value = true
    void reverseGeocode(g.lat, g.lng).then((addr) => {
      if (seq !== geoSeq) return
      geoName.value = addr
      geoLoading.value = false
    })
  },
)
// 简略浮层只放前几条，保证面板不滚动；完整列表进「更多详情」抽屉
const briefInfo = computed(() => commonInfo.value.slice(0, 6))
const counter = computed(() =>
  index.value >= 0 ? `${index.value + 1}/${siblings.value.length}` : '',
)
// 窗口标题：文件名 · 计数 · 尺寸 · 大小 · 缩放（顶栏已省掉，信息全放标题栏）
const title = computed(() => {
  if (!info.value) return '素阅 SViewer'
  const parts = [info.value.fileName]
  if (counter.value) parts.push(counter.value)
  if (info.value.width > 0) parts.push(`${info.value.width} × ${info.value.height}`)
  parts.push(humanSize(info.value.size))
  parts.push(`${Math.round(view.scale * 100)}%`)
  return parts.join('  ')
})
watchEffect(() => {
  getCurrentWindow().setTitle(title.value).catch((e) => console.warn('设置标题失败', e))
})

// ── 平台判定（快捷键提示 / Esc 行为 / 设置页 tab 显隐共用）──
const isMac = /Mac/i.test(navigator.platform)

// ── 系统菜单（窗口菜单栏）→ 映射到本地函数 ──────────────
const menuActions: Record<AppMenuAction, () => void> = {
  'open-file': () => void pickFile(),
  'save-as': () => void saveAs(),
  'batch-convert': () => void openBatch(),
  fit: () => fitView(),
  'actual-size': () => actualSize(),
  'toggle-info': () => (showInfo.value = !showInfo.value),
  settings: () => (modal.value = 'settings'),
  about: openAbout,
}
useAppMenu((a) => menuActions[a]())

// ── 主题：实际主题应用到 <html data-theme>，CSS 变量随之切换 ──
useThemeSync()

// ── 另存为（对话框里选「保存类型」即可顺带转换格式）──────
// 菜单加速键（Cmd/Ctrl+S）与 webview 快捷键在部分平台会同时命中：
// 加重入锁防止连开两个保存对话框。
let saveAsBusy = false
async function saveAs() {
  if (saveAsBusy) return
  const p = currentPath.value
  if (!p) return
  saveAsBusy = true
  try {
    await saveAsViaDialog({
      path: p,
      fileName: info.value?.fileName || '',
      outputFormat: editOutput.format,
      quality: editOutput.quality,
      modified: modified.value,
      edits: editsFromState(),
    })
  } finally {
    saveAsBusy = false
  }
}

// ── 右键菜单 ───────────────────────────────────────────
const ctx = reactive({ show: false, x: 0, y: 0 })
const ctxEl = ref<HTMLElement | null>(null)

function onContextMenu(e: MouseEvent) {
  e.preventDefault()
  // 弹窗打开时不弹右键菜单（否则菜单会穿透浮在弹窗下层）
  if (modal.value) return
  ctx.x = e.clientX
  ctx.y = e.clientY
  ctx.show = true
  // 渲染后测量实际尺寸，贴边时向内收
  void nextTick(() => {
    const el = ctxEl.value
    if (!el) return
    const r = el.getBoundingClientRect()
    if (ctx.x + r.width > window.innerWidth - 4) ctx.x = Math.max(4, window.innerWidth - r.width - 4)
    if (ctx.y + r.height > window.innerHeight - 4) ctx.y = Math.max(4, window.innerHeight - r.height - 4)
  })
}

function ctxAct(fn: () => void) {
  ctx.show = false
  fn()
}

/** 窗口尺寸变化时收起右键菜单（原生菜单同理）。 */
function onWinResize() {
  ctx.show = false
}

// ── 打开图片 ───────────────────────────────────────────
// 请求序号：HEIC 解码慢，快速切换时只让最新一次请求的结果生效
let loadSeq = 0

/** 空闲时预载当前图的左右邻居，翻页直接命中缓存出图。
 *  放空闲回调：避免和当前图的解码/编码抢 CPU，拖慢本张显示。 */
function preloadNeighbors() {
  const idle = (fn: () => void) =>
    'requestIdleCallback' in window ? requestIdleCallback(fn, { timeout: 2000 }) : setTimeout(fn, 300)
  idle(() => {
    const i = siblings.value.indexOf(currentPath.value ?? '')
    if (i < 0) return
    for (const p of [siblings.value[i - 1], siblings.value[i + 1]]) {
      if (p) preloadImage(p)
    }
  })
}

async function openPath(path: string, loadSiblings = true) {
  const seq = ++loadSeq
  loading.value = true
  loadError.value = ''
  currentPath.value = path
  // 清掉上一张的固有尺寸：新图加载完（onImgLoad）再按适应窗口重算
  natural.w = 0
  natural.h = 0
  // 上一张的编辑不带到下一张
  edit.rotation = 0
  edit.flip = false
  resetView()
  try {
    const result = await resolveImage(path)
    if (seq !== loadSeq) return // 已被后续请求取代，丢弃
    if (result.kind === 'bitmap') {
      // HEIC：位图交 <canvas> 绘制（见 bitmap 的 watch），不走 <img>
      imgSrc.value = ''
      bitmap.value = result.bitmap
      applyNaturalSize(result.bitmap.width, result.bitmap.height)
    } else {
      bitmap.value = null
      imgSrc.value = result.src
    }
  } catch (e) {
    if (seq !== loadSeq) return
    loadError.value = String(e)
    imgSrc.value = ''
    bitmap.value = null
    console.error('加载图片失败', path, e)
  }
  if (seq !== loadSeq) return
  loading.value = false
  // 元信息与同目录列表并行加载，失败不阻断显示
  readImageInfo(path)
    .then((i) => {
      if (seq === loadSeq) info.value = i
    })
    .catch((e) => console.warn('读取信息失败', e))
  if (loadSiblings) {
    listSiblings(path)
      .then((list) => {
        if (seq !== loadSeq) return
        siblings.value = list
        preloadNeighbors()
      })
      .catch((e) => console.warn('读取目录失败', e))
  } else {
    // 同目录切换：列表已就绪，直接排预载
    preloadNeighbors()
  }
}

async function pickFile() {
  const selected = await openDialog({
    multiple: false,
    filters: OPEN_FILTERS,
  })
  if (typeof selected === 'string') await openPath(selected)
}

function step(delta: number) {
  if (index.value < 0) return
  const next = index.value + delta
  if (next >= 0 && next < siblings.value.length) {
    void openPath(siblings.value[next], false)
  }
}

// ── 视图变换（与编辑窗口共用 useImageView）──────────────
const stageEl = ref<HTMLElement | null>(null)
// 本次按下序列开始时是否处于「适应窗口」：拖拽会解除 fit，双击判断仍以按下前为准
let fitAtDown = false
const {
  view,
  natural,
  drag,
  fitView,
  resetView,
  setScale,
  actualSize,
  toggleFit,
  onWheel,
  onPointerDown,
  onPointerMove,
  onPointerUp,
  applyNaturalSize,
  imgStyle: baseImgStyle,
} = useImageView({
  stageEl,
  rotation: () => edit.rotation,
  flip: () => edit.flip,
  defaultView: () => settings.defaultView,
  onDragStart: () => {
    fitAtDown = view.fit
  },
})

// 画布双击：适应 ↔ 1:1
function onStageDblClick() {
  if (fitAtDown) actualSize()
  else fitView()
  fitAtDown = false
}

// 主窗口光标绑定在图片上（编辑窗口绑在 stage 上随工具切换）
const imgStyle = computed(() => {
  if (!natural.w || !natural.h) return baseImgStyle.value
  return { ...baseImgStyle.value, cursor: drag.on ? 'grabbing' : 'grab' }
})

function onImgLoad(e: Event) {
  const img = e.target as HTMLImageElement
  applyNaturalSize(img.naturalWidth, img.naturalHeight)
}

// HEIC 位图画进 canvas。nextTick 等 <canvas> 挂载；先设 width/height 再画
// （赋值即清空旧内容）。位图即使之后被 LRU close，已画上的内容也不受影响。
watch(bitmap, async (b) => {
  if (!b) return
  await nextTick()
  const el = canvasEl.value
  if (!el) return
  el.width = b.width
  el.height = b.height
  el.getContext('2d')!.drawImage(b, 0, 0)
})

// 全屏
const fullscreen = ref(false)
async function toggleFullscreen() {
  const win = getCurrentWindow()
  fullscreen.value = !(await win.isFullscreen())
  await win.setFullscreen(fullscreen.value)
}

// ── 键盘 ───────────────────────────────────────────────
function onKey(e: KeyboardEvent) {
  switch (e.key) {
    case 'ArrowLeft': step(-1); break
    case 'ArrowRight': step(1); break
    case 'Escape':
      // 逐层关闭浮层；无浮层时按设置最小化或退出程序。
      // macOS 惯例：Esc 只关浮层，不最小化 / 退出窗口。
      if (ctx.show) ctx.show = false
      else if (modal.value) modal.value = null
      else if (showDetail.value) showDetail.value = false
      else if (!isMac) {
        if (settings.escClose) void getCurrentWindow().close()
        else void getCurrentWindow().minimize()
      }
      break
    case 's': case 'S':
      if (e.ctrlKey || e.metaKey) {
        e.preventDefault()
        void saveAs()
      }
      break
    case '+': case '=': setScale(view.scale * 1.2); break
    case '-': setScale(view.scale / 1.2); break
    case '0': resetView(); break
    case '1': actualSize(); break
    case 'i': case 'I': showInfo.value = !showInfo.value; break
    case 'o': case 'O': void pickFile(); break
    case 'r': case 'R': rotate(); break
    case 'm': case 'M': mirror(); break
    case 'F11': void toggleFullscreen(); break
  }
}

// ── 拖拽文件到窗口 ─────────────────────────────────────
let unlistenDrop: (() => void) | null = null
let unlistenOpen: (() => void) | null = null
let unlistenFocus: (() => void) | null = null
let unlistenEdited: (() => void) | null = null

onMounted(async () => {
  window.addEventListener('keydown', onKey)

  // 右键菜单模拟原生行为：窗口失焦即关闭（切窗口 / Alt-Tab / 点别的窗口）
  void getCurrentWindow()
    .onFocusChanged(({ payload: focused }) => {
      if (!focused) ctx.show = false
    })
    .then((un) => (unlistenFocus = un))
  // 窗口尺寸变化时同样收起，避免菜单悬在错位的位置
  window.addEventListener('resize', onWinResize)

  // 关于弹窗里展示版本号
  getVersion().then((v) => (appVersion.value = v)).catch(() => {})

  // 启动时的待打开文件（双击关联 / 命令行）
  try {
    const launch = await getLaunchFile()
    if (launch) await openPath(launch)
  } catch (e) {
    console.warn('获取启动文件失败', e)
  }

  // 第二实例：双击另一张图 → Rust emit open-file
  unlistenOpen = await listen<string>('open-file', (e) => {
    if (e.payload) void openPath(e.payload)
  })

  // 拖拽文件进窗口：打开查看（批量窗口有自己的拖入处理）
  unlistenDrop = await getCurrentWindow().onDragDropEvent((e) => {
    if (e.payload.type === 'drop' && e.payload.paths.length > 0) {
      void openPath(e.payload.paths[0])
    }
  })

  // 编辑窗口保存到原图后刷新当前显示（带缓存穿透，见 reloadCurrent）
  unlistenEdited = await listen<string>('image-edited', (e) => {
    if (e.payload && e.payload === currentPath.value) reloadCurrent()
  })
})

onUnmounted(() => {
  window.removeEventListener('keydown', onKey)
  window.removeEventListener('resize', onWinResize)
  unlistenDrop?.()
  unlistenOpen?.()
  unlistenFocus?.()
  unlistenEdited?.()
})
</script>

<template>
  <div class="viewer" @contextmenu.prevent="onContextMenu">
    <!-- 画布（文件名等信息在窗口标题栏，顶栏已省掉） -->
    <main
      ref="stageEl"
      class="stage"
      :class="{ plain: !settings.checkerboard }"
      @wheel="onWheel"
      @pointerdown="onPointerDown"
      @pointermove="onPointerMove"
      @pointerup="onPointerUp"
      @pointercancel="onPointerUp"
      @dblclick="onStageDblClick"
    >
      <template v-if="imgSrc">
        <img :src="imgSrc" :style="imgStyle" class="pic" :class="{ outline: settings.outline }" draggable="false" alt="" @load="onImgLoad" />
      </template>
      <!-- HEIC 位图直显：canvas 的 CSS 变换与 <img> 完全同构（见 imgStyle） -->
      <canvas v-else-if="bitmap" ref="canvasEl" class="pic" :class="{ outline: settings.outline }" :style="imgStyle" />
      <div v-else-if="loadError" class="empty error">
        <p>无法显示该图片</p>
        <pre>{{ loadError }}</pre>
      </div>
      <div v-else class="empty">
        <p>📂 拖入图片，或点下方打开</p>
        <p class="hint">← → 切换 · 滚轮缩放 · 双击适应/原始 · I 信息</p>
      </div>
    </main>

    <!-- 底部工具栏：打开 / 缩放 / 切换 / 视图，全屏固定最右 -->
    <footer class="dock">
      <div class="dock-center">
        <button class="tool" title="打开 (O)" @click="pickFile">📂</button>
        <span class="sep" />
        <button class="tool" title="缩小 (-)" @click="setScale(view.scale / 1.2)">−</button>
        <button class="tool" title="放大 (+)" @click="setScale(view.scale * 1.2)">＋</button>
        <span class="sep" />
        <button class="tool" title="上一张 (←)" :disabled="index <= 0" @click="step(-1)">‹</button>
        <button class="tool" title="下一张 (→)" :disabled="index < 0 || index >= siblings.length - 1" @click="step(1)">›</button>
        <span class="sep" />
        <button class="tool" :class="{ active: edit.rotation !== 0 }" title="旋转 (R)" :disabled="!natural.w" @click="rotate">↻</button>
        <button class="tool" :class="{ active: edit.flip }" title="镜像 (M)" :disabled="!natural.w" @click="mirror">⇋</button>
        <button class="tool" :title="saveBtnTitle" :disabled="!canSaveEdits || savingEdits" @click="saveEdits">💾</button>
        <span class="sep" />
        <button class="tool" title="适应/原始 (0/1)" @click="toggleFit">⤢</button>
        <button class="tool" :class="{ active: showInfo }" title="信息 (I)" @click="showInfo = !showInfo">ⓘ</button>
      </div>
      <button class="tool corner" title="全屏 (F11)" @click="toggleFullscreen">⛶</button>
    </footer>

    <!-- 信息浮层：左上角，只放常用信息（「更多详情」展开完整 EXIF 抽屉） -->
    <aside v-if="showInfo && info" class="info">
      <header class="info-head">
        <span class="name">{{ info.fileName }}</span>
        <span v-if="counter" class="pill">{{ counter }}</span>
      </header>
      <div class="stats">
        <div class="stat"><span class="k">大小</span><span class="v">{{ humanSize(info.size) }}</span></div>
        <div class="stat"><span class="k">尺寸</span><span class="v">{{ info.width }} × {{ info.height }}</span></div>
        <div class="stat"><span class="k">格式</span><span class="v">{{ info.format }}</span></div>
      </div>
      <dl v-if="briefInfo.length > 0" class="rows">
        <div v-for="c in commonInfo" :key="c.label"><dt>{{ c.label }}</dt><dd>{{ c.value }}</dd></div>
      </dl>
      <button v-if="info.exif.length > 0" class="more" @click="showDetail = true">
        更多详情<span class="pill">{{ info.exif.length }}</span>
      </button>
    </aside>

    <!-- 详情抽屉：完整 EXIF，左上角滑出的半透明面板 -->
    <transition name="slide">
      <aside v-if="showDetail && info" class="detail">
        <header>
          <span>图片详情</span>
          <button class="close" title="关闭" @click="showDetail = false">×</button>
        </header>
        <p class="file-name">{{ info.fileName }}</p>
        <h4>基本信息</h4>
        <div class="stats">
          <div class="stat"><span class="k">大小</span><span class="v">{{ humanSize(info.size) }}</span></div>
          <div class="stat"><span class="k">尺寸</span><span class="v">{{ info.width }} × {{ info.height }}</span></div>
          <div class="stat"><span class="k">格式</span><span class="v">{{ info.format }}</span></div>
        </div>
        <template v-if="gps">
          <h4>位置</h4>
          <p class="gps-coord">{{ gps.lat.toFixed(6) }}, {{ gps.lng.toFixed(6) }}</p>
          <p v-if="geoName" class="geo-name">{{ geoName }}</p>
          <p v-else-if="geoLoading" class="geo-name pending">地名解析中…</p>
          <div class="map-links">
            <button v-for="m in mapLinks(gps.lat, gps.lng)" :key="m.name" class="map-btn" @click="openExternal(m.url)">
              {{ m.name }}
            </button>
          </div>
        </template>
        <h4>EXIF <span class="pill">{{ info.exif.length }}</span></h4>
        <dl v-if="info.exif.length > 0">
          <div v-for="ex in info.exif" :key="ex.tag">
            <dt>{{ exifLabel(ex.tag) }}</dt><dd>{{ ex.value }}</dd>
          </div>
        </dl>
        <p v-else class="none">此图片没有 EXIF 信息</p>
      </aside>
    </transition>

    <!-- 右键菜单：透明遮罩负责点击关闭，菜单浮在其上 -->
    <div v-if="ctx.show" class="ctx-backdrop" @mousedown="ctx.show = false" @contextmenu.prevent="ctx.show = false">
      <nav ref="ctxEl" class="ctx" :style="{ left: ctx.x + 'px', top: ctx.y + 'px' }" @mousedown.stop>
        <button class="ctx-item" @click="ctxAct(() => pickFile())">打开…<span class="k">O</span></button>
        <button class="ctx-item" :disabled="!currentPath" @click="ctxAct(() => saveAs())">另存为…<span class="k">{{ isMac ? '⌘S' : 'Ctrl+S' }}</span></button>
        <button class="ctx-item" :disabled="!currentPath" @click="ctxAct(() => void openEdit())">编辑…</button>
        <button class="ctx-item" @click="ctxAct(() => void openBatch())">批量转换…</button>
        <div class="ctx-sep" />
        <button class="ctx-item" :disabled="index <= 0" @click="ctxAct(() => step(-1))">上一张<span class="k">←</span></button>
        <button class="ctx-item" :disabled="index < 0 || index >= siblings.length - 1" @click="ctxAct(() => step(1))">下一张<span class="k">→</span></button>
        <div class="ctx-sep" />
        <button class="ctx-item" :disabled="!currentPath" @click="ctxAct(() => fitView())">适应窗口<span class="k">0</span></button>
        <button class="ctx-item" :disabled="!currentPath" @click="ctxAct(() => actualSize())">实际大小<span class="k">1</span></button>
        <button class="ctx-item" @click="ctxAct(() => toggleFullscreen())">全屏<span class="k">F11</span></button>
        <div class="ctx-sep" />
        <button class="ctx-item" @click="ctxAct(() => (modal = 'settings'))">设置…</button>
        <button class="ctx-item" @click="ctxAct(openAbout)">关于素阅</button>
      </nav>
    </div>

    <!-- 设置弹窗：分类页定位见 settingsTab；打开时拉取格式关联状态在弹窗内部处理 -->
    <SettingsDialog
      :open="modal === 'settings'"
      :initial-tab="settingsTab"
      :app-version="appVersion"
      :updater="updater"
      @close="modal = null"
      @tab="settingsTab = $event"
    />

    <!-- 应用内更新对话框：启动时自动检查更新，下载/安装/重启一体 -->
    <UpdaterDialog />
  </div>
</template>

<style>
/* 主题变量与基础样式在 common.css（两个窗口共用） */

.viewer { display: flex; flex-direction: column; height: 100%; position: relative; }
.tool {
  background: none; border: none; color: var(--fg); cursor: pointer;
  font-size: 16px; line-height: 1; padding: 4px 8px; border-radius: 6px;
}
.tool:hover { background: var(--hover); }
.tool.active { color: var(--primary); }
.tool:disabled { opacity: 0.35; cursor: default; }
.tool:disabled:hover { background: none; }

/* 底部工具栏：中间操作组居中，全屏按钮贴右 */
.dock {
  display: flex; align-items: center;
  padding: 4px 10px;
  background: var(--bar);
  border-top: 1px solid var(--border);
  z-index: 2;
}
.dock-center { display: flex; align-items: center; gap: 2px; margin: 0 auto; }
.dock .sep { width: 1px; height: 16px; background: var(--border); margin: 0 6px; }

.stage {
  flex: 1; position: relative; overflow: hidden;
  display: flex; align-items: center; justify-content: center;
  touch-action: none; /* 触屏拖图由 Pointer Events 接管，不让浏览器处理手势 */
  background:
    repeating-conic-gradient(var(--check-a) 0% 25%, var(--check-b) 0% 50%) 50% / 24px 24px;
}
/* 棋盘格关闭：纯色背景 */
.stage.plain { background: var(--bg); }
.pic { display: block; will-change: transform; }
/* 图片边缘轮廓：outline 不占布局，跟随 transform 缩放，透明图也能看清边界 */
.pic.outline { outline: 1px solid var(--primary); }

.empty { color: var(--fg-muted); text-align: center; }
.empty .hint { font-size: 12px; opacity: 0.7; margin-top: 8px; }
.empty.error pre { color: #cf6679; white-space: pre-wrap; max-width: 70vw; }

/* ── 信息浮层 / 详情抽屉：统一的暗色玻璃面板，主题蓝点缀 ──
   面板恒为暗色（深浅主题一致，压住任何背景的图片），内部变量整体切到亮色；
   主题色只做点缀：计数胶囊、分区标题竖条、主按钮。 */
.info, .detail {
  --fg: #fff;
  --fg-muted: rgba(255, 255, 255, 0.55);
  --border: rgba(255, 255, 255, 0.12);
  --row: rgba(255, 255, 255, 0.05);
  --row-hover: rgba(255, 255, 255, 0.1);
  --accent: var(--primary);
  --accent-soft: color-mix(in srgb, var(--primary) 22%, transparent);
  --accent-strong: color-mix(in srgb, var(--primary) 34%, transparent);
  --panel-bg: rgba(16, 18, 24, 0.82);
}
.info {
  position: absolute; left: 12px; top: 12px;
  width: 320px; max-width: calc(100vw - 24px);
  /* 内容固定为「前 6 条常用信息」，面板不滚动 */
  overflow: hidden;
  background: var(--panel-bg);
  backdrop-filter: blur(14px);
  border: 1px solid var(--border);
  border-radius: 12px;
  box-shadow: 0 8px 28px rgba(0, 0, 0, 0.45);
  padding: 12px;
  font-size: 12px; line-height: 1.6;
  color: var(--fg);
  z-index: 3;
}
/* 细滚动条，弱化存在感（简略浮层不滚动，无需滚动条） */
.detail::-webkit-scrollbar { width: 8px; }
.detail::-webkit-scrollbar-thumb {
  background: rgba(255, 255, 255, 0.16); border-radius: 4px;
  background-clip: content-box; border: 2px solid transparent;
}
.detail::-webkit-scrollbar-thumb:hover { background-color: rgba(255, 255, 255, 0.28); }

/* 标题行：文件名 + 计数胶囊 */
.info-head { display: flex; align-items: flex-start; gap: 8px; margin-bottom: 10px; }
.info-head .name { font-weight: 600; word-break: break-all; min-width: 0; }
.pill {
  flex-shrink: 0;
  background: var(--accent-soft); color: var(--fg);
  border-radius: 999px; padding: 1px 8px;
  font-size: 11px; font-weight: 500; line-height: 1.5;
}
/* 基本信息：三枚统计卡片 */
.info .stats, .detail .stats { display: grid; grid-template-columns: repeat(3, 1fr); gap: 6px; }
.stat {
  display: flex; flex-direction: column; gap: 1px; min-width: 0;
  background: var(--row); border-radius: 8px; padding: 6px 9px;
}
.stat .k { font-size: 10px; color: var(--fg-muted); }
.stat .v { font-size: 11.5px; font-weight: 600; word-break: break-all; }
/* EXIF 行：标签列固定宽对齐，无冒号；悬停整行提亮 */
.info .rows { margin: 10px 0 0; border-top: 1px solid var(--border); padding-top: 7px; }
.info .rows > div, .detail dl > div {
  display: grid; grid-template-columns: 64px 1fr; gap: 0 10px;
  padding: 3px 6px; border-radius: 6px;
}
.info .rows > div:nth-child(odd) { background: var(--row); }
.info dt, .detail dt { color: var(--fg-muted); }
.info dd, .detail dd { margin: 0; word-break: break-all; }
/* 「更多详情」主按钮：主题色幽灵按钮 */
.info .more {
  margin-top: 10px; width: 100%;
  display: flex; align-items: center; justify-content: center; gap: 6px;
  background: var(--accent-soft); border: none;
  border-radius: 8px; color: var(--fg); cursor: pointer;
  font-size: 12px; padding: 6px 0;
}
.info .more:hover { background: var(--accent-strong); }

/* 详情抽屉：与信息浮层同一套面板语言 */
.detail {
  position: absolute; top: 0; left: 0; bottom: 34px; width: 380px; max-width: 92vw;
  background: var(--panel-bg); backdrop-filter: blur(14px);
  border-right: 1px solid var(--border);
  overflow-y: auto; padding: 0 14px 14px;
  font-size: 12px; line-height: 1.6; color: var(--fg); z-index: 4;
  box-shadow: 8px 0 28px rgba(0, 0, 0, 0.4);
}
.detail header {
  position: sticky; top: 0; z-index: 1;
  display: flex; align-items: center; justify-content: space-between;
  margin: 0 -14px 12px; padding: 10px 14px;
  background: rgba(16, 18, 24, 0.85); backdrop-filter: blur(14px);
  border-bottom: 1px solid var(--border);
  font-weight: 600;
}
.detail .close {
  background: none; border: none; color: var(--fg-muted); cursor: pointer;
  font-size: 16px; line-height: 1; padding: 2px 6px; border-radius: 6px;
}
.detail .close:hover { color: var(--fg); background: var(--row-hover); }
.detail .file-name { margin: 0 0 10px; font-weight: 600; word-break: break-all; }
/* 分区标题：主题色竖条 + 灰字 */
.detail h4 {
  display: flex; align-items: center; gap: 6px;
  margin: 14px 0 6px; font-size: 11px; font-weight: 600;
  color: var(--fg-muted);
}
.detail h4::before {
  content: ''; width: 3px; height: 11px;
  border-radius: 2px; background: var(--accent);
}
.detail dl { margin: 0; }
.detail dl > div { grid-template-columns: 96px 1fr; gap: 0 10px; }
.detail dl > div:nth-child(odd) { background: var(--row); }
.detail dl > div:hover { background: var(--row-hover); }
.detail .none { margin: 0; color: var(--fg-muted); }
/* GPS 位置：等宽坐标 + 两列地图按钮 */
.detail .gps-coord { margin: 0 0 8px; font-family: ui-monospace, Consolas, monospace; letter-spacing: 0.2px; }
/* 逆地理编码出的地名：最多两行，超出省略；解析中给弱化占位 */
.detail .geo-name {
  margin: -2px 0 8px;
  color: var(--fg-muted);
  font-size: 12px;
  line-height: 1.5;
  display: -webkit-box;
  -webkit-box-orient: vertical;
  -webkit-line-clamp: 2;
  overflow: hidden;
}
.detail .geo-name.pending { opacity: 0.6; }
.detail .map-links { display: grid; grid-template-columns: 1fr 1fr; gap: 6px; }
.detail .map-btn {
  background: var(--row); border: none;
  border-radius: 8px; color: var(--fg); cursor: pointer;
  font-size: 12px; padding: 6px 0;
}
.detail .map-btn:hover { background: var(--accent-soft); }
.slide-enter-active, .slide-leave-active { transition: transform 0.18s ease, opacity 0.18s ease; }
.slide-enter-from, .slide-leave-to { transform: translateX(-24px); opacity: 0; }

/* 右键菜单：透明遮罩接管点击，菜单本体浮在光标处 */
.ctx-backdrop { position: fixed; inset: 0; z-index: 20; }
.ctx {
  position: absolute; min-width: 180px;
  background: var(--panel);
  border: 1px solid var(--border);
  border-radius: 10px;
  padding: 5px;
  box-shadow: 0 8px 28px rgba(0, 0, 0, 0.4);
}
.ctx-item {
  display: flex; align-items: center; justify-content: space-between; gap: 18px;
  width: 100%;
  background: none; border: none; border-radius: 6px;
  color: var(--fg); cursor: pointer;
  font-size: 13px; text-align: left; padding: 6px 10px;
}
.ctx-item:hover { background: var(--hover); }
.ctx-item:disabled { opacity: 0.35; cursor: default; }
.ctx-item:disabled:hover { background: none; }
.ctx-item .k { color: var(--fg-muted); font-size: 11px; }
.ctx-sep { height: 1px; background: var(--border); margin: 5px 8px; }

</style>
