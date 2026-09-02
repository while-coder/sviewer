/**
 * 应用设置：localStorage 持久化的轻量偏好。
 * 设置弹窗（App.vue）直接改这里的 reactive 字段，watch 自动落盘；
 * 主题等派生值也在这里统一计算，App.vue 只管消费。
 */
import { computed, reactive, ref, watch } from 'vue'
import { invoke } from '@tauri-apps/api/core'

/** 界面主题：深色 / 浅色 / 跟随系统。 */
export type Theme = 'dark' | 'light' | 'system'
/** 打开图片后的默认视图。 */
export type DefaultView = 'fit' | 'actual'

export interface AppSettings {
  theme: Theme
  defaultView: DefaultView
  /** 画布背景棋盘格（透明区域指示），关闭则显示纯色 */
  checkerboard: boolean
  /** 显示图片边缘轮廓线（透明 PNG 等与背景融为一体时用来定位边界） */
  outline: boolean
  /** 是否显示右侧图片信息面板 */
  showInfo: boolean
  /** 无浮层时按 Esc：退出程序（true）还是最小化窗口（默认） */
  escClose: boolean
  /** 允许多开：改完后写入标记文件，下次启动生效（Rust 启动时读不到 localStorage） */
  allowMulti: boolean
}

const KEY = 'sviewer:settings'

const DEFAULTS: AppSettings = {
  theme: 'dark',
  defaultView: 'fit',
  checkerboard: true,
  outline: false,
  showInfo: false,
  escClose: false,
  allowMulti: false,
}

function load(): AppSettings {
  try {
    return { ...DEFAULTS, ...JSON.parse(localStorage.getItem(KEY) ?? '{}') }
  } catch {
    return { ...DEFAULTS }
  }
}

/** 全局设置单例：直接改字段即可生效并持久化。 */
export const settings = reactive<AppSettings>(load())

/**
 * 监听其他窗口（主窗口）改设置：storage 事件只在别的窗口触发，收到后重读。
 * 批量转换窗口等副窗口调用，主窗口不需要。
 */
export function watchExternalSettings() {
  window.addEventListener('storage', (e) => {
    if (e.key !== KEY || !e.newValue) return
    try {
      Object.assign(settings, { ...DEFAULTS, ...JSON.parse(e.newValue) })
    } catch {
      /* 新值非法则忽略，保持现状 */
    }
  })
}

watch(
  settings,
  (v) => {
    try {
      localStorage.setItem(KEY, JSON.stringify(v))
    } catch {
      /* 存储写入失败不影响使用 */
    }
  },
  { deep: true },
)

// 「允许多开」镜像到 Rust 侧标记文件（%APPDATA%/com.while.sviewer/allow-multi-instance），
// 启动时据此决定是否注册 single-instance 插件；变化时写入，启动时也同步一次（重装后恢复）。
watch(
  () => settings.allowMulti,
  (v) => {
    void invoke('set_multi_instance', { enabled: v }).catch((e) =>
      console.warn('同步多开设置失败', e),
    )
  },
  { immediate: true },
)

/** 系统当前是否深色（用于 theme = 'system'）。 */
const systemDark = ref(window.matchMedia('(prefers-color-scheme: dark)').matches)
window.matchMedia('(prefers-color-scheme: dark)').addEventListener('change', (e) => {
  systemDark.value = e.matches
})

/** 实际生效的主题。 */
export const resolvedTheme = computed<'dark' | 'light'>(() =>
  settings.theme === 'system' ? (systemDark.value ? 'dark' : 'light') : settings.theme,
)
