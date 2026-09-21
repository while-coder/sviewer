/**
 * 错误弹窗（window.alert 的替代品）：
 * WebView2（wry）关闭了默认脚本对话框，window.alert / confirm 是静默 no-op，
 * 所有「保存失败」之类的错误提示必须走 dialog 插件的原生 message 对话框，
 * 否则失败对用户不可见，表现为「点了没反应」。
 */
import { message } from '@tauri-apps/plugin-dialog'

/** 弹原生错误对话框；对话框本身失败时兜底 console（不抛出，避免打断调用方）。 */
export async function alertError(headline: string, detail?: unknown): Promise<void> {
  const text = detail === undefined ? headline : `${headline}：${detail}`
  console.error(text)
  try {
    await message(text, { kind: 'error', title: '速阅' })
  } catch (e) {
    console.error('错误弹窗显示失败', e, text)
  }
}
