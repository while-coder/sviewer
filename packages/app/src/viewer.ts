/**
 * 看图器前后端桥接：封装 Rust command 调用、图片源解析、格式判断。
 *
 * 显示策略（性能关键）：
 * - WebView 原生支持的格式（jpg/png/gif/webp/bmp/ico/svg/avif）直接走 asset 协议
 *   convertFileSrc，由 Chromium 解码，最快、最省内存；
 * - 其余格式（tiff 等）交给 Rust 用 image crate 解码成 PNG 再以 data URL 显示。
 *   HEIC/HEIF 优先 Rust 原生解码（Windows WIC / macOS Image I/O），失败回退
 *   heic-worker.ts 子线程的 libheif WASM；结果均以 WebP blob URL 显示。
 */
import { invoke } from '@tauri-apps/api/core'
import { convertFileSrc } from '@tauri-apps/api/core'

/** 一条 EXIF 信息：标签名 + 展示值。 */
export interface ExifEntry {
  tag: string
  value: string
}

/** 图片元信息，由 Rust read_image_info 返回。 */
export interface ImageInfo {
  path: string
  fileName: string
  /** 文件字节数 */
  size: number
  width: number
  height: number
  /** 格式名，如 "JPEG"、"PNG" */
  format: string
  exif: ExifEntry[]
}

/** WebView 可直接渲染的扩展名（小写，不含点）。jpe/jfif 是 JPEG 别名。 */
const WEB_NATIVE = new Set([
  'jpg', 'jpeg', 'jpe', 'jfif', 'png', 'gif', 'webp', 'bmp', 'ico', 'svg', 'avif',
])

/** 需要 libheif（WASM）在前端解码的扩展名。Rust 的 image crate 不支持 HEIC。hif 是富士的 HEIF 容器。 */
const HEIF_EXT = new Set(['heic', 'heif', 'hif'])

/** 取扩展名（小写，不含点）。 */
export function extOf(path: string): string {
  const i = path.lastIndexOf('.')
  return i >= 0 ? path.slice(i + 1).toLowerCase() : ''
}

/** 该格式能否由 WebView 直接显示。 */
export function isWebNative(path: string): boolean {
  return WEB_NATIVE.has(extOf(path))
}

/**
 * 解析出 <img> 可用的 src：
 * - 原生格式 → asset 协议 URL（不复制、不解码，最快）；
 * - HEIC/HEIF → heic-worker.ts 子线程 libheif WASM 解码，返回 WebP blob URL
 *   （带 LRU 缓存，来回切换不重复解码）；
 * - 其它格式 → Rust 解码为 PNG data URL。
 */
export async function resolveImageSrc(path: string): Promise<string> {
  if (isWebNative(path)) {
    return convertFileSrc(path)
  }
  if (HEIF_EXT.has(extOf(path))) {
    return decodeHeic(path)
  }
  return invoke<string>('decode_to_png', { path })
}

// ── HEIC 子线程解码 ──────────────────────────────────────

/** HEIC 解码结果缓存上限。每张解码后约 1~3MB WebP，3 张足够覆盖来回切换。 */
const HEIC_CACHE_MAX = 3
/** path → 解码后的 WebP blob URL，Map 保持插入序以实现 LRU。 */
const heicCache = new Map<string, string>()

/** 解码 Worker 惰性创建、常驻复用；HEIC 用不到时这个大 chunk 不会进主包。 */
let heifWorker: Worker | null = null
let heifReqId = 0
const heifPending = new Map<number, { resolve: (b: Blob) => void; reject: (e: unknown) => void }>()

function getHeifWorker(): Worker {
  if (heifWorker) return heifWorker
  heifWorker = new Worker(new URL('./heic-worker.ts', import.meta.url), { type: 'module' })
  heifWorker.onmessage = (e: MessageEvent<{ id: number; ok: boolean; blob?: Blob; error?: string }>) => {
    const { id } = e.data
    const p = heifPending.get(id)
    if (!p) return
    heifPending.delete(id)
    if (e.data.ok && e.data.blob) p.resolve(e.data.blob)
    else p.reject(new Error(e.data.error ?? 'HEIC 解码失败'))
  }
  heifWorker.onerror = (e) => {
    // Worker 自身崩溃（如 WASM 加载失败）：拒绝所有在途请求，置空以便下次重建
    for (const p of heifPending.values()) p.reject(new Error(e.message || 'HEIC 解码 Worker 崩溃'))
    heifPending.clear()
    heifWorker = null
  }
  return heifWorker
}

