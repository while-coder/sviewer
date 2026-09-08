/**
 * 「另存为」共享流程（主窗口与编辑窗口共用）：
 * 对话框 → 扩展名推断目标格式 → original 快路径 / Rust 重编码。
 * 重入锁由调用方持有（主窗口有菜单加速键 + webview 快捷键双触发问题）。
 */
import { save as saveDialog } from '@tauri-apps/plugin-dialog'
import { saveImageAs, encodeTo } from '../bridge/bridge'
import { SAVE_FILTERS, EXT_FORMAT, inferFormat, extOf } from '../formats/formats'
import type { ImageEdits, SaveFormat } from '../types'

export async function saveAsViaDialog(args: {
  /** 源图片绝对路径 */
  path: string
  /** 展示用文件名（取不到时用 path 尾段） */
  fileName: string
  /** 输出偏好：'original' 表示跟随源格式 */
  outputFormat: SaveFormat
  /** JPEG 输出质量 1~100 */
  quality: number
  /** 当前是否有编辑（有编辑时同扩展名也必须重编码，否则 edits 被 original 快路径丢掉） */
  modified: boolean
  /** 当前编辑参数（旋转/镜像/裁剪/标记） */
  edits: ImageEdits
}): Promise<void> {
  const { path, outputFormat, quality, modified, edits } = args
  const stem = (args.fileName || path.split(/[\\/]/).pop() || 'image').replace(/\.[^.]+$/, '')
  // 输出偏好选了目标格式时预填对应扩展名
  const defExt =
    outputFormat === 'original' ? (extOf(path) || 'jpg') : outputFormat === 'jpeg' ? 'jpg' : outputFormat
  const dest = await saveDialog({
    defaultPath: `${stem}.${defExt}`,
    filters: SAVE_FILTERS,
  })
  if (!dest) return
  try {
    // 带编辑时「与源同扩展名」也必须重编码（否则 edits 会被 original 快路径丢掉）
    let fmt = inferFormat(dest, path)
    if (modified && fmt === 'original') {
      fmt = EXT_FORMAT[dest.split('.').pop()?.toLowerCase() ?? ''] ?? 'original'
    }
    if (fmt === 'original') {
      // 无编辑且同格式：原样复制，不重编码
      await saveImageAs(path, dest, 'original')
    } else {
      await encodeTo(path, dest, fmt, outputFormat === 'jpeg' ? quality : null, edits)
    }
  } catch (e) {
    console.error('另存为失败', e)
    window.alert(`另存为失败：${e}`)
  }
}
