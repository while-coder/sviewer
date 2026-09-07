/** 通用工具函数。 */
import { openUrl } from '@tauri-apps/plugin-opener'

/** 用系统默认浏览器打开外链。 */
export async function openExternal(url: string) {
  try {
    await openUrl(url)
  } catch (e) {
    console.error('打开链接失败', url, e)
  }
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
