/**
 * 前端图片解码：显示策略（性能关键）——
 * - WebView 原生支持的格式（jpg/png/gif/webp/bmp/ico/svg/avif）直接走 asset 协议
 *   convertFileSrc，由 Chromium 解码，最快、最省内存；
 * - HEIC/HEIF 优先 Rust 原生解码（Windows WIC / macOS Image I/O），失败回退
 *   libheif WASM 子线程解码；其余格式（tiff 等）由 image crate 解码。
 *   两者都产出裸像素，在 decode-worker.ts 子线程转成 ImageBitmap，主窗口用
 *   <canvas> 直接绘制——省掉 PNG/WebP 重编码与二次解码。
 *   编辑窗口等必须用 <img> 的场景走 resolveImageSrc（重编码成 WebP/PNG）。
 *
 * 依赖方向约束：本文件不得 import settings / geo 等应用层模块。
 */
import { convertFileSrc, invoke } from '@tauri-apps/api/core'
import { extOf, isWebNative, HEIF_EXT } from '../formats/formats'
import { decodeToPng } from '../bridge/bridge'
import type { ImageSource } from '../types'

/**
 * 解析主窗口显示用的图片源：
 * - 原生格式 → asset 协议 URL（不复制、不解码，最快）；
 * - 其余全部 → 解码为 ImageBitmap 交 canvas（带 LRU 缓存，来回切换不重复解码）。
 */
export async function resolveImage(path: string): Promise<ImageSource> {
  if (isWebNative(path)) {
    return { kind: 'url', src: convertFileSrc(path) }
  }
  return { kind: 'bitmap', bitmap: await decodeToBitmap(path) }
}

/**
 * 解析出 <img> 可用的 src（编辑窗口等不能用 canvas 的场景）。
 * HEIC/HEIF 需重编码成 WebP blob，比 resolveImage 的位图路径慢，
 * 主窗口显示一律走 resolveImage。
 */
export async function resolveImageSrc(path: string): Promise<string> {
  if (isWebNative(path)) {
    return convertFileSrc(path)
  }
  if (HEIF_EXT.has(extOf(path))) {
    return decodeHeicToBlob(path)
  }
  return decodeToPng(path)
}

/**
 * 后台预载一张图，翻到它时直接出图：
 * - web 原生格式 → 喂给 <img> 预热 WebView 缓存与系统文件缓存；
 * - 其余格式 → 提前走完整解码链，位图落进 LRU 缓存。
 * 失败静默——预载只是优化，真翻过去时正常走报错路径。
 */
export function preloadImage(path: string): void {
  if (isWebNative(path)) {
    const img = new Image()
    img.src = convertFileSrc(path)
    return
  }
  decodeToBitmap(path).catch((e) => console.info('预载失败（不影响使用）', path, e))
}

// ── 位图子线程解码（HEIC 与 tiff 等非原生格式共用）──────────

/** 位图缓存张数上限。预载邻居（preloadImage）后「当前 + 前后各一」能全命中。 */
const BITMAP_CACHE_MAX = 3
/** 位图缓存像素总量上限（72M 像素 ≈ 288MB RGBA）。超大图（48MP）按此先淘汰，防内存失控。 */
const BITMAP_CACHE_MAX_PIXELS = 72_000_000
/** path → 解码后的 ImageBitmap，Map 保持插入序以实现 LRU。 */
const bitmapCache = new Map<string, ImageBitmap>()
/** 缓存中的像素总量，配合 BITMAP_CACHE_MAX_PIXELS 淘汰。 */
let bitmapPixels = 0

/** worker 解码结果：位图（主窗口 canvas）或 blob（编辑窗口 <img>），按请求种类取其一。 */
interface WorkerResult {
  bitmap?: ImageBitmap
  blob?: Blob
}

/** 发给 worker 的请求（id 由 requestWorker 统一分配）。 */
type WorkerRequest =
  | { kind: 'bitmap-rgba' | 'blob-rgba'; width: number; height: number; buf: ArrayBuffer }
  | { kind: 'bitmap-wasm' | 'blob-wasm'; url: string }

/** 解码 Worker 惰性创建、常驻复用；HEIC 用不到时这个大 chunk 不会进主包。 */
let worker: Worker | null = null
let reqId = 0
const pending = new Map<number, { resolve: (r: WorkerResult) => void; reject: (e: unknown) => void }>()

function getWorker(): Worker {
  if (worker) return worker
  worker = new Worker(new URL('./decode-worker.ts', import.meta.url), { type: 'module' })
  worker.onmessage = (e: MessageEvent<{ id: number; ok: boolean; error?: string } & WorkerResult>) => {
    const { id } = e.data
    const p = pending.get(id)
    if (!p) return
    pending.delete(id)
    if (e.data.ok && (e.data.bitmap || e.data.blob)) p.resolve({ bitmap: e.data.bitmap, blob: e.data.blob })
    else p.reject(new Error(e.data.error ?? '解码失败'))
  }
  worker.onerror = (e) => {
    // Worker 自身崩溃（如 WASM 加载失败）：拒绝所有在途请求，置空以便下次重建
    for (const p of pending.values()) p.reject(new Error(e.message || '解码 Worker 崩溃'))
    pending.clear()
    worker = null
  }
  return worker
}

