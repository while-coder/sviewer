<script setup lang="ts">
import { ref, reactive, computed, nextTick, watch, watchEffect, onMounted, onUnmounted } from 'vue'
import { listen, emit } from '@tauri-apps/api/event'
import { getCurrentWindow } from '@tauri-apps/api/window'
import { WebviewWindow } from '@tauri-apps/api/webviewWindow'
import { getVersion } from '@tauri-apps/api/app'
import { convertFileSrc, invoke } from '@tauri-apps/api/core'
import { open as openDialog, save as saveDialog } from '@tauri-apps/plugin-dialog'
import {
  resolveImageSrc,
  listSiblings,
  readImageInfo,
  getLaunchFile,
  saveImageAs,
  saveEditsTo,
  encodeTo,
  extOf,
  extSupportsEdit,
  type SaveFormat,
  type ImageEdits,
  humanSize,
  exifLabel,
  pickCommonInfo,
  type ImageInfo,
} from './viewer'
import { useAppMenu, type AppMenuAction } from './menu'
import { SAVE_FILTERS, EXT_FORMAT, inferFormat } from './edit-shared'
import { settings, resolvedTheme } from './settings'

// ── 状态 ───────────────────────────────────────────────
const currentPath = ref<string | null>(null)
const imgSrc = ref<string>('')
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
// 设置弹窗当前分类页
const settingsTab = ref<'general' | 'view' | 'assoc' | 'about'>('general')
/** 打开设置弹窗并切到「关于」页（菜单/右键的「关于」入口）。 */
function openAbout() {
  settingsTab.value = 'about'
  modal.value = 'settings'
}
const appVersion = ref('')

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
    title: '批量转换 - SViewer',
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
    title: '编辑 - SViewer',
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

// 视图变换：缩放 + 平移
const view = reactive({ scale: 1, x: 0, y: 0, fit: true })

// ── 编辑：旋转 / 镜像（快捷操作；裁剪/标记/改尺寸在编辑窗口）──
const edit = reactive({ rotation: 0, flip: false })
// 输出偏好（另存为 JPEG 时的质量）
const editOutput = reactive({ format: 'original' as SaveFormat, quality: 85 })

// 旋转 90/270 后显示宽高互换
const swapped = computed(() => edit.rotation % 180 !== 0)
// 旋转后的显示尺寸
const dispW = computed(() => (swapped.value ? natural.h : natural.w))
const dispH = computed(() => (swapped.value ? natural.w : natural.h))
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

