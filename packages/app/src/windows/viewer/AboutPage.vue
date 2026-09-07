<script setup lang="ts">
/**
 * 「关于」页（设置弹窗第 4 页）：产品信息、相关链接、版本更新。
 *
 * updater 实例由 ViewerWindow 创建（保证应用内更新只自动检查一次），
 * 经 props 下传展示状态与手动检查入口。
 */
import { computed } from 'vue'
import { useTauriUpdater } from '@while-coder/tauri-updater-vue'
import { openExternal } from '../../lib/util'

const props = defineProps<{
  /** 当前应用版本（getVersion 取得，空串表示未取到） */
  appVersion: string
  /** ViewerWindow 单例创建的 updater */
  updater: ReturnType<typeof useTauriUpdater>
}>()

/** 相关链接（GitHub 仓库 / 发布页 / Issues）。 */
const GITHUB_URL = 'https://github.com/while-coder/sviewer'
const RELEASES_URL = `${GITHUB_URL}/releases/latest`
const ISSUES_URL = `${GITHUB_URL}/issues`

const updateBusy = computed(() => {
  const s = props.updater.updateStatus.value
  return s === 'checking' || s === 'downloading'
})
// 版本徽章文案与配色，随检查状态切换
const versionTagType = computed(() => {
  switch (props.updater.updateStatus.value) {
    case 'latest': case 'installed': return 'ok'
    case 'available': return 'warn'
    case 'error': return 'err'
    default: return ''
  }
})
const versionTagText = computed(() => {
  switch (props.updater.updateStatus.value) {
    case 'latest': return '已是最新'
    case 'installed': return '已就绪'
    case 'available': return props.updater.updateVersion.value ? `新版本 v${props.updater.updateVersion.value}` : '发现新版本'
    case 'checking': return '检查中'
    case 'error': return '检查失败'
    default: return props.appVersion ? `v${props.appVersion}` : '未检测'
  }
})
function checkUpdate() {
  void props.updater.checkForUpdate()
}
</script>

<template>
  <div class="about-page">
    <section class="about-hero">
      <img class="about-logo" src="/sviewer-icon.png" alt="素阅" />
      <div class="about-product">
        <div class="about-title-row">
          <h2>素阅</h2>
          <span v-if="appVersion" class="vtag">v{{ appVersion }}</span>
        </div>
        <p>轻量级本地图片查看器</p>
        <span>支持 JPG / PNG / GIF / WebP / AVIF / TIFF / HEIC 等</span>
      </div>
    </section>

    <div class="about-links">
      <button class="about-link-card" type="button" @click="openExternal(GITHUB_URL)">
        <span class="about-link-icon" aria-hidden="true">
          <svg viewBox="0 0 24 24"><path d="M12 2a10 10 0 0 0-3.16 19.49c.5.09.68-.22.68-.48v-1.87c-2.78.6-3.37-1.18-3.37-1.18-.45-1.16-1.11-1.47-1.11-1.47-.91-.62.07-.61.07-.61 1 .07 1.53 1.03 1.53 1.03.9 1.53 2.35 1.09 2.92.83.09-.65.35-1.09.64-1.34-2.22-.25-4.55-1.11-4.55-4.94 0-1.09.39-1.98 1.03-2.68-.1-.25-.45-1.27.1-2.64 0 0 .84-.27 2.75 1.02A9.6 9.6 0 0 1 12 6.82a9.6 9.6 0 0 1 2.5.34c1.91-1.29 2.75-1.02 2.75-1.02.55 1.37.2 2.39.1 2.64.64.7 1.03 1.59 1.03 2.68 0 3.84-2.34 4.68-4.56 4.93.36.31.68.92.68 1.86v2.76c0 .27.18.58.69.48A10 10 0 0 0 12 2Z" /></svg>
        </span>
        <span class="about-link-copy">
          <strong>GitHub</strong>
          <small>github.com/while-coder/sviewer</small>
        </span>
        <span class="about-link-arrow" aria-hidden="true"><svg viewBox="0 0 24 24"><path d="M7 17 17 7M7 7h10v10" /></svg></span>
      </button>

      <button class="about-link-card" type="button" @click="openExternal(ISSUES_URL)">
        <span class="about-link-icon" aria-hidden="true">
          <svg viewBox="0 0 24 24"><circle cx="12" cy="12" r="10" /><path d="M12 8v4M12 16h.01" /></svg>
        </span>
        <span class="about-link-copy">
          <strong>问题反馈</strong>
          <small>提交 Bug 或功能建议</small>
        </span>
        <span class="about-link-arrow" aria-hidden="true"><svg viewBox="0 0 24 24"><path d="M7 17 17 7M7 7h10v10" /></svg></span>
      </button>

      <button class="about-link-card" type="button" @click="openExternal(RELEASES_URL)">
        <span class="about-link-icon" aria-hidden="true">
          <svg viewBox="0 0 24 24"><path d="M21 15v4a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2v-4" /><path d="m7 10 5 5 5-5" /><path d="M12 15V3" /></svg>
        </span>
        <span class="about-link-copy">
          <strong>版本发布</strong>
          <small>查看各平台安装包与更新日志</small>
        </span>
        <span class="about-link-arrow" aria-hidden="true"><svg viewBox="0 0 24 24"><path d="M7 17 17 7M7 7h10v10" /></svg></span>
      </button>
    </div>

    <div class="about-card">
      <h4>版本更新</h4>
      <div class="settings-row">
        <span class="vtag" :class="versionTagType">{{ versionTagText }}</span>
        <span v-if="appVersion" class="hint">当前版本 v{{ appVersion }}</span>
      </div>
      <p class="hint">{{ updater.updateStatusText.value }}</p>
      <div class="card-actions">
        <button class="ep-btn" :disabled="!updater.updaterSupported || updateBusy" @click="checkUpdate">检查更新</button>
      </div>
    </div>
  </div>
