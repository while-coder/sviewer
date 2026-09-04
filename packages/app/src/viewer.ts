/**
 * 看图器前后端桥接：封装 Rust command 调用、图片源解析、格式判断。
 *
 * 显示策略（性能关键）：
 * - WebView 原生支持的格式（jpg/png/gif/webp/bmp/ico/svg/avif）直接走 asset 协议
 *   convertFileSrc，由 Chromium 解码，最快、最省内存；
 * - HEIC/HEIF 优先 Rust 原生解码（Windows WIC / macOS Image I/O），失败回退
 *   libheif WASM 子线程解码；其余格式（tiff 等）由 image crate 解码。
 *   两者都产出裸像素，在 heic-worker.ts 子线程转成 ImageBitmap，主窗口用
 *   <canvas> 直接绘制——省掉 PNG/WebP 重编码与二次解码。
 *   编辑窗口等必须用 <img> 的场景走 resolveImageSrc（重编码成 WebP/PNG）；
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

/** 图片源：url 交给 <img>；bitmap 交给 <canvas> drawImage。 */
export type ImageSource =
  | { kind: 'url'; src: string }
  | { kind: 'bitmap'; bitmap: ImageBitmap }

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
  return invoke<string>('decode_to_png', { path })
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
interface HeifResult {
  bitmap?: ImageBitmap
  blob?: Blob
}

/** 发给 worker 的请求（id 由 requestWorker 统一分配）。 */
type HeifRequest =
  | { kind: 'bitmap-rgba' | 'blob-rgba'; width: number; height: number; buf: ArrayBuffer }
  | { kind: 'bitmap-wasm' | 'blob-wasm'; url: string }

/** 解码 Worker 惰性创建、常驻复用；HEIC 用不到时这个大 chunk 不会进主包。 */
let heifWorker: Worker | null = null
let heifReqId = 0
const heifPending = new Map<number, { resolve: (r: HeifResult) => void; reject: (e: unknown) => void }>()