/** 能否「保存到原图」：有编辑、目标格式=原格式、且该扩展名可编码写回。 */
const canSaveEdits = computed(() => {
  if (!currentPath.value || !modified.value) return false
  if (editOutput.format !== 'original') return false
  return extSupportsEdit(currentPath.value)
})
const saveBtnTitle = computed(() => {
  if (!currentPath.value) return '保存'
  if (!modified.value) return '保存（先旋转/镜像，更多编辑请右键打开编辑窗口）'
  if (editOutput.format !== 'original') return '保存（已选目标格式，请用「另存为…」）'
  if (!extSupportsEdit(currentPath.value))
    return `保存（.${extOf(currentPath.value)} 不支持直接修改，可用「另存为」转换格式）`
  return '保存（写回原图）'
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
const counter = computed(() =>
  index.value >= 0 ? `${index.value + 1}/${siblings.value.length}` : '',
)
// 窗口标题：文件名 · 计数 · 尺寸 · 大小 · 缩放（顶栏已省掉，信息全放标题栏）
const title = computed(() => {
  if (!info.value) return 'SViewer'
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

// ── 设置：格式关联（Windows，只写 HKCU 免管理员）──────────
// assoc_status 返回空列表（非 Windows / 读取失败）时整个区块不显示。
interface AssocStatus { ext: string; app: string; isSviewer: boolean }
const assocList = ref<AssocStatus[]>([])
const assocSelected = ref<string[]>([])
const assocBusy = ref(false)
const allAssocChecked = computed(
  () => assocList.value.length > 0 && assocSelected.value.length === assocList.value.length,
)
function toggleAllAssoc(e: Event) {
  assocSelected.value = (e.target as HTMLInputElement).checked
    ? assocList.value.map((a) => a.ext)
    : []
}
async function loadAssoc() {
  try {
    assocList.value = await invoke<AssocStatus[]>('assoc_status')
  } catch (e) {
    console.warn('读取格式关联失败', e)
  }
}
async function applyAssoc(exts: string[]) {
  if (!exts.length || assocBusy.value) return
  assocBusy.value = true
  try {
    await invoke('assoc_set', { exts })
    await loadAssoc()
  } catch (e) {
    window.alert(`关联失败：${e}`)
  } finally {
    assocBusy.value = false
  }
}
// 打开设置弹窗时才拉取关联状态
watch(modal, (m) => {
  if (m === 'settings') loadAssoc()
})

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
watchEffect(() => {
  document.documentElement.dataset.theme = resolvedTheme.value
})

// ── 另存为（对话框里选「保存类型」即可顺带转换格式）──────
async function saveAs() {
  const p = currentPath.value
  if (!p) return
  const stem = (info.value?.fileName || p.split(/[\\/]/).pop() || 'image').replace(/\.[^.]+$/, '')
  // 编辑面板里选了目标格式时预填对应扩展名
  const defExt =
    editOutput.format === 'original'
      ? (extOf(p) || 'jpg')
      : editOutput.format === 'jpeg'
        ? 'jpg'
        : editOutput.format
  const dest = await saveDialog({
    defaultPath: `${stem}.${defExt}`,
    filters: SAVE_FILTERS,
  })
  if (!dest) return
  try {
    // 带编辑时「与源同扩展名」也必须重编码（否则 edits 会被 original 快路径丢掉）
    let fmt = inferFormat(dest, p)
    if (modified.value && fmt === 'original') {
      fmt = EXT_FORMAT[dest.split('.').pop()?.toLowerCase() ?? ''] ?? 'original'
    }
    if (fmt === 'original') {
      // 无编辑且同格式：原样复制，不重编码
      await saveImageAs(p, dest, 'original')
    } else {
      await encodeTo(p, dest, fmt, editOutput.format === 'jpeg' ? editOutput.quality : null, editsFromState())
    }
  } catch (e) {
    console.error('另存为失败', e)
    window.alert(`另存为失败：${e}`)
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
    const src = await resolveImageSrc(path)
    if (seq !== loadSeq) return // 已被后续请求取代，丢弃
    imgSrc.value = src
  } catch (e) {
    if (seq !== loadSeq) return
    loadError.value = String(e)
    imgSrc.value = ''
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
        if (seq === loadSeq) siblings.value = list
      })
      .catch((e) => console.warn('读取目录失败', e))
  }
}

async function pickFile() {
  const selected = await openDialog({
    multiple: false,
    filters: [
      {
        name: '图片',
        extensions: ['jpg', 'jpeg', 'png', 'gif', 'webp', 'bmp', 'ico', 'svg', 'avif', 'tiff', 'tif', 'heic', 'heif'],
      },
    ],
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

// ── 视图变换 ───────────────────────────────────────────
// 统一用 transform 实现：view.scale 永远等于真实缩放比例，
// 「适应窗口」就是 scale = min(stage/图片, 1)，百分比显示与实际始终一致。
const stageEl = ref<HTMLElement | null>(null)
const natural = reactive({ w: 0, h: 0 }) // 图片固有尺寸（@load 时记录）

function fitView() {
  view.fit = true
  const el = stageEl.value
  if (!el || !natural.w || !natural.h) {
    view.scale = 1
    view.x = 0
    view.y = 0
    return
  }
  const r = el.getBoundingClientRect()
  // 用旋转后的显示尺寸换算，90° 旋转的竖图才能正确适应窗口
  const s = Math.min(r.width / dispW.value, r.height / dispH.value, 1)
  view.scale = s
  // 居中
  view.x = (r.width - dispW.value * s) / 2
  view.y = (r.height - dispH.value * s) / 2
}

function resetView() {
  fitView()
}

function setScale(next: number, cx?: number, cy?: number) {
  const clamped = Math.min(Math.max(next, 0.1), 20)
  // 以（stage 内）锚点为中心缩放
  if (cx !== undefined && cy !== undefined) {
    const ratio = clamped / view.scale
    view.x = cx - (cx - view.x) * ratio
    view.y = cy - (cy - view.y) * ratio
  }
  view.scale = clamped
  view.fit = false
}

function actualSize() {
  const el = stageEl.value
  if (!el) {
    setScale(1)
    return
  }
  const r = el.getBoundingClientRect()
  setScale(1, r.width / 2, r.height / 2) // 以画布中心为锚点切 1:1
}

function onWheel(e: WheelEvent) {
  e.preventDefault()
  const el = stageEl.value
  if (!el) return
  const r = el.getBoundingClientRect()
  const factor = e.deltaY < 0 ? 1.12 : 1 / 1.12
  setScale(view.scale * factor, e.clientX - r.left, e.clientY - r.top)
}

function toggleFit() {
  if (view.fit) {
    actualSize() // 适应 ↔ 1:1
  } else {
    fitView()
  }
}

// 画布双击：适应 ↔ 1:1。按下时拖拽已解除 fit，所以以按下前的状态判断
function onStageDblClick() {
  if (fitAtDown) actualSize()
  else fitView()
  fitAtDown = false
}

function onImgLoad(e: Event) {
  const img = e.target as HTMLImageElement
  natural.w = img.naturalWidth
  natural.h = img.naturalHeight
  // 图片刚加载完（或快速切换）时按默认视图设置重算
  if (view.fit) {
    if (settings.defaultView === 'actual') actualSize()
    else fitView()
  }
}

// 全屏
const fullscreen = ref(false)
async function toggleFullscreen() {
  const win = getCurrentWindow()
  fullscreen.value = !(await win.isFullscreen())
  await win.setFullscreen(fullscreen.value)
}

// 拖拽平移：Pointer Events + 指针捕获，光标移出窗口/划过信息面板也不会断
const drag = reactive({ on: false, sx: 0, sy: 0, ox: 0, oy: 0 })
// 本次按下序列开始时是否处于「适应窗口」：拖拽会解除 fit，双击判断仍以按下前为准
let fitAtDown = false
function onPointerDown(e: PointerEvent) {
  if (!e.isPrimary || e.button !== 0 || !natural.w || !natural.h) return
  fitAtDown = view.fit
  // 适应模式下也能直接抓图拖动：保持当前缩放与位置，转入自由平移
  view.fit = false
  drag.on = true
  drag.sx = e.clientX
  drag.sy = e.clientY
  drag.ox = view.x
  drag.oy = view.y
  // 捕获后续指针事件：移出画布/窗口仍持续收到 move/up
  ;(e.currentTarget as HTMLElement).setPointerCapture(e.pointerId)
}
function onPointerMove(e: PointerEvent) {
  if (!drag.on) return
  view.x = drag.ox + (e.clientX - drag.sx)
  view.y = drag.oy + (e.clientY - drag.sy)
}
function onPointerUp(e: PointerEvent) {
  if (!drag.on) return
  drag.on = false
  ;(e.currentTarget as HTMLElement).releasePointerCapture(e.pointerId)
}

const imgStyle = computed(() => {
  // 固有尺寸未知（加载瞬间）先用 CSS contain 兜底，避免闪跳
  if (!natural.w || !natural.h) {
    return { maxWidth: '100%', maxHeight: '100%', objectFit: 'contain' as const }
  }
  return {
    // 绝对定位脱离 flex 居中，平移锚点才与坐标计算一致
    position: 'absolute' as const,
    top: '0',
    left: '0',
    // transform 从右往左应用：先绕图片中心旋转/镜像，再以显示区左上角
    // 为锚点缩放、平移——因此 view.x/y 直接对应旋转后外接框（dispW×dispH）的位置
    transform:
      `translate(${view.x}px, ${view.y}px) scale(${view.scale})` +
      ` translate(${dispW.value / 2}px, ${dispH.value / 2}px)` +
      ` scaleX(${edit.flip ? -1 : 1}) rotate(${edit.rotation}deg)` +
      ` translate(${-natural.w / 2}px, ${-natural.h / 2}px)`,
    transformOrigin: '0 0',
    cursor: drag.on ? 'grabbing' : 'grab',
  }
})

// ── 键盘 ───────────────────────────────────────────────
function onKey(e: KeyboardEvent) {
  switch (e.key) {
    case 'ArrowLeft': step(-1); break
    case 'ArrowRight': step(1); break
    case 'Escape':
      // 逐层关闭浮层，最后才最小化窗口
      if (ctx.show) ctx.show = false
      else if (modal.value) modal.value = null
      else if (showDetail.value) showDetail.value = false
      else void getCurrentWindow().minimize()
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
let resizeObserver: ResizeObserver | null = null

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

  // 窗口/画布尺寸变化时，适应模式下重新计算缩放
  resizeObserver = new ResizeObserver(() => {
    if (view.fit) fitView()
  })
  if (stageEl.value) resizeObserver.observe(stageEl.value)

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
  resizeObserver?.disconnect()
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

    <!-- 信息浮层：锚在底部 ⓘ 按钮上方，只放常用信息 -->
    <aside v-if="showInfo && info" class="info">
      <h3>{{ info.fileName }} <span v-if="counter">{{ counter }}</span></h3>
      <dl>
        <div><dt>大小</dt><dd>{{ humanSize(info.size) }}</dd></div>
        <div><dt>尺寸</dt><dd>{{ info.width }} × {{ info.height }}</dd></div>
        <div><dt>格式</dt><dd>{{ info.format }}</dd></div>
      </dl>
      <dl v-if="commonInfo.length > 0" class="common">
        <div v-for="c in commonInfo" :key="c.label"><dt>{{ c.label }}</dt><dd>{{ c.value }}</dd></div>
      </dl>
      <button v-if="info.exif.length > 0" class="more" @click="showDetail = true">
        更多详情（{{ info.exif.length }}）›
      </button>
    </aside>

    <!-- 详情抽屉：完整 EXIF，右侧滑出 -->
    <transition name="slide">
      <aside v-if="showDetail && info" class="detail">
        <header>
          <span>图片详情</span>
          <button class="close" title="关闭" @click="showDetail = false">×</button>
        </header>
        <h4>{{ info.fileName }}</h4>
        <dl>
          <div><dt>大小</dt><dd>{{ humanSize(info.size) }}</dd></div>
          <div><dt>尺寸</dt><dd>{{ info.width }} × {{ info.height }}</dd></div>
          <div><dt>格式</dt><dd>{{ info.format }}</dd></div>
        </dl>
        <h4>EXIF（{{ info.exif.length }}）</h4>
        <dl>
          <div v-for="ex in info.exif" :key="ex.tag">
            <dt>{{ exifLabel(ex.tag) }}</dt><dd>{{ ex.value }}</dd>
          </div>
        </dl>
      </aside>
    </transition>

    <!-- 右键菜单：透明遮罩负责点击关闭，菜单浮在其上 -->
    <div v-if="ctx.show" class="ctx-backdrop" @mousedown="ctx.show = false" @contextmenu.prevent="ctx.show = false">
      <nav ref="ctxEl" class="ctx" :style="{ left: ctx.x + 'px', top: ctx.y + 'px' }" @mousedown.stop>
        <button class="ctx-item" @click="ctxAct(() => pickFile())">打开…<span class="k">O</span></button>
        <button class="ctx-item" :disabled="!currentPath" @click="ctxAct(() => saveAs())">另存为…<span class="k">Ctrl+S</span></button>
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
        <button class="ctx-item" @click="ctxAct(openAbout)">关于 SViewer</button>
      </nav>
    </div>

    <!-- 设置弹窗：左侧分类菜单 + 右侧内容页 -->
    <transition name="fade">
      <div v-if="modal === 'settings'" class="modal-backdrop" @mousedown.self="modal = null">
        <div class="modal settings">
          <header>
            <span>设置</span>
            <button class="close" title="关闭" @click="modal = null">×</button>
          </header>

          <div class="settings-body">
            <nav class="settings-nav">
              <button :class="{ on: settingsTab === 'general' }" @click="settingsTab = 'general'">常规</button>
              <button :class="{ on: settingsTab === 'view' }" @click="settingsTab = 'view'">查看</button>
              <button :class="{ on: settingsTab === 'assoc' }" @click="settingsTab = 'assoc'">格式关联</button>
              <button :class="{ on: settingsTab === 'about' }" @click="settingsTab = 'about'">关于</button>
            </nav>

            <div class="settings-page">
              <template v-if="settingsTab === 'general'">
                <h3>常规</h3>
                <div class="row">
                  <span class="label">主题</span>
                  <div class="seg">
                    <button :class="{ on: settings.theme === 'dark' }" @click="settings.theme = 'dark'">深色</button>
                    <button :class="{ on: settings.theme === 'light' }" @click="settings.theme = 'light'">浅色</button>
                    <button :class="{ on: settings.theme === 'system' }" @click="settings.theme = 'system'">跟随系统</button>
                  </div>
                </div>
              </template>

              <template v-else-if="settingsTab === 'view'">
                <h3>查看</h3>
                <div class="row">
                  <span class="label">打开图片时</span>
                  <div class="seg">
                    <button :class="{ on: settings.defaultView === 'fit' }" @click="settings.defaultView = 'fit'">适应窗口</button>
                    <button :class="{ on: settings.defaultView === 'actual' }" @click="settings.defaultView = 'actual'">原始大小</button>
                  </div>
                </div>

                <div class="row">
                  <span class="label">背景棋盘格</span>
                  <div class="seg">
                    <button :class="{ on: settings.checkerboard }" @click="settings.checkerboard = true">开</button>
                    <button :class="{ on: !settings.checkerboard }" @click="settings.checkerboard = false">关</button>
                  </div>
                </div>

                <div class="row">
                  <span class="label">显示图片边缘</span>
                  <div class="seg">
                    <button :class="{ on: settings.outline }" @click="settings.outline = true">开</button>
                    <button :class="{ on: !settings.outline }" @click="settings.outline = false">关</button>
                  </div>
                </div>

                <div class="row">
                  <span class="label">图片信息面板</span>
                  <div class="seg">
                    <button :class="{ on: settings.showInfo }" @click="settings.showInfo = true">开</button>
                    <button :class="{ on: !settings.showInfo }" @click="settings.showInfo = false">关</button>
                  </div>
                </div>
              </template>

              <!-- 格式关联（仅 Windows）：勾选格式后一键设为默认打开方式 -->
              <template v-else-if="settingsTab === 'assoc'">
                <div class="page-head">
                  <h3>格式关联</h3>
                  <span v-if="assocList.length" class="assoc-actions">
                    <label class="assoc-all"><input type="checkbox" :checked="allAssocChecked" @change="toggleAllAssoc" />全选</label>
                    <button class="mini" :disabled="!assocSelected.length || assocBusy" @click="applyAssoc(assocSelected)">关联所选</button>
                    <button class="mini" :disabled="assocBusy" @click="applyAssoc(assocList.map((a) => a.ext))">关联全部</button>
                  </span>
                </div>
                <template v-if="assocList.length">
                  <div class="assoc-list">
                    <label v-for="a in assocList" :key="a.ext" class="assoc-item">
                      <input v-model="assocSelected" type="checkbox" :value="a.ext" />
                      <span class="ext">.{{ a.ext }}</span>
                      <span class="app" :class="{ ours: a.isSviewer }">{{ a.app }}</span>
                    </label>
                  </div>
                  <p class="assoc-tip">部分格式可能被系统「默认应用」锁定，关联后仍打开异常时请在 Windows 设置 → 默认应用中确认。</p>
                </template>
                <p v-else class="assoc-tip">当前平台不支持格式关联。</p>
              </template>

              <template v-else>
                <div class="about-page">
                  <img class="logo" src="/sviewer-icon.png" alt="SViewer" />
                  <h2>SViewer <small>v{{ appVersion || '?' }}</small></h2>
                  <p class="desc">轻量级本地图片查看器</p>
                  <p class="tip">支持 JPG / PNG / GIF / WebP / AVIF / TIFF / HEIC 等，HEIC 由系统原生解码器加速。</p>
                </div>
              </template>
            </div>
          </div>

        </div>
      </div>
    </transition>
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

/* 信息面板：锚在底部 ⓘ 按钮上方的深色浮层，不遮图片主体、任何背景上都清晰 */
.info {
  position: absolute; right: 12px; bottom: 44px;
  width: max-content; max-width: 360px;
  max-height: min(62%, 460px);
  overflow-y: auto;
  background: var(--overlay);
  border: 1px solid var(--border);
  border-radius: 10px;
  padding: 10px 14px;
  font-size: 12px; line-height: 1.7;
  color: var(--fg);
  backdrop-filter: blur(8px);
  box-shadow: 0 6px 24px rgba(0,0,0,0.5);
  z-index: 3;
}
.info h3 { margin: 0 0 2px; font-size: 12px; font-weight: 600; word-break: break-all; }
.info h3 span { color: var(--fg-muted); font-weight: 400; margin-left: 4px; }
.info dl { margin: 0; }
.info dl > div { display: flex; gap: 8px; }
.info dt { color: var(--fg-muted); flex-shrink: 0; }
.info dt::after { content: ':'; }
.info dd { margin: 0; word-break: break-all; }
.info .common { border-top: 1px solid var(--border); margin-top: 6px; padding-top: 4px; }
.info .more {
  margin-top: 8px; width: 100%;
  background: var(--hover); border: 1px solid var(--border);
  border-radius: 6px; color: var(--fg); cursor: pointer;
  font-size: 12px; padding: 4px 0;
}
.info .more:hover { background: var(--hover-strong); }

/* 详情抽屉：完整 EXIF，右侧滑出 */
.detail {
  position: absolute; top: 0; right: 0; bottom: 34px; width: 380px; max-width: 92vw;
  background: var(--panel); border-left: 1px solid var(--border);
  overflow-y: auto; padding: 0 16px 14px;
  font-size: 12px; line-height: 1.7; z-index: 4;
}
.detail header {
  position: sticky; top: 0; z-index: 1;
  display: flex; align-items: center; justify-content: space-between;
  margin: 0 -16px 10px; padding: 10px 16px;
  background: var(--panel); border-bottom: 1px solid var(--border);
  font-weight: 600;
}
.detail .close {
  background: none; border: none; color: var(--fg-muted); cursor: pointer;
  font-size: 16px; line-height: 1; padding: 2px 6px; border-radius: 6px;
}
.detail .close:hover { color: var(--fg); background: var(--hover); }
.detail h4 { margin: 10px 0 4px; font-size: 12px; color: var(--fg-muted); font-weight: 600; word-break: break-all; }
.detail dl { margin: 0; }
.detail dl > div { display: flex; gap: 8px; padding: 1px 0; }
.detail dt { color: var(--fg-muted); flex-shrink: 0; }
.detail dt::after { content: ':'; }
.detail dd { margin: 0; word-break: break-all; }
.slide-enter-active, .slide-leave-active { transition: transform 0.18s ease, opacity 0.18s ease; }
.slide-enter-from, .slide-leave-to { transform: translateX(24px); opacity: 0; }

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

/* 关于 / 设置 弹窗 */
.fade-enter-active, .fade-leave-active { transition: opacity 0.15s ease; }
.fade-enter-from, .fade-leave-to { opacity: 0; }
.modal-backdrop {
  position: fixed; inset: 0; z-index: 30;
  display: flex; align-items: center; justify-content: center;
  background: rgba(0, 0, 0, 0.45);
}
.modal {
  width: 400px; max-width: 92vw;
  background: var(--panel);
  border: 1px solid var(--border);
  border-radius: 12px;
  padding: 18px 20px;
  box-shadow: 0 12px 40px rgba(0, 0, 0, 0.4);
}
/* 「关于」页：居中展示应用信息 */
.about-page { text-align: center; padding-top: 36px; }
.about-page .logo { width: 64px; height: 64px; margin-bottom: 8px; }
.about-page h2 { margin: 0; font-size: 18px; }
.about-page h2 small { color: var(--fg-muted); font-size: 12px; font-weight: 400; margin-left: 6px; }
.about-page .desc { margin: 6px 0 2px; }
.about-page .tip { margin: 8px 0 0; color: var(--fg-muted); font-size: 12px; line-height: 1.7; }
.modal.settings {
  width: 620px; max-width: 94vw;
  height: 480px; max-height: 86vh; /* 高度固定，切换分类页时窗口不跳动 */
  padding: 0; overflow: hidden;
  display: flex; flex-direction: column;
}
.modal.settings header {
  display: flex; align-items: center; justify-content: space-between;
  padding: 12px 20px;
  border-bottom: 1px solid var(--border);
  font-weight: 600;
}
.modal.settings .close {
  background: none; border: none; color: var(--fg-muted); cursor: pointer;
  font-size: 16px; line-height: 1; padding: 2px 6px; border-radius: 6px;
}
.modal.settings .close:hover { color: var(--fg); background: var(--hover); }

/* 设置弹窗主体：左分类菜单 + 右内容页 */
.settings-body { display: flex; flex: 1; min-height: 0; }
.settings-nav {
  width: 128px; flex-shrink: 0;
  display: flex; flex-direction: column; gap: 2px;
  padding: 10px 8px;
  border-right: 1px solid var(--border);
}
.settings-nav button {
  background: none; border: none; cursor: pointer;
  color: var(--fg-muted); font-size: 13px; text-align: left;
  padding: 7px 12px; border-radius: 8px;
}
.settings-nav button:hover { background: var(--hover); color: var(--fg); }
.settings-nav button.on { background: var(--hover); color: var(--primary); font-weight: 600; }
.settings-page { flex: 1; min-width: 0; padding: 14px 20px; overflow-y: auto; }
.settings-page h3 { margin: 0 0 4px; font-size: 14px; }
.page-head { display: flex; align-items: center; justify-content: space-between; gap: 12px; margin-bottom: 8px; }
.page-head h3 { margin: 0; }

/* 设置行：左标签右分段选择器 */
.row {
  display: flex; align-items: center; justify-content: space-between; gap: 12px;
  padding: 9px 0;
}
.row .label { flex-shrink: 0; }
.seg { display: flex; border: 1px solid var(--border); border-radius: 8px; overflow: hidden; }
.seg button {
  background: none; border: none; color: var(--fg-muted); cursor: pointer;
  font-size: 12px; padding: 4px 10px;
}
.seg button + button { border-left: 1px solid var(--border); }
.seg button:hover { background: var(--hover); }
.seg button.on { background: var(--primary); color: #fff; }

/* 格式关联：工具行 + 可滚动格式列表 */
.assoc-actions { display: flex; align-items: center; gap: 8px; }
.assoc-all { display: flex; align-items: center; gap: 4px; font-size: 12px; color: var(--fg-muted); cursor: pointer; }
.assoc-actions .mini {
  background: none; border: 1px solid var(--border); color: var(--fg); cursor: pointer;
  font-size: 12px; padding: 3px 10px; border-radius: 6px;
}
.assoc-actions .mini:hover:not(:disabled) { background: var(--hover); }
.assoc-actions .mini:disabled { opacity: 0.4; cursor: default; }
.assoc-list {
  max-height: 176px; overflow-y: auto;
  border: 1px solid var(--border); border-radius: 8px;
}
.assoc-item {
  display: flex; align-items: center; gap: 8px;
  padding: 4px 10px; font-size: 12px; cursor: pointer;
}
.assoc-item:hover { background: var(--hover); }
.assoc-item .ext { width: 52px; }
.assoc-item .app { margin-left: auto; color: var(--fg-muted); }
.assoc-item .app.ours { color: var(--primary); }
.assoc-tip { margin: 6px 0 0; font-size: 11px; line-height: 1.6; color: var(--fg-muted); }
</style>
