/**
 * 图片格式知识：全部派生自 formats.json（单一事实来源）。
 * 改扩展名清单 / 格式关联后必须运行 `pnpm gen:formats` 同步 Rust 侧与 tauri.conf.json。
 */
import raw from './formats.json'
import type { SaveFormat } from '../types'

/** 受支持的扩展名（小写，不含点）。与 Rust 侧 formats_gen::SUPPORTED_EXT 同源。 */
export const SUPPORTED_EXT = new Set(raw.supported)
/** WebView 可直接渲染的扩展名。jpe/jfif 是 JPEG 别名。 */
export const WEB_NATIVE = new Set(raw.webNative)
/** 需要 libheif（WASM）在前端解码的扩展名。Rust 的 image crate 不支持 HEIC。hif 是富士的 HEIF 容器。 */
export const HEIF_EXT = new Set(raw.heif)
/** 可直接改写原图的扩展名（heic 无编码器、svg 矢量、gif 动图会丢帧）。jpe/jfif 按 JPEG 写回。 */
export const EDITABLE_EXT = new Set(raw.editable)
/** 批量转换可作转换源的扩展名（svg 矢量不参与；仅列出常用，pbm/dds 等小众格式不进列表）。 */
export const CONVERTIBLE_EXT = new Set(raw.convertible)

/** 保存目标格式。original 为原样复制，其余由 Rust 重编码。 */
export type { SaveFormat }

/** 取扩展名（小写，不含点）。 */
export function extOf(path: string): string {
  const i = path.lastIndexOf('.')
  return i >= 0 ? path.slice(i + 1).toLowerCase() : ''
}

/** 该格式能否由 WebView 直接显示。 */
export function isWebNative(path: string): boolean {
  return WEB_NATIVE.has(extOf(path))
}

/** 该图片能否「保存到原图」（旋转/裁剪等编辑写回）。 */
export function extSupportsEdit(path: string): boolean {
  return EDITABLE_EXT.has(extOf(path))
}

/** 打开文件对话框的「图片」过滤器（全部受支持扩展名）。 */
export const OPEN_FILTERS = [{ name: '图片', extensions: [...SUPPORTED_EXT] }]

/** 另存为对话框的「保存类型」列表。 */
export const SAVE_FILTERS = [
  { name: 'JPEG 图片', extensions: ['jpg', 'jpeg'] },
  { name: 'PNG 图片', extensions: ['png'] },
  { name: 'WebP 图片', extensions: ['webp'] },
  { name: 'TIFF 图片', extensions: ['tiff', 'tif'] },
  { name: 'BMP 图片', extensions: ['bmp'] },
  { name: 'GIF 图片', extensions: ['gif'] },
  { name: 'ICO 图标（自动缩至 256）', extensions: ['ico'] },
  { name: 'TGA 图片', extensions: ['tga'] },
  { name: 'PPM 图片', extensions: ['ppm'] },
  { name: 'QOI 图片', extensions: ['qoi'] },
  { name: 'EXR 图片', extensions: ['exr'] },
  { name: 'AVIF 图片（较慢）', extensions: ['avif'] },
  { name: 'Farbfeld 图片', extensions: ['ff'] },
]

/** 保存扩展名 → 目标格式。 */
export const EXT_FORMAT: Record<string, SaveFormat> = {
  jpg: 'jpeg',
  jpeg: 'jpeg',
  png: 'png',
  webp: 'webp',
  bmp: 'bmp',
  tiff: 'tiff',
  tif: 'tiff',
  gif: 'gif',
  ico: 'ico',
  tga: 'tga',
  ppm: 'ppm',
  qoi: 'qoi',
  exr: 'exr',
  ff: 'ff',
  avif: 'avif',
}

/** 按保存路径的扩展名推断目标格式；与源同扩展名视为原样保留（避免无谓的有损重编码）。 */
export function inferFormat(dest: string, src: string): SaveFormat {
  const ext = dest.split('.').pop()?.toLowerCase() ?? ''
  if (ext === (src.split('.').pop()?.toLowerCase() ?? '')) return 'original'
  return EXT_FORMAT[ext] ?? 'original'
}

/** 目标格式选择列表（编辑窗口与批量转换共用；批量转换过滤掉 original 首项）。 */
export const SAVE_FORMAT_OPTIONS: { value: SaveFormat; label: string }[] = [
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
