<script setup lang="ts">
/**
 * 编辑窗口根组件（edit.ts 挂载）：独立编辑器，与批量转换窗口同构。
 *
 * - 由主窗口经 WebviewWindow('edit') 创建，初始图片经 URL ?path= 传入；
 *   之后主窗口再次点「编辑」时发 edit-file 事件切换图片。
 * - 编辑项：旋转 / 镜像 / 框选裁剪 / 改尺寸 / 标记（画笔·矩形·椭圆·箭头），
 *   保存时整体交给 Rust 编辑管线烘焙写回或另存。
 * - 标记存「显示空间」坐标（EXIF 归一化 + 旋转/镜像后、裁剪前），与裁剪框
 *   同一坐标系，旋转/镜像时一起变换；预览用 SVG，落盘由 Rust 光栅化，所见即所得。
 */
import { ref, reactive, computed, nextTick, watchEffect, onMounted, onUnmounted } from 'vue'
import { listen, emit } from '@tauri-apps/api/event'
import { getCurrentWindow } from '@tauri-apps/api/window'
import { convertFileSrc } from '@tauri-apps/api/core'
import { save as saveDialog } from '@tauri-apps/plugin-dialog'
import {
  resolveImageSrc,
  readImageInfo,
  saveEditsTo,
  saveImageAs,
  encodeTo,
  extOf,
  extSupportsEdit,
  type SaveFormat,
  type ImageEdits,
  type CropRect,
  type MarkShape,
} from './viewer'
import { SAVE_FILTERS, EXT_FORMAT, inferFormat } from './edit-shared'
import { settings, resolvedTheme, watchExternalSettings } from './settings'

watchEffect(() => {
  document.documentElement.dataset.theme = resolvedTheme.value
})
watchExternalSettings()

// ── 图片状态 ───────────────────────────────────────────
const path = ref<string | null>(null)
const info = ref<{ fileName: string } | null>(null)
const imgSrc = ref<string>('')
const loadError = ref<string>('')
const loading = ref(false)

// ── 视图变换：缩放 + 平移 ──────────────────────────────
const view = reactive({ scale: 1, x: 0, y: 0, fit: true })
const stageEl = ref<HTMLElement | null>(null)
const natural = reactive({ w: 0, h: 0 })

// ── 编辑状态 ───────────────────────────────────────────
const edit = reactive({
  rotation: 0,
  flip: false,
  crop: null as CropRect | null, // 显示空间像素坐标（浮点，保存时取整）
  resize: null as { w: number; h: number; keep: boolean } | null,
})
const editOutput = reactive({ format: 'original' as SaveFormat, quality: 85 })
const marks = ref<MarkShape[]>([])
const draft = ref<MarkShape | null>(null) // 正在拖的笔画

// 工具：pan = 平移看图；crop = 框选裁剪；其余为标记工具
type Tool = 'pan' | 'crop' | 'pen' | 'rect' | 'ellipse' | 'arrow'
const tool = ref<Tool>('pan')
const isMarkTool = computed(() => tool.value === 'pen' || tool.value === 'rect' || tool.value === 'ellipse' || tool.value === 'arrow')

const COLORS = ['#ff3b30', '#ff9500', '#ffcc00', '#34c759', '#2d6cdf', '#ffffff', '#000000']
const markColor = ref('#ff3b30')
const markWidth = ref(4)

const swapped = computed(() => edit.rotation % 180 !== 0)
const dispW = computed(() => (swapped.value ? natural.h : natural.w))
const dispH = computed(() => (swapped.value ? natural.w : natural.h))
const modified = computed(
  () => edit.rotation !== 0 || edit.flip || !!edit.crop || !!edit.resize || marks.value.length > 0,
)

const pct = computed(() => `${Math.round(view.scale * 100)}%`)
watchEffect(() => {
  const name = info.value?.fileName
  getCurrentWindow().setTitle(name ? `编辑 - ${name} - SViewer` : '编辑 - SViewer').catch(() => {})
})

// ── 加载图片 ───────────────────────────────────────────
let loadSeq = 0
async function loadPath(p: string) {
  const seq = ++loadSeq
  path.value = p
  loading.value = true
  loadError.value = ''
  imgSrc.value = ''
  natural.w = 0
  natural.h = 0
  // 上一张的编辑不带到下一张
  edit.rotation = 0
  edit.flip = false
  edit.crop = null
  edit.resize = null
  marks.value = []
  draft.value = null
  tool.value = 'pan'
  view.fit = true
  try {
    const src = await resolveImageSrc(p)
    if (seq !== loadSeq) return
    imgSrc.value = src
  } catch (e) {
    if (seq !== loadSeq) return
    loadError.value = String(e)
    imgSrc.value = ''
    console.error('加载图片失败', p, e)
    return
  } finally {
    if (seq === loadSeq) loading.value = false
  }
  readImageInfo(p)
    .then((i) => {
      if (seq === loadSeq) info.value = i
    })
    .catch(() => {})
}

function onImgLoad(e: Event) {
  const img = e.target as HTMLImageElement
  natural.w = img.naturalWidth
  natural.h = img.naturalHeight
  fitView()
}

