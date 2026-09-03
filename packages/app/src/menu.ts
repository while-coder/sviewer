/**
 * 原生系统菜单（窗口菜单栏）。
 *
 * 参考 sterm 的 useNativeAppMenu：用 @tauri-apps/api/menu 在前端构建原生菜单，
 * 菜单项 action 直接回调前端处理函数，不需要经过 Rust。移动端跳过。
 */
import { onBeforeUnmount, onMounted } from 'vue'
import { isTauri } from '@tauri-apps/api/core'
import { Menu, MenuItem, Submenu, type PredefinedMenuItemOptions } from '@tauri-apps/api/menu'

/** 菜单能触发的动作，由 App.vue 映射到具体函数。 */
export type AppMenuAction =
  | 'open-file'
  | 'save-as'
  | 'batch-convert'
  | 'fit'
  | 'actual-size'
  | 'toggle-info'
  | 'settings'
  | 'about'

const sep = (): PredefinedMenuItemOptions => ({ item: 'Separator' })

const isMacOs = /Mac/i.test(navigator.platform)

export function useAppMenu(onAction: (a: AppMenuAction) => void) {
  const supported = isTauri() && !/Android|iPhone|iPad|iPod/i.test(navigator.userAgent)
  let disposed = false

  const item = (id: AppMenuAction, text: string, accelerator?: string) =>
    MenuItem.new({ id: `menu:${id}`, text, accelerator: accelerator ?? null, action: () => handle(id) })

  async function initialize() {
    if (!supported) return
    try {
      // 加速键与 webview 快捷键语义对齐（0 适应窗口 / 1 原始大小 / i 信息面板）。
      // save-as 的 Cmd/Ctrl+S webview 侧也在监听，App.vue 里有重入锁防双触发。
      const [openFile, saveAs, batchConvert, fit, actualSize, toggleInfo, settings, about] = await Promise.all([
        item('open-file', '打开…', 'CmdOrCtrl+O'),
        item('save-as', '另存为…', 'CmdOrCtrl+S'),
        item('batch-convert', '批量转换…'),
        item('fit', '适应窗口', 'CmdOrCtrl+0'),
        item('actual-size', '原始大小', 'CmdOrCtrl+1'),
        item('toggle-info', '信息面板', 'CmdOrCtrl+I'),
        item('settings', '设置…', 'CmdOrCtrl+,'),
        item('about', '关于素阅'),
      ])

      const file = await Submenu.new({
        id: 'menu:file',
        text: '文件',
        items: [openFile, saveAs, batchConvert],
      })
      const view = await Submenu.new({
        id: 'menu:view',
        text: '视图',
        items: [fit, actualSize, sep(), toggleInfo, sep(), settings],
      })

      const submenus = [file, view]
      if (isMacOs) {
        // macOS：第一个子菜单是应用菜单（Hide/Quit 等系统预定义项）；
        // 「关于」按平台惯例放应用菜单，打开后跳到设置弹窗的「关于」页
        const app = await Submenu.new({
          id: 'menu:app',
          text: '素阅',
          items: [about, sep(), { item: 'Hide' }, { item: 'HideOthers' }, { item: 'ShowAll' }, sep(), { item: 'Quit' }],
        })
        submenus.unshift(app)
      }

      if (disposed) return
      const menu = await Menu.new({ id: 'app-menu', items: submenus })
      if (isMacOs) await menu.setAsAppMenu()
      else await menu.setAsWindowMenu()
    } catch (e) {
      console.warn('初始化系统菜单失败', e)
    }
  }

  function handle(a: AppMenuAction) {
    onAction(a)
  }

  onMounted(() => void initialize())
  onBeforeUnmount(() => {
    disposed = true
  })

  return { supported }
}
