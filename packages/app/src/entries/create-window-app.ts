/**
 * 窗口入口工厂：三个 MPA 窗口（main / edit / batch）的挂载流程完全一致，
 * 收敛到这里，各入口文件只声明自己的根组件。
 */
import { createApp, type Component } from 'vue'
import { setupLogger } from '../lib/logger'
import '../common.css'

export function createWindowApp(root: Component) {
  setupLogger()
  createApp(root).mount('#app')
}