function getHeifWorker(): Worker {
  if (heifWorker) return heifWorker
  heifWorker = new Worker(new URL('./heic-worker.ts', import.meta.url), { type: 'module' })
  heifWorker.onmessage = (e: MessageEvent<{ id: number; ok: boolean; error?: string } & HeifResult>) => {
    const { id } = e.data
    const p = heifPending.get(id)
    if (!p) return
    heifPending.delete(id)
    if (e.data.ok && (e.data.bitmap || e.data.blob)) p.resolve({ bitmap: e.data.bitmap, blob: e.data.blob })
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

/** 发一个请求给 worker 并等待结果；transfer 列表用于零拷贝转移像素。 */
function requestWorker(req: HeifRequest, transfer: Transferable[] = []): Promise<HeifResult> {
  const id = ++heifReqId
  return new Promise((resolve, reject) => {
    heifPending.set(id, { resolve, reject })
    getHeifWorker().postMessage({ ...req, id }, transfer)
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
 *   → 失败回退 libheif WASM（heic-worker 子线程）；
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
  return rawPixelsToBitmap(await invoke<ArrayBuffer>('decode_heic', { path }))
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
  let result: HeifResult | null = null
  try {
    const { width, height, pixels } = unpackNativePixels(await invoke<ArrayBuffer>('decode_heic', { path }))
    result = await requestWorker({ kind: 'blob-rgba', width, height, buf: pixels }, [pixels])
  } catch (e) {
    console.info('系统原生解码不可用，回退 libheif WASM：', e)
  }
  result ??= await requestWorker({ kind: 'blob-wasm', url: convertFileSrc(path) })
  return URL.createObjectURL(result.blob!)
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
  // GPS 扩展字段（kamadak-exif 的 GPS 标签组）
  GPSVersionID: 'GPS 版本',
  GPSMapDatum: 'GPS 地图基准',
  GPSPositioningError: 'GPS 定位误差',
  GPSProcessingMethod: 'GPS 处理方法',
  GPSAreaInformation: 'GPS 区域信息',
  GPSDifferential: 'GPS 差分修正',
  GPSDOP: 'GPS 精度',
  GPSSpeedRef: 'GPS 速度单位',
  GPSSpeed: 'GPS 速度',
  GPSTrackRef: 'GPS 移动方向参考',
  GPSTrack: 'GPS 移动方向',
  GPSImgDirectionRef: 'GPS 朝向参考',
  GPSImgDirection: 'GPS 朝向',
  GPSDestLatitudeRef: 'GPS 目的地纬度参考',
  GPSDestLatitude: 'GPS 目的地纬度',
  GPSDestLongitudeRef: 'GPS 目的地经度参考',
  GPSDestLongitude: 'GPS 目的地经度',
  GPSDestBearingRef: 'GPS 目地方向参考',
  GPSDestBearing: 'GPS 目地方向',
  GPSDestDistanceRef: 'GPS 目的地距离单位',
  GPSDestDistance: 'GPS 目的地距离',
  // iPhone 常见的苹果扩展 / 复合图像标签
  CompositeImage: '复合图像',
  SourceImageNumberOfCompositeImage: '复合图像源数量',
  SourceExposureTimesOfCompositeImage: '复合图像曝光时间',
  CameraOwnerName: '相机所有者',
  ImageUniqueID: '图像唯一 ID',
  OwnerName: '所有者',
}

/** EXIF 标签的中文显示名。 */
export function exifLabel(tag: string): string {
  return EXIF_ZH[tag] ?? tag
}

/** 去掉 libheif/kamadak 展示值两端的引号。 */
export function cleanExifValue(value: string): string {
  return value.replace(/^"(.*)"$/s, '$1')
}

/** 解析单个 GPS 坐标值：兼容 kamadak 的有理数展示 "36/1, 6/1, 1230/100"
 *  与度分秒展示 `36° 6' 12.30"` 两种格式，返回十进制度。 */
function parseCoord(value: string): number | null {
  const parts = [...value.matchAll(/(\d+(?:\.\d+)?)(?:\/(\d+(?:\.\d+)?))?/g)]
  if (parts.length === 0) return null
  const nums = parts.slice(0, 3).map((m) => {
    const n = parseFloat(m[1])
    return m[2] ? n / parseFloat(m[2]) : n
  })
  return nums[0] + (nums[1] ?? 0) / 60 + (nums[2] ?? 0) / 3600
}

/** 从 EXIF 里提取 GPS 经纬度（十进制度，WGS-84）；无 GPS 或解析失败返回 null。 */
export function parseGpsCoord(exif: ExifEntry[]): { lat: number; lng: number } | null {
  const find = (tag: string) => {
    const e = exif.find((x) => x.tag === tag)
    return e ? cleanExifValue(e.value) : null
  }
  const latRaw = find('GPSLatitude')
  const lngRaw = find('GPSLongitude')
  if (!latRaw || !lngRaw) return null
  let lat = parseCoord(latRaw)
  let lng = parseCoord(lngRaw)
  if (lat == null || lng == null) return null
  // 南纬/西经为负；参考方向缺失时按北纬东经处理
  if ((find('GPSLatitudeRef') ?? 'N').toUpperCase().startsWith('S')) lat = -lat
  if ((find('GPSLongitudeRef') ?? 'E').toUpperCase().startsWith('W')) lng = -lng
  return { lat, lng }
}

/** 一个地图打开入口：名称 + URL。 */
export interface MapLink {
  name: string
  url: string
}

/** 由十进制经纬度生成各地图的打开链接。EXIF 坐标为 WGS-84：
 *  高德（默认 GCJ-02）用 coordinate=wgs84、百度（默认 BD-09）用 coord_type=wgs84
 *  声明坐标系，由地图侧转换；Google / Apple 直接用 WGS-84。 */
export function mapLinks(lat: number, lng: number): MapLink[] {
  const pos = `${lng},${lat}`
  return [
    { name: '高德地图', url: `https://uri.amap.com/marker?position=${pos}&coordinate=wgs84&name=图片位置` },
    { name: '百度地图', url: `https://api.map.baidu.com/marker?location=${lat},${lng}&coord_type=wgs84&content=图片位置&output=html&src=sviewer` },
    { name: 'Google 地图', url: `https://www.google.com/maps/search/?api=1&query=${lat},${lng}` },
    { name: 'Apple 地图', url: `https://maps.apple.com/?ll=${lat},${lng}&q=%E5%9B%BE%E7%89%87%E4%BD%8D%E7%BD%AE` },
  ]
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
