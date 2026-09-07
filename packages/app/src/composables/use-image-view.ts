/**
 * 图片视图变换（主窗口与编辑窗口共用）：
 * 适应窗口 / 1:1 / 以锚点缩放 / 拖拽平移 / 双击切换 / ResizeObserver 重适应。
 *
 * 统一用 transform 实现：view.scale 永远等于真实缩放比例，
 * 「适应窗口」就是 scale = min(stage/图片, 1)，百分比显示与实际始终一致。
 * transform 从右往左应用：先绕图片中心旋转/镜像，再以显示区左上角为锚点
 * 缩放、平移——view.x/y 直接对应旋转后外接框（dispW×dispH）的位置。
 *
 * imgStyle 不含 cursor：主窗口在 <img>/canvas 上自带 grab 光标，编辑窗口
 * 的光标随工具切换绑在 stage 上，由调用方各自处理。
 */
import { computed, onMounted, onUnmounted, reactive, type Ref } from 'vue'

export interface UseImageViewOptions {
  /** 图片舞台元素（尺寸变化时适应模式重算缩放） */
  stageEl: Ref<HTMLElement | null>
  /** 当前顺时针旋转角度（0/90/180/270） */
  rotation: () => number
  /** 是否水平镜像 */
  flip: () => boolean
  /** 图片就绪且处于适应模式时的默认视图（主窗口读设置，编辑窗口恒适应） */
  defaultView: () => 'fit' | 'actual'
  /** 额外的拖拽准入（编辑窗口仅 pan 工具可拖）；返回 false 时不启动拖拽 */
  canDrag?: (e: PointerEvent) => boolean
  /** 拖拽开始前回调（此时 fit 尚未解除，主窗口用它记双击判断基准） */
  onDragStart?: () => void
}

export function useImageView(options: UseImageViewOptions) {
  const { stageEl } = options

  const view = reactive({ scale: 1, x: 0, y: 0, fit: true })
  /** 图片固有尺寸（@load / 位图就绪时记录） */
  const natural = reactive({ w: 0, h: 0 })
  // 旋转 90/270 后显示宽高互换
  const swapped = computed(() => options.rotation() % 180 !== 0)
  // 旋转后的显示尺寸
  const dispW = computed(() => (swapped.value ? natural.h : natural.w))
  const dispH = computed(() => (swapped.value ? natural.w : natural.h))

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
    const r = el?.getBoundingClientRect()
    setScale(1, r ? r.width / 2 : undefined, r ? r.height / 2 : undefined) // 以画布中心为锚点切 1:1
  }

  function onWheel(e: WheelEvent) {
    e.preventDefault()
    const el = stageEl.value
    if (!el) return
    const r = el.getBoundingClientRect()
    const factor = e.deltaY < 0 ? 1.12 : 1 / 1.12
    setScale(view.scale * factor, e.clientX - r.left, e.clientY - r.top)
  }

  /** 适应 ↔ 1:1（双击 / 工具按钮）。 */
  function toggleFit() {
    if (view.fit) actualSize()
    else fitView()
  }

  /** 图片固有尺寸就绪（<img> onload / HEIC 位图解码完）：按默认视图重算。 */
  function applyNaturalSize(w: number, h: number) {
    natural.w = w
    natural.h = h
    if (view.fit) {
      if (options.defaultView() === 'actual') actualSize()
      else fitView()
    }
  }

  // 拖拽平移：Pointer Events + 指针捕获，光标移出窗口/划过信息面板也不会断
  const drag = reactive({ on: false, sx: 0, sy: 0, ox: 0, oy: 0 })
  function onPointerDown(e: PointerEvent) {
    if (options.canDrag && !options.canDrag(e)) return
    if (!e.isPrimary || e.button !== 0 || !natural.w || !natural.h) return
    options.onDragStart?.()
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

  /** <img> / <canvas> 的定位样式（固有尺寸未知时 CSS contain 兜底，避免闪跳）。不含 cursor。 */
  const imgStyle = computed(() => {
    if (!natural.w || !natural.h) {
      return { maxWidth: '100%', maxHeight: '100%', objectFit: 'contain' as const }
    }
    return {
      // 绝对定位脱离 flex 居中，平移锚点才与坐标计算一致
      position: 'absolute' as const,
      top: '0',
      left: '0',
      transform:
        `translate(${view.x}px, ${view.y}px) scale(${view.scale})` +
        ` translate(${dispW.value / 2}px, ${dispH.value / 2}px)` +
        ` scaleX(${options.flip() ? -1 : 1}) rotate(${options.rotation()}deg)` +
        ` translate(${-natural.w / 2}px, ${-natural.h / 2}px)`,
      transformOrigin: '0 0',
    }
  })

  // 窗口/画布尺寸变化时，适应模式下重新计算缩放
  let resizeObserver: ResizeObserver | null = null
  onMounted(() => {
    resizeObserver = new ResizeObserver(() => {
      if (view.fit) fitView()
    })
    if (stageEl.value) resizeObserver.observe(stageEl.value)
  })
  onUnmounted(() => resizeObserver?.disconnect())

  return {
    view,
    natural,
    swapped,
    dispW,
    dispH,
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
    imgStyle,
  }
}