function cacheHeic(path: string, url: string) {
  const old = heicCache.get(path)
  if (old === url) return
  if (old) URL.revokeObjectURL(old)
  heicCache.delete(path)
  heicCache.set(path, url)
  // 当前正在显示的那张是最新插入的，被淘汰的必然不是它，revoke 是安全的
  while (heicCache.size > HEIC_CACHE_MAX) {
    const [k, v] = heicCache.entries().next().value!
    heicCache.delete(k)
    URL.revokeObjectURL(v)
  }
}

/**
 * 解码 HEIC/HEIF，返回 WebP blob URL；命中缓存时零开销。
 * 链路：Rust 原生解码（Windows WIC / macOS Image I/O，最快）
 *   → 失败则 libheif WASM 兜底（heic-worker 子线程）。
 * 编码一律在子线程完成，主线程不卡顿。
 */
async function decodeHeic(path: string): Promise<string> {
  const cached = heicCache.get(path)
  if (cached) {
    // 触碰一下，刷新 LRU 顺序
    heicCache.delete(path)
    heicCache.set(path, cached)
    return cached
  }

  let blob: Blob | null = null
  try {
    blob = await decodeHeicNative(path)
  } catch (e) {
    console.info('系统原生解码不可用，回退 libheif WASM：', e)
  }
  blob ??= await decodeHeicWasm(path)

  const url = URL.createObjectURL(blob)
  cacheHeic(path, url)
  return url
}

/**
 * Rust 原生解码：得到 [宽 u32 LE][高 u32 LE][RGBA8...]，
 * 把像素部分转移（transfer）给 worker 编码成 WebP。
 */
async function decodeHeicNative(path: string): Promise<Blob> {
  const buf = await invoke<ArrayBuffer>('decode_heic', { path })
  const dv = new DataView(buf)
  const width = dv.getUint32(0, true)
  const height = dv.getUint32(4, true)
  if (width === 0 || height === 0 || buf.byteLength < 8 + width * height * 4) {
    throw new Error('原生解码返回数据异常')
  }
  const pixels = buf.slice(8)
  const id = ++heifReqId
  return new Promise<Blob>((resolve, reject) => {
    heifPending.set(id, { resolve, reject })
    getHeifWorker().postMessage({ id, kind: 'rgba', width, height, buf: pixels }, [pixels])
  })
}

/** libheif WASM 兜底：把整份文件交给 worker 解码。 */
async function decodeHeicWasm(path: string): Promise<Blob> {
  const id = ++heifReqId
  return new Promise<Blob>((resolve, reject) => {
    heifPending.set(id, { resolve, reject })
    // convertFileSrc 依赖 window.__TAURI_INTERNALS__，须在主线程转好再传给 Worker
    getHeifWorker().postMessage({ id, kind: 'wasm', url: convertFileSrc(path) })
  })
}

/** 启动时（双击文件 / 命令行）传入的待打开文件，取一次后清空；无则返回 null。 */
export function getLaunchFile(): Promise<string | null> {
  return invoke<string | null>('get_launch_file')
}

/** 同目录下所有受支持的图片（已排序），用于左右切换。 */
export function listSiblings(path: string): Promise<string[]> {
  return invoke<string[]>('list_dir_images', { path })
}

/** 另存为目标格式：original 为原样复制，其余由 Rust 重编码。 */
export type SaveFormat =
  | 'original'
  | 'jpeg'
  | 'png'
  | 'webp'
  | 'bmp'
  | 'tiff'
  | 'gif'
  | 'ico'
  | 'tga'
  | 'ppm'
  | 'qoi'
  | 'ff'
  | 'avif'

/** 把图片另存到目标路径（original 为原样复制，其余格式由 Rust 重编码）。 */
export function saveImageAs(src: string, dest: string, format: SaveFormat): Promise<void> {
  return invoke('save_image_as', { src, dest, format })
}

