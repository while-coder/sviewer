/** 主题同步：实际主题应用到 <html data-theme>，并监听其他窗口的设置变更。 */
import { watchEffect } from 'vue'
import { resolvedTheme, watchExternalSettings } from '../lib/settings'

export function useThemeSync() {
  watchEffect(() => {
    document.documentElement.dataset.theme = resolvedTheme.value
  })
  watchExternalSettings()
}