// ── 视图：适应 / 1:1 / 缩放 ────────────────────────────
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
  const s = Math.min(r.width / dispW.value, r.height / dispH.value, 1)
  view.scale = s
  view.x = (r.width - dispW.value * s) / 2
  view.y = (r.height - dispH.value * s) / 2
}

function setScale(next: number, cx?: number, cy?: number) {
  const clamped = Math.min(Math.max(next, 0.1), 20)
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
  const r = el?.getBoundingClientRect()
  setScale(1, r ? r.width / 2 : undefined, r ? r.height / 2 : undefined)
}

function onWheel(e: WheelEvent) {
  e.preventDefault()
  const el = stageEl.value
  if (!el) return
  const r = el.getBoundingClientRect()
  const factor = e.deltaY < 0 ? 1.12 : 1 / 1.12
  setScale(view.scale * factor, e.clientX - r.left, e.clientY - r.top)
}

// ── 平移（仅 pan 工具）────────────────────────────────
const drag = reactive({ on: false, sx: 0, sy: 0, ox: 0, oy: 0 })
function onPointerDown(e: PointerEvent) {
  if (tool.value !== 'pan' || !e.isPrimary || e.button !== 0 || !natural.w) return
  view.fit = false
  drag.on = true
  drag.sx = e.clientX
  drag.sy = e.clientY
  drag.ox = view.x
  drag.oy = view.y
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

// ── 旋转 / 镜像（裁剪框与标记一起变换）─────────────────
/** 显示空间点随 +90° 顺时针旋转：(x, y) → (oldH - y, x)。 */
function rotPt(p: [number, number], oldH: number): [number, number] {
  return [oldH - p[1], p[0]]
}

function rotate() {
  if (!natural.w) return
  const oldW = dispW.value
  const oldH = dispH.value
  edit.rotation = (edit.rotation + 90) % 360
  if (edit.crop) {
    const c = edit.crop
    edit.crop = { x: oldH - (c.y + c.h), y: c.x, w: c.h, h: c.w }
  }
  if (marks.value.length) {
    marks.value = marks.value.map((m) => ({ ...m, pts: m.pts.map((p) => rotPt(p, oldH)) }))
  }
  if (draft.value) draft.value = null // 拖一半就旋转：直接丢弃在途笔画
  if (view.fit) fitView()
}

function mirror() {
  if (!natural.w) return
  edit.flip = !edit.flip
  const flipX = (p: [number, number]): [number, number] => [dispW.value - p[0], p[1]]
  if (edit.crop) edit.crop = { ...edit.crop, x: dispW.value - (edit.crop.x + edit.crop.w) }
  if (marks.value.length) {
    marks.value = marks.value.map((m) => ({ ...m, pts: m.pts.map(flipX) }))
  }
  draft.value = null
}

// ── 框选裁剪（与主窗口同一套交互）──────────────────────
const HANDLES = ['nw', 'n', 'ne', 'e', 'se', 's', 'sw', 'w'] as const
type Handle = (typeof HANDLES)[number]

const cropDrag = reactive({
  on: false,
  mode: 'new' as 'new' | 'move' | Handle,
  sx: 0,
  sy: 0,
  orig: { x: 0, y: 0, w: 0, h: 0 },
})

/** stage 客户端坐标 → 显示空间图片坐标。 */
function stageToDisp(e: PointerEvent): { x: number; y: number } {
  const r = stageEl.value!.getBoundingClientRect()
  return {
    x: (e.clientX - r.left - view.x) / view.scale,
    y: (e.clientY - r.top - view.y) / view.scale,
  }
}
const clampX = (x: number) => Math.min(Math.max(x, 0), dispW.value)
const clampY = (y: number) => Math.min(Math.max(y, 0), dispH.value)

function onCropLayerDown(e: PointerEvent) {
  if (e.button !== 0) return
  e.stopPropagation()
  ;(e.currentTarget as HTMLElement).setPointerCapture(e.pointerId)
  const p = stageToDisp(e)
  cropDrag.on = true
  cropDrag.mode = 'new'
  cropDrag.sx = clampX(p.x)
  cropDrag.sy = clampY(p.y)
  edit.crop = { x: cropDrag.sx, y: cropDrag.sy, w: 0, h: 0 }
}

function onHandleDown(h: Handle | 'move', e: PointerEvent) {
  if (e.button !== 0 || !edit.crop) return
  e.stopPropagation()
  ;(e.currentTarget as HTMLElement).setPointerCapture(e.pointerId)
  const p = stageToDisp(e)
  cropDrag.on = true
  cropDrag.mode = h
  cropDrag.sx = p.x
  cropDrag.sy = p.y
  cropDrag.orig = { ...edit.crop }
}

function onCropMove(e: PointerEvent) {
  if (!cropDrag.on) return
  const p = stageToDisp(e)
  if (cropDrag.mode === 'new') {
    const x2 = clampX(p.x)
    const y2 = clampY(p.y)
    edit.crop = {
      x: Math.min(cropDrag.sx, x2),
      y: Math.min(cropDrag.sy, y2),
      w: Math.abs(x2 - cropDrag.sx),
      h: Math.abs(y2 - cropDrag.sy),
    }
    return
  }
  if (!edit.crop) return
  const o = cropDrag.orig
  const dx = p.x - cropDrag.sx
  const dy = p.y - cropDrag.sy
  if (cropDrag.mode === 'move') {
    edit.crop = {
      ...o,
      x: Math.min(Math.max(o.x + dx, 0), dispW.value - o.w),
      y: Math.min(Math.max(o.y + dy, 0), dispH.value - o.h),
    }
    return
  }
  let { x, y, w, h } = o
  if (cropDrag.mode.includes('w')) {
    x = Math.min(clampX(o.x + dx), o.x + o.w - 1)
    w = o.x + o.w - x
  }
  if (cropDrag.mode.includes('e')) {
    w = Math.max(clampX(o.x + o.w + dx) - o.x, 1)
  }
  if (cropDrag.mode.includes('n')) {
    y = Math.min(clampY(o.y + dy), o.y + o.h - 1)
    h = o.y + o.h - y
  }
  if (cropDrag.mode.includes('s')) {
    h = Math.max(clampY(o.y + o.h + dy) - o.y, 1)
  }
  edit.crop = { x, y, w, h }
}

function onCropUp(e: PointerEvent) {
  if (!cropDrag.on) return
  cropDrag.on = false
  ;(e.currentTarget as HTMLElement).releasePointerCapture(e.pointerId)
  if (cropDrag.mode === 'new' && edit.crop && (edit.crop.w < 8 || edit.crop.h < 8)) {
    edit.crop = null
    return
  }
  // 框选完成即退出裁剪工具：裁剪框保留（虚线常显 + 提示条），保存时生效
  if (cropDrag.mode === 'new') tool.value = 'pan'
}

const cropLayerStyle = computed(() => ({
  position: 'absolute' as const,
  left: '0',
  top: '0',
  width: `${dispW.value}px`,
  height: `${dispH.value}px`,
  transform: `translate(${view.x}px, ${view.y}px) scale(${view.scale})`,
  transformOrigin: '0 0',
}))

const cropBoxStyle = computed(() => {
  const c = edit.crop
  if (!c) return null
  return {
    left: `${c.x}px`,
    top: `${c.y}px`,
    width: `${c.w}px`,
    height: `${c.h}px`,
    '--hs': `${10 / view.scale}px`,
  }
})

/** 裁剪后尺寸（改尺寸的等比基准）。 */
const cropBase = computed(() => ({
  w: edit.crop ? Math.round(edit.crop.w) : dispW.value,
  h: edit.crop ? Math.round(edit.crop.h) : dispH.value,
}))

// ── 标记绘制 ───────────────────────────────────────────
/** 屏幕上 2px 对应的显示空间距离（画笔采点阈值，防密堆积）。 */
const penStep = computed(() => 2 / Math.max(view.scale, 0.01))

function onMarkDown(e: PointerEvent) {
  if (e.button !== 0 || !isMarkTool.value) return
  e.stopPropagation()
  ;(e.currentTarget as HTMLElement).setPointerCapture(e.pointerId)
  const p = stageToDisp(e)
  const pt: [number, number] = [clampX(p.x), clampY(p.y)]
  draft.value =
    tool.value === 'pen'
      ? { kind: 'pen', color: markColor.value, width: markWidth.value, pts: [pt] }
      : { kind: tool.value, color: markColor.value, width: markWidth.value, pts: [pt, pt] }
}

function onMarkMove(e: PointerEvent) {
  const d = draft.value
  if (!d) return
  const p = stageToDisp(e)
  if (d.kind === 'pen') {
    const last = d.pts[d.pts.length - 1]
    const x = clampX(p.x)
    const y = clampY(p.y)
    if (Math.hypot(x - last[0], y - last[1]) < penStep.value) return
    d.pts.push([x, y])
  } else {
    d.pts[1] = [clampX(p.x), clampY(p.y)]
  }
}

function onMarkUp(e: PointerEvent) {
  const d = draft.value
  if (!d) return
  draft.value = null
  ;(e.currentTarget as HTMLElement).releasePointerCapture(e.pointerId)
  if (d.kind === 'pen') {
    if (d.pts.length >= 2) marks.value.push(d)
    return
  }
  const [[x0, y0], [x1, y1]] = d.pts
  // 手抖出的小点丢弃
  if (Math.abs(x1 - x0) >= 3 || Math.abs(y1 - y0) >= 3) marks.value.push(d)
}

function undoMark() {
  marks.value.pop()
}

// ── 标记 → SVG 预览（与 Rust 光栅化同一几何定义，所见即所得）──
type Shape =
  | { tag: 'rect'; x: number; y: number; w: number; h: number; color: string; width: number }
  | { tag: 'ellipse'; cx: number; cy: number; rx: number; ry: number; color: string; width: number }
  | { tag: 'polyline'; points: string; color: string; width: number }
  | { tag: 'line'; x1: number; y1: number; x2: number; y2: number; color: string; width: number }

function toShapes(list: MarkShape[]): Shape[] {
  const out: Shape[] = []
  for (const m of list) {
    const a = m.pts[0]
    const b = m.pts[m.pts.length - 1]
    if (!a) continue
    if (m.kind === 'rect' && b) {
      out.push({
        tag: 'rect',
        x: Math.min(a[0], b[0]), y: Math.min(a[1], b[1]),
        w: Math.abs(b[0] - a[0]), h: Math.abs(b[1] - a[1]),
        color: m.color, width: m.width,
      })
    } else if (m.kind === 'ellipse' && b) {
      out.push({
        tag: 'ellipse',
        cx: (a[0] + b[0]) / 2, cy: (a[1] + b[1]) / 2,
        rx: Math.abs(b[0] - a[0]) / 2, ry: Math.abs(b[1] - a[1]) / 2,
        color: m.color, width: m.width,
      })
    } else if (m.kind === 'pen' && m.pts.length >= 2) {
      out.push({ tag: 'polyline', points: m.pts.map((p) => p.join(',')).join(' '), color: m.color, width: m.width })
    } else if (m.kind === 'arrow' && b) {
      out.push({ tag: 'line', x1: a[0], y1: a[1], x2: b[0], y2: b[1], color: m.color, width: m.width })
      // 箭翼：≈146° 后掠角，翼长与 Rust draw_arrow 一致
      const ang = Math.atan2(b[1] - a[1], b[0] - a[0])
      const hl = Math.max(m.width * 3, 10)
      for (const da of [2.55, -2.55]) {
        out.push({
          tag: 'line',
          x1: b[0], y1: b[1],
          x2: b[0] + hl * Math.cos(ang + da), y2: b[1] + hl * Math.sin(ang + da),
          color: m.color, width: m.width,
        })
      }
    }
  }
  return out
}

const shapes = computed(() => toShapes([...marks.value, ...(draft.value ? [draft.value] : [])]))

// ── 保存 ───────────────────────────────────────────────
/** 当前编辑 → Rust ImageEdits（取整、去掉无效果的 resize）。 */
function editsFromState(): ImageEdits {
  const base = cropBase.value
  const r = edit.resize
  const resize: [number, number] | null =
    r && (Math.round(r.w) !== base.w || Math.round(r.h) !== base.h)
      ? [Math.max(1, Math.round(r.w)), Math.max(1, Math.round(r.h))]
      : null
  const c = edit.crop
  return {
    rotation: edit.rotation,
    flip: edit.flip,
    crop:
      c && c.w >= 1 && c.h >= 1
        ? { x: Math.round(c.x), y: Math.round(c.y), w: Math.round(c.w), h: Math.round(c.h) }
        : null,
    resize,
    quality: editOutput.format === 'jpeg' ? editOutput.quality : null,
    marks: marks.value.length ? marks.value : null,
  }
}

/** 能否「保存到原图」：有编辑、目标格式=原格式、且该扩展名可编码写回。 */
const canSaveEdits = computed(() => {
  if (!path.value || !modified.value) return false
  if (editOutput.format !== 'original') return false
  return extSupportsEdit(path.value)
})
const saveBtnTitle = computed(() => {
  if (!path.value) return '保存'
  if (!modified.value) return '保存（先旋转/镜像/裁剪/改尺寸/标记）'
  if (editOutput.format !== 'original') return '保存（已选目标格式，请用「另存为…」）'
  if (!extSupportsEdit(path.value))
    return `保存（.${extOf(path.value)} 不支持直接修改，可用「另存为」转换格式）`
  return '保存（写回原图）'
})

const savingEdits = ref(false)
async function saveEdits() {
  const p = path.value
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
  resetEdits()
  // 通知主窗口刷新当前图（可编辑格式都是 web 原生，加时间戳绕过 WebView 缓存）
  void emit('image-edited', p)
  const seq = ++loadSeq
  imgSrc.value = `${convertFileSrc(p)}?v=${Date.now()}`
  readImageInfo(p)
    .then((i) => {
      if (seq === loadSeq) info.value = i
    })
    .catch(() => {})
}

function resetEdits() {
  edit.rotation = 0
  edit.flip = false
  edit.crop = null
  edit.resize = null
  marks.value = []
  editOutput.format = 'original'
  editOutput.quality = 85
}

// ── 另存为 ─────────────────────────────────────────────
async function saveAs() {
  const p = path.value
  if (!p || savingEdits.value) return
  const stem = (info.value?.fileName || p.split(/[\\/]/).pop() || 'image').replace(/\.[^.]+$/, '')
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

// ── 侧栏：裁剪数值微调 / 改尺寸 ────────────────────────
function setCropField(k: keyof CropRect, v: number) {
  if (!edit.crop) return
  const b = cropBase.value
  const c = { ...edit.crop }
  c[k] = Math.max(0, Math.round(v) || 0)
  c.x = Math.min(c.x, Math.max(0, b.w - 1))
  c.y = Math.min(c.y, Math.max(0, b.h - 1))
  c.w = Math.min(Math.max(1, c.w), b.w - c.x)
  c.h = Math.min(Math.max(1, c.h), b.h - c.y)
  edit.crop = c
}
function cropWhole() {
  edit.crop = { x: 0, y: 0, w: cropBase.value.w, h: cropBase.value.h }
}

const resizeW = computed(() => edit.resize?.w ?? cropBase.value.w)
const resizeH = computed(() => edit.resize?.h ?? cropBase.value.h)
const resizeKeep = computed(() => edit.resize?.keep ?? true)

function setResize(w: number, h: number, keep: boolean) {
  edit.resize = { w: Math.max(1, Math.round(w) || 1), h: Math.max(1, Math.round(h) || 1), keep }
}
function onResizeW(v: number) {
  const w = Math.max(1, Math.round(v) || 1)
  setResize(w, resizeKeep.value ? Math.max(1, Math.round((w * cropBase.value.h) / cropBase.value.w)) : resizeH.value, resizeKeep.value)
}
function onResizeH(v: number) {
  const h = Math.max(1, Math.round(v) || 1)
  setResize(resizeKeep.value ? Math.max(1, Math.round((h * cropBase.value.w) / cropBase.value.h)) : resizeW.value, h, resizeKeep.value)
}
function onKeepToggle() {
  const next = !resizeKeep.value
  if (next) {
    setResize(resizeW.value, Math.max(1, Math.round((resizeW.value * cropBase.value.h) / cropBase.value.w)), true)
  } else {
    setResize(resizeW.value, resizeH.value, false)
  }
}
const resizeChanged = computed(
  () => resizeW.value !== cropBase.value.w || resizeH.value !== cropBase.value.h,
)

const FORMATS: { value: SaveFormat; label: string }[] = [
  { value: 'original', label: '原格式' },
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

// ── 图片样式（与主窗口同一套变换）──────────────────────
const imgStyle = computed(() => {
  if (!natural.w || !natural.h) {
    return { maxWidth: '100%', maxHeight: '100%', objectFit: 'contain' as const }
  }
  return {
    position: 'absolute' as const,
    top: '0',
    left: '0',
    transform:
      `translate(${view.x}px, ${view.y}px) scale(${view.scale})` +
      ` translate(${dispW.value / 2}px, ${dispH.value / 2}px)` +
      ` scaleX(${edit.flip ? -1 : 1}) rotate(${edit.rotation}deg)` +
      ` translate(${-natural.w / 2}px, ${-natural.h / 2}px)`,
    transformOrigin: '0 0',
  }
})

const stageCursor = computed(() => {
  if (tool.value === 'crop') return 'crosshair'
  if (isMarkTool.value) return 'crosshair'
  return drag.on ? 'grabbing' : 'grab'
})

// ── 键盘 ───────────────────────────────────────────────
function onKey(e: KeyboardEvent) {
  switch (e.key) {
    case 'Escape':
      // 逐层退：丢弃在途笔画 → 退出工具回平移
      if (draft.value) draft.value = null
      else if (tool.value !== 'pan') tool.value = 'pan'
      break
    case 's': case 'S':
      if (e.ctrlKey || e.metaKey) {
        e.preventDefault()
        if (canSaveEdits.value) void saveEdits()
        else void saveAs()
      }
      break
    case '+': case '=': setScale(view.scale * 1.2); break
    case '-': setScale(view.scale / 1.2); break
    case '0': fitView(); break
    case '1': actualSize(); break
    case 'r': case 'R': rotate(); break
    case 'm': case 'M': mirror(); break
  }
}

// ── 生命周期 ───────────────────────────────────────────
let unlistenFile: (() => void) | null = null
const resizeObserver = new ResizeObserver(() => {
  if (view.fit) fitView()
})

onMounted(async () => {
  window.addEventListener('keydown', onKey)
  // 主窗口「编辑」：已有编辑窗口时发 edit-file 切图；首次创建则经 URL 传入
  const initial = new URLSearchParams(location.search).get('path')
  if (initial) void loadPath(initial)
  unlistenFile = await listen<string>('edit-file', (e) => {
    if (e.payload) void loadPath(e.payload)
  })
  await nextTick()
  if (stageEl.value) resizeObserver.observe(stageEl.value)
})

onUnmounted(() => {
  window.removeEventListener('keydown', onKey)
  unlistenFile?.()
  resizeObserver.disconnect()
})
</script>

<template>
  <div class="ew" @contextmenu.prevent>
    <header class="bar">
      <button class="tool" title="适应窗口 (0)" @click="fitView">⤢</button>
      <button class="tool" title="实际大小 (1)" @click="actualSize">1:1</button>
      <button class="tool" title="缩小 (-)" @click="setScale(view.scale / 1.2)">−</button>
      <span class="pct">{{ pct }}</span>
      <button class="tool" title="放大 (+)" @click="setScale(view.scale * 1.2)">＋</button>
      <span class="sep" />
      <button class="tool" title="旋转 90° (R)" :disabled="!natural.w" @click="rotate">↻</button>
      <button class="tool" title="镜像 (M)" :disabled="!natural.w" @click="mirror">⇋</button>
      <span class="sep" />
      <button class="tool" :class="{ active: tool === 'pan' }" title="平移查看" @click="tool = 'pan'">✥</button>
      <button class="tool" :class="{ active: tool === 'crop' }" title="框选裁剪" :disabled="!natural.w" @click="tool = 'crop'">⛶</button>
      <button class="tool" :class="{ active: tool === 'pen' }" title="画笔标记" :disabled="!natural.w" @click="tool = 'pen'">✏</button>
      <button class="tool" :class="{ active: tool === 'rect' }" title="矩形标记" :disabled="!natural.w" @click="tool = 'rect'">▭</button>
      <button class="tool" :class="{ active: tool === 'ellipse' }" title="椭圆标记" :disabled="!natural.w" @click="tool = 'ellipse'">◯</button>
      <button class="tool" :class="{ active: tool === 'arrow' }" title="箭头标记" :disabled="!natural.w" @click="tool = 'arrow'">↗</button>
      <template v-if="isMarkTool">
        <span class="swatches">
          <button
            v-for="c in COLORS" :key="c" class="swatch" :class="{ on: markColor === c }"
            :style="{ background: c }" :title="c" @click="markColor = c"
          />
        </span>
        <input v-model="markColor" type="color" class="color-input" title="自定义颜色" />
        <select v-model.number="markWidth" class="w-select" title="线宽">
          <option v-for="w in [2, 4, 6, 10, 16]" :key="w" :value="w">{{ w }}px</option>
        </select>
      </template>
      <span class="sep" />
      <button class="tool" title="撤销上一笔" :disabled="!marks.length" @click="undoMark">↶</button>
      <button class="tool" title="清空全部标记" :disabled="!marks.length" @click="marks = []">🗑</button>
      <span class="grow" />
      <button class="ep-btn" :title="saveBtnTitle" @click="saveAs()">另存为…</button>
      <button class="btn-primary" :disabled="!canSaveEdits || savingEdits" :title="saveBtnTitle" @click="saveEdits">
        {{ savingEdits ? '保存中…' : '保存到原图' }}
      </button>
    </header>

    <div class="body">
      <main
        ref="stageEl"
        class="stage"
        :class="{ plain: !settings.checkerboard }"
        :style="{ cursor: stageCursor }"
        @wheel="onWheel"
        @pointerdown="onPointerDown"
        @pointermove="onPointerMove"
        @pointerup="onPointerUp"
        @pointercancel="onPointerUp"
      >
        <template v-if="imgSrc">
          <img :src="imgSrc" :style="imgStyle" class="pic" draggable="false" alt="" @load="onImgLoad" />

          <!-- 框选裁剪层 -->
          <div
            v-if="tool === 'crop' && natural.w"
            class="crop-layer"
            :style="cropLayerStyle"
            @pointerdown="onCropLayerDown"
            @pointermove="onCropMove"
            @pointerup="onCropUp"
            @pointercancel="onCropUp"
          >
            <div v-if="edit.crop" class="crop-box" :style="cropBoxStyle" @pointerdown="onHandleDown('move', $event)">
              <span v-for="h in HANDLES" :key="h" :class="['crop-h', h]" @pointerdown.stop="onHandleDown(h, $event)" />
            </div>
          </div>

          <!-- 已就绪的裁剪框：非裁剪工具时常显（虚线 + 弱遮罩），保存时生效 -->
          <div v-if="tool !== 'crop' && edit.crop && natural.w" class="crop-layer static" :style="cropLayerStyle">
            <div class="crop-box static" :style="cropBoxStyle" />
          </div>

          <!-- 标记绘制层（仅标记工具时接管指针） -->
          <div
            v-if="isMarkTool && natural.w"
            class="crop-layer mark-layer"
            :style="cropLayerStyle"
            @pointerdown="onMarkDown"
            @pointermove="onMarkMove"
            @pointerup="onMarkUp"
            @pointercancel="onMarkUp"
          />

          <!-- 标记预览：SVG 与图片同变换，线宽随缩放与落盘效果一致 -->
          <svg
            v-if="natural.w && shapes.length"
            class="mark-preview"
            :width="dispW"
            :height="dispH"
            :style="cropLayerStyle"
          >
            <template v-for="(s, i) in shapes" :key="i">
              <rect
                v-if="s.tag === 'rect'" :x="s.x" :y="s.y" :width="s.w" :height="s.h"
                fill="none" :stroke="s.color" :stroke-width="s.width" stroke-linejoin="round"
              />
              <ellipse
                v-else-if="s.tag === 'ellipse'" :cx="s.cx" :cy="s.cy" :rx="Math.max(s.rx, 0.5)" :ry="Math.max(s.ry, 0.5)"
                fill="none" :stroke="s.color" :stroke-width="s.width"
              />
              <polyline
                v-else-if="s.tag === 'polyline'" :points="s.points"
                fill="none" :stroke="s.color" :stroke-width="s.width" stroke-linecap="round" stroke-linejoin="round"
              />
              <line
                v-else-if="s.tag === 'line'" :x1="s.x1" :y1="s.y1" :x2="s.x2" :y2="s.y2"
                :stroke="s.color" :stroke-width="s.width" stroke-linecap="round"
              />
            </template>
          </svg>
        </template>
        <div v-else-if="loadError" class="empty error">
          <p>无法显示该图片</p>
          <pre>{{ loadError }}</pre>
        </div>
        <div v-else-if="!loading" class="empty">
          <p>在主窗口打开图片后，右键选择「编辑」</p>
        </div>
        <div v-if="loading" class="loading">解码中…</div>

        <!-- 裁剪就绪提示：告诉用户框选完成后下一步做什么 -->
        <div v-if="edit.crop && tool !== 'crop' && natural.w" class="crop-toast">
          <span>裁剪 {{ Math.round(edit.crop.w) }} × {{ Math.round(edit.crop.h) }} 已就绪，保存时生效</span>
          <button class="ep-btn" @click="tool = 'crop'">重新框选</button>
          <button class="ep-btn" @click="edit.crop = null">清除</button>
        </div>
      </main>

      <!-- 右侧参数栏 -->
      <aside class="side">
        <fieldset class="ep-group" :disabled="!natural.w">
          <legend>裁剪</legend>
          <div class="ep-line">
            <button class="ep-btn" :class="{ on: tool === 'crop' }" @click="tool = tool === 'crop' ? 'pan' : 'crop'">
              {{ tool === 'crop' ? '框选中' : '框选' }}
            </button>
            <button class="ep-btn" :disabled="!edit.crop" @click="edit.crop = null">清除</button>
            <button class="ep-btn" @click="cropWhole">全图</button>
          </div>
          <div class="ep-nums">
            <label>X <input type="number" min="0" :value="edit.crop ? Math.round(edit.crop.x) : ''" :disabled="!edit.crop" @change="setCropField('x', +($event.target as HTMLInputElement).value)" /></label>
            <label>Y <input type="number" min="0" :value="edit.crop ? Math.round(edit.crop.y) : ''" :disabled="!edit.crop" @change="setCropField('y', +($event.target as HTMLInputElement).value)" /></label>
            <label>宽 <input type="number" min="1" :value="edit.crop ? Math.round(edit.crop.w) : ''" :disabled="!edit.crop" @change="setCropField('w', +($event.target as HTMLInputElement).value)" /></label>
            <label>高 <input type="number" min="1" :value="edit.crop ? Math.round(edit.crop.h) : ''" :disabled="!edit.crop" @change="setCropField('h', +($event.target as HTMLInputElement).value)" /></label>
          </div>
          <p v-if="edit.crop" class="ep-hint">已框选 {{ cropBase.w }} × {{ cropBase.h }}，保存时生效；旋转/镜像时框会跟着转</p>
          <p v-else class="ep-hint">点「框选」后在图上拖出裁剪区域，保存时生效</p>
        </fieldset>

        <fieldset class="ep-group" :disabled="!natural.w">
          <legend>改尺寸</legend>
          <div class="ep-nums">
            <label>宽 <input type="number" min="1" :value="resizeW" @change="onResizeW(+($event.target as HTMLInputElement).value)" /></label>
            <label>高 <input type="number" min="1" :value="resizeH" @change="onResizeH(+($event.target as HTMLInputElement).value)" /></label>
          </div>
          <div class="ep-line">
            <label class="ep-check"><input type="checkbox" :checked="resizeKeep" @change="onKeepToggle" /> 保持纵横比</label>
            <button class="ep-btn" :disabled="!resizeChanged" @click="edit.resize = null">重置</button>
          </div>
        </fieldset>

        <fieldset class="ep-group">
          <legend>输出</legend>
          <select v-model="editOutput.format" class="ep-select">
            <option v-for="f in FORMATS" :key="f.value" :value="f.value">{{ f.label }}</option>
          </select>
          <label v-if="editOutput.format === 'jpeg'" class="ep-quality">质量 {{ editOutput.quality }}
            <input v-model.number="editOutput.quality" type="range" min="1" max="100" />
          </label>
          <p v-else-if="editOutput.format === 'webp' || editOutput.format === 'avif'" class="ep-hint">该格式由当前编码器固定参数输出</p>
          <p v-if="editOutput.format !== 'original'" class="ep-hint">目标格式 ≠ 原格式时只能「另存为…」</p>
        </fieldset>
      </aside>
    </div>
  </div>
</template>

<style>
/* 编辑窗口布局：顶部工具栏 + 画布/侧栏两栏 */
.ew { display: flex; flex-direction: column; height: 100%; }
.ew .bar {
  display: flex; align-items: center; flex-wrap: wrap; gap: 2px;
  padding: 4px 8px;
  background: var(--bar);
  border-bottom: 1px solid var(--border);
  z-index: 2;
}
.ew .bar .grow { flex: 1; }
.ew .bar .btn-primary { padding: 5px 14px; margin-left: 6px; }
.ew .bar .btn-primary:disabled { opacity: 0.4; cursor: default; }
.ew .bar .btn-primary:disabled:hover { filter: none; }
.ew .bar .save-as { margin-left: 6px; }
.pct { min-width: 44px; text-align: center; font-size: 12px; color: var(--fg-muted); }

.ew .tool {
  background: none; border: none; color: var(--fg); cursor: pointer;
  font-size: 15px; line-height: 1; padding: 5px 8px; border-radius: 6px;
}
.ew .tool:hover { background: var(--hover); }
.ew .tool.active { color: var(--primary); background: var(--hover); }
.ew .tool:disabled { opacity: 0.35; cursor: default; }
.ew .tool:disabled:hover { background: none; }
.ew .sep { width: 1px; height: 16px; background: var(--border); margin: 0 5px; }

/* 标记颜色色板与线宽 */
.swatches { display: inline-flex; gap: 3px; margin: 0 4px; }
.swatch {
  width: 16px; height: 16px; border-radius: 50%;
  border: 2px solid transparent; cursor: pointer; padding: 0;
}
.swatch.on { border-color: var(--primary); }
.color-input {
  width: 26px; height: 22px; padding: 0; border: 1px solid var(--border);
  border-radius: 6px; background: none; cursor: pointer;
}
.w-select {
  background: none; border: 1px solid var(--border); border-radius: 6px;
  color: var(--fg); font-size: 12px; padding: 3px 4px; margin-left: 4px;
}
.w-select option { background: var(--bar); color: var(--fg); }

.body { display: flex; flex: 1; min-height: 0; }
.stage {
  flex: 1; position: relative; overflow: hidden;
  display: flex; align-items: center; justify-content: center;
  touch-action: none;
  background:
    repeating-conic-gradient(var(--check-a) 0% 25%, var(--check-b) 0% 50%) 50% / 24px 24px;
}
.stage.plain { background: var(--bg); }
.pic { display: block; will-change: transform; }

/* 裁剪层 / 标记层：与图片外接框对齐 */
.crop-layer { overflow: hidden; cursor: crosshair; touch-action: none; }
.mark-layer { cursor: crosshair; }
.mark-preview {
  position: absolute; left: 0; top: 0;
  pointer-events: none; overflow: visible;
}
/* 已就绪的裁剪框：非交互，虚线 + 弱遮罩，任何工具下都可见 */
.crop-layer.static { pointer-events: none; }
.crop-box.static {
  box-shadow: 0 0 0 9999px rgba(0, 0, 0, 0.35);
  outline: 1px dashed var(--primary);
  cursor: default;
}
/* 裁剪就绪提示条：画布底部居中 */
.crop-toast {
  position: absolute; left: 50%; bottom: 14px; transform: translateX(-50%);
  display: flex; align-items: center; gap: 10px;
  max-width: calc(100% - 24px);
  background: var(--overlay); border: 1px solid var(--border);
  border-radius: 8px; padding: 6px 12px;
  font-size: 12px; white-space: nowrap;
  backdrop-filter: blur(8px);
  z-index: 3;
}
.crop-box {
  position: absolute;
  box-shadow: 0 0 0 9999px rgba(0, 0, 0, 0.55);
  outline: 1px solid var(--primary);
  cursor: move;
  touch-action: none;
}
.crop-h {
  position: absolute;
  width: var(--hs, 10px);
  height: var(--hs, 10px);
  background: #fff;
  border: 1px solid var(--primary);
  border-radius: 50%;
  box-sizing: border-box;
  touch-action: none;
}
.crop-h.nw { left: 0; top: 0; transform: translate(-50%, -50%); cursor: nwse-resize; }
.crop-h.n  { left: 50%; top: 0; transform: translate(-50%, -50%); cursor: ns-resize; }
.crop-h.ne { left: 100%; top: 0; transform: translate(-50%, -50%); cursor: nesw-resize; }
.crop-h.e  { left: 100%; top: 50%; transform: translate(-50%, -50%); cursor: ew-resize; }
.crop-h.se { left: 100%; top: 100%; transform: translate(-50%, -50%); cursor: nwse-resize; }
.crop-h.s  { left: 50%; top: 100%; transform: translate(-50%, -50%); cursor: ns-resize; }
.crop-h.sw { left: 0; top: 100%; transform: translate(-50%, -50%); cursor: nesw-resize; }
.crop-h.w  { left: 0; top: 50%; transform: translate(-50%, -50%); cursor: ew-resize; }

.empty { color: var(--fg-muted); text-align: center; }
.empty.error pre { color: #cf6679; white-space: pre-wrap; max-width: 70vw; }
.loading {
  position: absolute; left: 50%; top: 50%; transform: translate(-50%, -50%);
  color: var(--fg-muted); font-size: 13px;
  background: var(--overlay); border: 1px solid var(--border);
  border-radius: 8px; padding: 8px 18px;
}

/* 右侧参数栏：沿用编辑面板的表单风格 */
.side {
  width: 270px; flex-shrink: 0;
  background: var(--bar);
  border-left: 1px solid var(--border);
  overflow-y: auto;
  padding: 10px 12px 16px;
}
.ep-group { border: 1px solid var(--border); border-radius: 8px; margin: 0 0 10px; padding: 8px 10px 10px; }
.ep-group legend { font-size: 12px; color: var(--fg-muted); padding: 0 6px; }
.ep-line { display: flex; align-items: center; gap: 8px; margin-top: 6px; }
.ep-nums { display: grid; grid-template-columns: 1fr 1fr; gap: 6px 10px; margin-top: 6px; }
.ep-nums label { display: flex; align-items: center; gap: 6px; font-size: 12px; color: var(--fg-muted); }
.ep-nums input {
  width: 100%; min-width: 0;
  background: none; border: 1px solid var(--border); border-radius: 6px;
  color: var(--fg); font-size: 12px; padding: 3px 6px;
}
.ep-nums input:disabled { opacity: 0.4; }
.ep-nums input::-webkit-outer-spin-button,
.ep-nums input::-webkit-inner-spin-button { -webkit-appearance: none; margin: 0; }
.ep-check { display: flex; align-items: center; gap: 5px; font-size: 12px; cursor: pointer; }
.ep-select {
  width: 100%; margin-top: 6px;
  background: none; border: 1px solid var(--border); border-radius: 6px;
  color: var(--fg); font-size: 12px; padding: 4px 6px;
}
.ep-select option { background: var(--bar); color: var(--fg); }
.ep-quality { display: block; margin-top: 8px; font-size: 12px; color: var(--fg-muted); }
.ep-quality input { width: 100%; margin-top: 4px; accent-color: var(--primary); }
.ep-hint { margin: 6px 0 0; font-size: 11px; color: var(--fg-muted); line-height: 1.5; }
.ep-btn.on { background: var(--primary); color: #fff; border-color: var(--primary); }
</style>