/**
 * 把旋转/镜像写回原图（原地覆盖）。
 * rotation：0/90/180/270（顺时针）；flip：水平镜像。
 * 仅支持可编码格式（heic/svg/gif 由调用方禁用）。
 */
export function applyTransform(path: string, rotation: number, flip: boolean): Promise<void> {
  return invoke('apply_transform', { path, rotation, flip })
}

// ── 编辑 / 转换 ─────────────────────────────────────────

/** 裁剪矩形：显示空间（EXIF 归一化 + 旋转/镜像之后）的像素坐标，左上原点。 */
export interface CropRect {
  x: number
  y: number
  w: number
  h: number
}

/** 一条标记笔画：显示空间（EXIF 归一化 + 旋转/镜像后，裁剪前）的像素坐标，保存时由 Rust 烘焙进图片。 */
export interface MarkShape {
  kind: 'rect' | 'ellipse' | 'arrow' | 'pen'
  /** "#rrggbb" */
  color: string
  /** 线宽（显示空间像素） */
  width: number
  /** rect/ellipse/arrow 为对角两点 [起点, 终点]，pen 为折线点集 */
  pts: [number, number][]
}

/** 一次编辑会话参数（对应 Rust ImageEdits；null 字段 = 不做该步）。 */
export interface ImageEdits {
  rotation: number
  flip: boolean
  crop: CropRect | null
  /** 裁剪后的目标宽高 */
  resize: [number, number] | null
  /** 编码质量 1~100，仅 JPEG 消费 */
  quality: number | null
  /** 标记笔画（编辑窗口绘制） */
  marks?: MarkShape[] | null
}

/** 保存结果：实际落盘路径 + 文件字节数。 */
export interface SaveOutcome {
  dest: string
  size: number
}

/** 把编辑（旋转/镜像/裁剪/改尺寸）烘焙后写回原图。 */
export function saveEditsTo(path: string, edits: ImageEdits): Promise<SaveOutcome> {
  return invoke('save_edits', { path, edits })
}

/** 解码 src → 编辑管线 → 按格式+质量写入 dest（另存为 / 批量转换共用）。 */
export function encodeTo(
  src: string,
  dest: string,
  format: SaveFormat,
  quality: number | null,
  edits: ImageEdits | null,
): Promise<SaveOutcome> {
  return invoke('encode_image', { src, dest, format, quality, edits })
}

/** 目标路径已存在时自动加 -2/-3… 后缀，返回不冲突的路径。 */
export function uniqueDest(dest: string): Promise<string> {
  return invoke('unique_dest', { dest })
}

/** 解码为缩略图 PNG data URL（最长边 ≤ maxPx），非 web 原生格式的列表缩略图用。 */
export function decodeThumb(path: string, maxPx: number): Promise<string> {
  return invoke('decode_thumb', { path, maxPx })
}

/** 可直接改写原图的扩展名（heic 无编码器、svg 矢量、gif 动图会丢帧）。jpe/jfif 按 JPEG 写回。 */
export const EDITABLE_EXT = new Set([
  'jpg', 'jpeg', 'jpe', 'jfif', 'png', 'webp', 'bmp', 'tiff', 'tif', 'avif', 'tga', 'qoi', 'exr',
])

/** 该图片能否「保存到原图」（旋转/裁剪等编辑写回）。 */
export function extSupportsEdit(path: string): boolean {
  return EDITABLE_EXT.has(extOf(path))
}

/** 读取图片元信息（尺寸 / 格式 / EXIF）。 */
export function readImageInfo(path: string): Promise<ImageInfo> {
  return invoke<ImageInfo>('read_image_info', { path })
}

