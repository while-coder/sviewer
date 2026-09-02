/**
 * 主窗口与编辑窗口共用的保存/格式逻辑：
 * 另存为对话框的文件类型、扩展名 → 目标格式映射、按扩展名推断格式。
 */
import type { SaveFormat } from './viewer'

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
