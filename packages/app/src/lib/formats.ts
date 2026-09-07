/**
 * 图片格式知识：扩展名能力清单、另存为过滤器、扩展名 → 目标格式映射。
 * 单一事实来源——前端各处需要判断「支持什么格式」都从这里取，
 * 不要再各自内联清单（Rust 侧由 scripts/gen-formats.cjs 从 formats.json 生成，见批次 3）。
 */
import type { SaveFormat } from './types'

/** WebView 可直接渲染的扩展名（小写，不含点）。jpe/jfif 是 JPEG 别名。 */
export const WEB_NATIVE = new Set([
  'jpg', 'jpeg', 'jpe', 'jfif', 'png', 'gif', 'webp', 'bmp', 'ico', 'svg', 'avif',
])

/** 需要 libheif（WASM）在前端解码的扩展名。Rust 的 image crate 不支持 HEIC。hif 是富士的 HEIF 容器。 */
export const HEIF_EXT = new Set(['heic', 'heif', 'hif'])

/** 可直接改写原图的扩展名（heic 无编码器、svg 矢量、gif 动图会丢帧）。jpe/jfif 按 JPEG 写回。 */
export const EDITABLE_EXT = new Set([
  'jpg', 'jpeg', 'jpe', 'jfif', 'png', 'webp', 'bmp', 'tiff', 'tif', 'avif', 'tga', 'qoi', 'exr',
])

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