/** 人类可读的文件大小。 */
export function humanSize(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`
  const units = ['KB', 'MB', 'GB']
  let v = bytes / 1024
  let i = 0
  while (v >= 1024 && i < units.length - 1) {
    v /= 1024
    i++
  }
  return `${v.toFixed(2)} ${units[i]}`
}

/** EXIF 标签名 → 中文。未命中的保留原名。 */
const EXIF_ZH: Record<string, string> = {
  Make: '制造商',
  Model: '相机型号',
  Software: '软件',
  Orientation: '方向',
  XResolution: 'X 分辨率',
  YResolution: 'Y 分辨率',
  ResolutionUnit: '分辨率单位',
  DateTime: '修改时间',
  DateTimeOriginal: '拍摄时间',
  DateTimeDigitized: '数字化时间',
  OffsetTime: '时区',
  OffsetTimeOriginal: '拍摄时区',
  OffsetTimeDigitized: '数字化时区',
  SubSecTime: '亚秒',
  SubSecTimeOriginal: '拍摄亚秒',
  SubSecTimeDigitized: '数字化亚秒',
  ExposureTime: '曝光时间',
  ShutterSpeedValue: '快门速度',
  FNumber: '光圈值',
  ApertureValue: '光圈',
  MaxApertureValue: '最大光圈',
  BrightnessValue: '亮度',
  ExposureBiasValue: '曝光补偿',
  ExposureProgram: '曝光程序',
  ExposureMode: '曝光模式',
  PhotographicSensitivity: '感光度',
  ISOSpeedRatings: '感光度',
  ISOSpeed: '感光度',
  MeteringMode: '测光模式',
  Flash: '闪光灯',
  FocalLength: '焦距',
  FocalLengthIn35mmFilm: '等效焦距',
  LensMake: '镜头制造商',
  LensModel: '镜头型号',
  LensSpecification: '镜头规格',
  LensSerialNumber: '镜头序列号',
  WhiteBalance: '白平衡',
  ColorSpace: '色彩空间',
  SceneCaptureType: '场景类型',
  SceneType: '场景类型',
  SensingMethod: '感应方式',
  Contrast: '对比度',
  Saturation: '饱和度',
  Sharpness: '锐度',
  SubjectDistance: '对焦距离',
  SubjectArea: '对焦区域',
  ExifVersion: 'EXIF 版本',
  FlashpixVersion: 'FlashPix 版本',
  PixelXDimension: '有效宽度',
  PixelYDimension: '有效高度',
  BodySerialNumber: '机身序列号',
  SerialNumber: '机身序列号',
  Artist: '作者',
  Copyright: '版权',
  ImageDescription: '描述',
  UserComment: '备注',
  GPSLatitudeRef: 'GPS 纬度参考',
  GPSLatitude: 'GPS 纬度',
  GPSLongitudeRef: 'GPS 经度参考',
  GPSLongitude: 'GPS 经度',
  GPSAltitudeRef: 'GPS 高度参考',
  GPSAltitude: 'GPS 高度',
  GPSTimeStamp: 'GPS 时间',
  GPSDateStamp: 'GPS 日期',
}

/** EXIF 标签的中文显示名。 */
export function exifLabel(tag: string): string {
  return EXIF_ZH[tag] ?? tag
}

/** 去掉 libheif/kamadak 展示值两端的引号。 */
export function cleanExifValue(value: string): string {
  return value.replace(/^"(.*)"$/s, '$1')
}

/** 一条常用信息（标签 + 中文说明 + 值）。 */
export interface CommonEntry {
  label: string
  value: string
}

/** 从完整 EXIF 里挑常用拍摄信息（按优先级取第一个命中的 tag）。 */
export function pickCommonInfo(exif: ExifEntry[]): CommonEntry[] {
  const find = (...tags: string[]) => {
    for (const tag of tags) {
      const e = exif.find((x) => x.tag === tag)
      if (e && e.value.trim()) return cleanExifValue(e.value)
    }
    return null
  }
  const out: CommonEntry[] = []
  const push = (label: string, ...tags: string[]) => {
    const v = find(...tags)
    if (v) out.push({ label, value: v })
  }
  push('相机', 'Model')
  push('镜头', 'LensModel')
  push('拍摄时间', 'DateTimeOriginal', 'DateTime')
  push('感光度', 'PhotographicSensitivity', 'ISOSpeedRatings', 'ISOSpeed')
  push('光圈', 'FNumber')
  push('快门', 'ExposureTime')
  push('焦距', 'FocalLength')
  push('等效焦距', 'FocalLengthIn35mmFilm')
  push('曝光补偿', 'ExposureBiasValue')
  push('白平衡', 'WhiteBalance')
  push('闪光灯', 'Flash')
  push('软件', 'Software')
  return out
}