</template>

<style>
/* 「关于」页：hero 区 + 相关链接卡片 + 版本更新 */
.about-page { display: flex; flex-direction: column; gap: 12px; }
.about-hero {
  display: flex; align-items: center; gap: 16px;
  padding: 16px 18px;
  border: 1px solid var(--border); border-radius: 12px;
  background: linear-gradient(135deg, var(--hover), transparent);
}
.about-logo {
  width: 56px; height: 56px; flex: 0 0 56px;
  border: 1px solid var(--border); border-radius: 12px;
  background: var(--bar); padding: 6px;
}
.about-product { min-width: 0; }
.about-title-row { display: flex; align-items: center; gap: 10px; }
.about-title-row h2 { margin: 0; font-size: 20px; }
.about-product p { margin: 6px 0 4px; color: var(--fg-muted); }
.about-product > span { color: var(--fg-muted); font-size: 11px; opacity: 0.85; }
/* 版本徽章：默认灰，按检查状态着色 */
.vtag {
  display: inline-block; padding: 1px 9px; border-radius: 999px;
  background: var(--hover); border: 1px solid var(--border);
  color: var(--fg-muted); font-size: 11px; line-height: 1.7;
}
.vtag.ok { color: #34d399; border-color: rgba(52, 211, 153, 0.4); }
.vtag.warn { color: #f59e0b; border-color: rgba(245, 158, 11, 0.4); }
.vtag.err { color: #ef4444; border-color: rgba(239, 68, 68, 0.4); }
/* 相关链接：两列卡片，悬停描边高亮 */
.about-links { display: grid; grid-template-columns: repeat(2, minmax(0, 1fr)); gap: 8px; }
.about-link-card {
  display: grid; grid-template-columns: 34px minmax(0, 1fr) auto;
  align-items: center; gap: 10px; min-width: 0; min-height: 60px;
  padding: 10px;
  background: none; border: 1px solid var(--border); border-radius: 10px;
  color: var(--fg); cursor: pointer; text-align: left;
}
.about-link-card:hover { border-color: var(--primary); background: var(--hover); }
.about-link-icon {
  display: grid; place-items: center; width: 34px; height: 34px;
  border-radius: 9px; background: var(--hover); color: var(--primary);
}
.about-link-icon svg {
  width: 18px; height: 18px;
  fill: none; stroke: currentColor; stroke-width: 1.8;
  stroke-linecap: round; stroke-linejoin: round;
}
/* GitHub 卡片排第一：图标实心填充 */
.about-link-card:first-child .about-link-icon svg { fill: currentColor; stroke: none; }
.about-link-copy { display: grid; gap: 2px; min-width: 0; }
.about-link-copy strong { font-size: 13px; font-weight: 600; }
.about-link-copy small {
  overflow: hidden; color: var(--fg-muted); font-size: 11px;
  text-overflow: ellipsis; white-space: nowrap;
}
.about-link-arrow { display: grid; place-items: center; color: var(--fg-muted); }
.about-link-arrow svg {
  width: 14px; height: 14px;
  fill: none; stroke: currentColor; stroke-width: 1.8;
  stroke-linecap: round; stroke-linejoin: round;
}
/* 版本更新卡片：徽章 + 状态说明 + 手动检查 */
.about-card { border: 1px solid var(--border); border-radius: 10px; padding: 12px 14px; }
.about-card h4 { margin: 0 0 8px; font-size: 13px; }
.about-card .settings-row { display: flex; align-items: center; gap: 10px; }
.about-card .hint { margin: 6px 0 0; color: var(--fg-muted); font-size: 12px; }
.about-card .card-actions { display: flex; justify-content: flex-end; margin-top: 10px; }
</style>
