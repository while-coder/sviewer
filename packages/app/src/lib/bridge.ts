/**
 * Tauri command 调用的唯一出口：所有 invoke 都收在这里，
 * 组件层不直接接触 @tauri-apps/api/core。
 */
import { invoke } from '@tauri-apps/api/core'
import type { ImageInfo, ImageEdits, SaveFormat, SaveOutcome } from './types'

/** 启动时（双击文件 / 命令行）传入的待打开文件，取一次后清空；无则返回 null。 */
export function getLaunchFile(): Promise<string | null> {
  return invoke<string | null>('get_launch_file')
}

/** 同目录下所有受支持的图片（已排序），用于左右切换。 */
export function listSiblings(path: string): Promise<string[]> {
  return invoke<string[]>('list_dir_images', { path })
}

/** 把图片另存到目标路径（original 为原样复制，其余格式由 Rust 重编码）。 */
export function saveImageAs(src: string, dest: string, format: SaveFormat): Promise<void> {
  return invoke('save_image_as', { src, dest, format })
}

/** 把编辑（旋转/镜像/裁剪/改尺寸/标记）烘焙后写回原图。 */
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

/** 读取图片元信息（尺寸 / 格式 / EXIF）。 */
export function readImageInfo(path: string): Promise<ImageInfo> {
  return invoke<ImageInfo>('read_image_info', { path })
}

/** 任意格式解码为 PNG，返回 data URL（<img> 场景对非原生格式的兜底）。 */
export function decodeToPng(path: string): Promise<string> {
  return invoke<string>('decode_to_png', { path })
}