/** 发一个请求给 worker 并等待结果；transfer 列表用于零拷贝转移像素。 */
function requestWorker(req: WorkerRequest, transfer: Transferable[] = []): Promise<WorkerResult> {
  const id = ++reqId
  return new Promise((resolve, reject) => {
    pending.set(id, { resolve, reject })
    getWorker().postMessage({ ...req, id }, transfer)
  })
}

function cacheBitmap(path: string, bitmap: ImageBitmap) {
  const old = bitmapCache.get(path)
  if (old === bitmap) return
  if (old) {
    old.close()
    bitmapPixels -= old.width * old.height
  }
  bitmapCache.delete(path)
  bitmapCache.set(path, bitmap)
  bitmapPixels += bitmap.width * bitmap.height
  // 被淘汰的位图即使正显示在 canvas 上也没关系——像素已画进 canvas，close 不影响已绘内容
  while (bitmapCache.size > BITMAP_CACHE_MAX || (bitmapCache.size > 1 && bitmapPixels > BITMAP_CACHE_MAX_PIXELS)) {
    const [k, v] = bitmapCache.entries().next().value!
    bitmapCache.delete(k)
    bitmapPixels -= v.width * v.height
    v.close()
  }
}

/** 解开 Rust decode_heic 返回的 [宽 u32 LE][高 u32 LE][RGBA8...]，校验并切出像素部分。 */
function unpackNativePixels(buf: ArrayBuffer): { width: number; height: number; pixels: ArrayBuffer } {
  const width = new DataView(buf).getUint32(0, true)
  const height = new DataView(buf).getUint32(4, true)
  if (width === 0 || height === 0 || buf.byteLength < 8 + width * height * 4) {
    throw new Error('原生解码返回数据异常')
  }
  return { width, height, pixels: buf.slice(8) }
}

/**
 * 解码任意非 web 原生格式为 ImageBitmap；命中缓存时零开销。
 * - HEIC/HEIF：Rust 原生解码（Windows WIC / macOS Image I/O，最快）
 *   → 失败回退 libheif WASM（decode-worker 子线程）；
 * - 其余（tiff/exr/tga…）：Rust image crate 解码裸像素。
 * 像素打包成 ImageBitmap 一律在子线程完成，主线程不卡顿。
 */
async function decodeToBitmap(path: string): Promise<ImageBitmap> {
  const cached = bitmapCache.get(path)
  if (cached) {
    // 触碰一下，刷新 LRU 顺序
    bitmapCache.delete(path)
    bitmapCache.set(path, cached)
    return cached
  }
  const bitmap = HEIF_EXT.has(extOf(path)) ? await decodeHeic(path) : await decodeViaImageCrate(path)
  cacheBitmap(path, bitmap)
  return bitmap
}

/** HEIC 链路：Rust 原生解码最快，失败（Linux / 缺 HEIF 扩展）回退 libheif WASM。 */
async function decodeHeic(path: string): Promise<ImageBitmap> {
  try {
    return await decodeHeicNative(path)
  } catch (e) {
    console.info('系统原生解码不可用，回退 libheif WASM：', e)
  }
  return decodeHeicWasm(path)
}

/** Rust 原生解码 HEIC → worker 里包成 ImageBitmap。 */
async function decodeHeicNative(path: string): Promise<ImageBitmap> {
  return rawPixelsToBitmap(await invokeHeic(path))
}

/** image crate 解码（tiff/exr/tga…）→ worker 里包成 ImageBitmap。 */
async function decodeViaImageCrate(path: string): Promise<ImageBitmap> {
  return rawPixelsToBitmap(await invoke<ArrayBuffer>('decode_raw', { path }))
}

/** 「头 + RGBA8」裸像素 → worker 包成 ImageBitmap（transfer 转移回主线程，零拷贝）。 */
async function rawPixelsToBitmap(buf: ArrayBuffer): Promise<ImageBitmap> {
  const { width, height, pixels } = unpackNativePixels(buf)
  const { bitmap } = await requestWorker({ kind: 'bitmap-rgba', width, height, buf: pixels }, [pixels])
  return bitmap!
}

/** libheif WASM 兜底：整份文件交给 worker 解码成 ImageBitmap。 */
async function decodeHeicWasm(path: string): Promise<ImageBitmap> {
  // convertFileSrc 依赖 window.__TAURI_INTERNALS__，须在主线程转好再传给 Worker
  const { bitmap } = await requestWorker({ kind: 'bitmap-wasm', url: convertFileSrc(path) })
  return bitmap!
}

/**
 * HEIC → WebP blob URL，给必须用 <img> 的场景（编辑窗口）。
 * 比位图路径多一次 WebP 重编码（约零点几秒），主窗口显示一律走 resolveImage。
 */
async function decodeHeicToBlob(path: string): Promise<string> {
  let result: WorkerResult | null = null
  try {
    const { width, height, pixels } = unpackNativePixels(await invokeHeic(path))
    result = await requestWorker({ kind: 'blob-rgba', width, height, buf: pixels }, [pixels])
  } catch (e) {
    console.info('系统原生解码不可用，回退 libheif WASM：', e)
  }
  result ??= await requestWorker({ kind: 'blob-wasm', url: convertFileSrc(path) })
  return URL.createObjectURL(result.blob!)
}

/** 原生解码 HEIC（返回 8 字节头 + RGBA8 裸像素的零序列化响应）。 */
function invokeHeic(path: string): Promise<ArrayBuffer> {
  return invoke<ArrayBuffer>('decode_heic', { path })
}
