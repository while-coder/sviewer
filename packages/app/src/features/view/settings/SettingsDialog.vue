<script setup lang="ts">
/**
 * 设置弹窗：左侧分类菜单 + 右侧内容页（常规 / 查看 / 格式关联 / 关于）。
 *
 * 由 ViewerWindow 持有 open 状态；打开时按 initialTab 定位分类页
 * （菜单「设置」进常规，右键/菜单「关于」进关于页），页内切换经 emit 同步回去。
 */
import { computed, ref, watch } from 'vue'
import { invoke } from '@tauri-apps/api/core'
import { useTauriUpdater } from '@while-coder/tauri-updater-vue'
import { settings } from '../../../lib/settings'
import AboutPage from './AboutPage.vue'

export type SettingsTab = 'general' | 'view' | 'assoc' | 'about'

const props = defineProps<{
  /** 弹窗开关（ViewerWindow 的 modal === 'settings'） */
  open: boolean
  /** 本次打开时的初始分类页 */
  initialTab: SettingsTab
  /** 透传给「关于」页 */
  appVersion: string
  /** 透传给「关于」页的 updater 单例 */
  updater: ReturnType<typeof useTauriUpdater>
}>()
const emit = defineEmits<{ close: []; tab: [SettingsTab] }>()

const tab = ref<SettingsTab>(props.initialTab)
// 每次打开都按 initialTab 重新定位（同一实例反复开关）
watch(
  () => props.open,
  (v) => {
    if (v) tab.value = props.initialTab
  },
)
function setTab(t: SettingsTab) {
  tab.value = t
  emit('tab', t)
}

// ── 格式关联（Windows，只写 HKCU 免管理员）──────────────
// 应用内一键关联仅 Windows 有实现（macOS 靠 Info.plist 声明、Linux 靠 .desktop），
// tab 只在 Windows 显示；assoc_status 返回空列表（读取失败）时区块内容也不显示。
const showAssocTab = /Win/i.test(navigator.platform)
interface AssocStatus { ext: string; app: string; isSviewer: boolean }
const assocList = ref<AssocStatus[]>([])
const assocSelected = ref<string[]>([])
const assocBusy = ref(false)
const allAssocChecked = computed(
  () => assocList.value.length > 0 && assocSelected.value.length === assocList.value.length,
)
function toggleAllAssoc(e: Event) {
  assocSelected.value = (e.target as HTMLInputElement).checked
    ? assocList.value.map((a) => a.ext)
    : []
}
async function loadAssoc() {
  try {
    assocList.value = await invoke<AssocStatus[]>('assoc_status')
  } catch (e) {
    console.warn('读取格式关联失败', e)
  }
}
async function applyAssoc(exts: string[]) {
  if (!exts.length || assocBusy.value) return
  assocBusy.value = true
  try {
    await invoke('assoc_set', { exts })
    await loadAssoc()
  } catch (e) {
    window.alert(`关联失败：${e}`)
  } finally {
    assocBusy.value = false
  }
}
// 打开设置弹窗时才拉取关联状态
watch(
  () => props.open,
  (v) => {
    if (v && showAssocTab) loadAssoc()
  },
)
</script>

<template>
  <transition name="fade">
    <div v-if="open" class="modal-backdrop" @mousedown.self="emit('close')">
      <div class="modal settings">
        <header>
          <span>设置</span>
          <button class="close" title="关闭" @click="emit('close')">×</button>
        </header>

        <div class="settings-body">
          <nav class="settings-nav">
            <button :class="{ on: tab === 'general' }" @click="setTab('general')">常规</button>
            <button :class="{ on: tab === 'view' }" @click="setTab('view')">查看</button>
            <button v-if="showAssocTab" :class="{ on: tab === 'assoc' }" @click="setTab('assoc')">格式关联</button>
            <button :class="{ on: tab === 'about' }" @click="setTab('about')">关于</button>
          </nav>

          <div class="settings-page">
            <template v-if="tab === 'general'">
              <h3>常规</h3>
              <div class="row">
                <span class="label">主题</span>
                <div class="seg">
                  <button :class="{ on: settings.theme === 'dark' }" @click="settings.theme = 'dark'">深色</button>
                  <button :class="{ on: settings.theme === 'light' }" @click="settings.theme = 'light'">浅色</button>
                  <button :class="{ on: settings.theme === 'system' }" @click="settings.theme = 'system'">跟随系统</button>
                </div>
              </div>

              <div class="row">
                <span class="label">按 Esc 关闭程序<small>无浮层时按 Esc 退出程序，而不是最小化窗口</small></span>
                <div class="seg">
                  <button :class="{ on: settings.escClose }" @click="settings.escClose = true">开</button>
                  <button :class="{ on: !settings.escClose }" @click="settings.escClose = false">关</button>
                </div>
              </div>

              <div class="row">
                <span class="label">允许多开<small>可同时打开多个素阅窗口，重启后生效</small></span>
                <div class="seg">
                  <button :class="{ on: settings.allowMulti }" @click="settings.allowMulti = true">开</button>
                  <button :class="{ on: !settings.allowMulti }" @click="settings.allowMulti = false">关</button>
                </div>
              </div>
            </template>

            <template v-else-if="tab === 'view'">
              <h3>查看</h3>
              <div class="row">
                <span class="label">打开图片时</span>
                <div class="seg">
                  <button :class="{ on: settings.defaultView === 'fit' }" @click="settings.defaultView = 'fit'">适应窗口</button>
                  <button :class="{ on: settings.defaultView === 'actual' }" @click="settings.defaultView = 'actual'">原始大小</button>
                </div>
              </div>

              <div class="row">
                <span class="label">背景棋盘格</span>
                <div class="seg">
                  <button :class="{ on: settings.checkerboard }" @click="settings.checkerboard = true">开</button>
                  <button :class="{ on: !settings.checkerboard }" @click="settings.checkerboard = false">关</button>
                </div>
              </div>

              <div class="row">
                <span class="label">显示图片边缘</span>
                <div class="seg">
                  <button :class="{ on: settings.outline }" @click="settings.outline = true">开</button>
                  <button :class="{ on: !settings.outline }" @click="settings.outline = false">关</button>
                </div>
              </div>

              <div class="row">
                <span class="label">图片信息面板</span>
                <div class="seg">
                  <button :class="{ on: settings.showInfo }" @click="settings.showInfo = true">开</button>
                  <button :class="{ on: !settings.showInfo }" @click="settings.showInfo = false">关</button>
                </div>
              </div>

              <div class="row">
                <span class="label">位置地名解析<small>详情抽屉打开时把 GPS 坐标解析成地名（需联网；结果自动缓存，请求限速约 1 条/秒）</small></span>
                <div class="seg">
                  <button :class="{ on: settings.geoProvider === 'osm' }" @click="settings.geoProvider = 'osm'">OSM</button>
                  <button :class="{ on: settings.geoProvider === 'amap' }" @click="settings.geoProvider = 'amap'">高德</button>
                  <button :class="{ on: settings.geoProvider === 'baidu' }" @click="settings.geoProvider = 'baidu'">百度</button>
                  <button :class="{ on: settings.geoProvider === 'off' }" @click="settings.geoProvider = 'off'">关闭</button>
                </div>
              </div>

              <div v-if="settings.geoProvider === 'amap'" class="row">
                <span class="label">高德 Key<small>lbs.amap.com 申请「Web 服务」类型 Key（个人实名免费）</small></span>
                <input v-model="settings.amapKey" class="text" type="password" placeholder="粘贴高德 Key" spellcheck="false" />
              </div>

              <div v-if="settings.geoProvider === 'baidu'" class="row">
                <span class="label">百度 AK<small>lbsyun.baidu.com 创建「服务端」类型应用（个人实名免费）</small></span>
                <input v-model="settings.baiduKey" class="text" type="password" placeholder="粘贴百度 AK" spellcheck="false" />
              </div>
            </template>

            <!-- 格式关联（仅 Windows）：勾选格式后一键设为默认打开方式 -->
            <template v-else-if="tab === 'assoc'">
              <div class="page-head">
                <h3>格式关联</h3>
                <span v-if="assocList.length" class="assoc-actions">
                  <label class="assoc-all"><input type="checkbox" :checked="allAssocChecked" @change="toggleAllAssoc" />全选</label>
                  <button class="mini" :disabled="!assocSelected.length || assocBusy" @click="applyAssoc(assocSelected)">关联所选</button>
                  <button class="mini" :disabled="assocBusy" @click="applyAssoc(assocList.map((a) => a.ext))">关联全部</button>
                </span>
              </div>
              <template v-if="assocList.length">
                <div class="assoc-list">
                  <label v-for="a in assocList" :key="a.ext" class="assoc-item">
                    <input v-model="assocSelected" type="checkbox" :value="a.ext" />
                    <span class="ext">.{{ a.ext }}</span>
                    <span class="app" :class="{ ours: a.isSviewer }">{{ a.app }}</span>
                  </label>
                </div>
                <p class="assoc-tip">部分格式可能被系统「默认应用」锁定，关联后仍打开异常时请在 Windows 设置 → 默认应用中确认。</p>
              </template>
              <p v-else class="assoc-tip">当前平台不支持格式关联。</p>
            </template>

            <template v-else>
              <AboutPage :app-version="appVersion" :updater="updater" />
            </template>
          </div>
        </div>

      </div>
    </div>
  </transition>
</template>

<style>
/* 关于 / 设置 弹窗 */
.fade-enter-active, .fade-leave-active { transition: opacity 0.15s ease; }
.fade-enter-from, .fade-leave-to { opacity: 0; }
.modal-backdrop {
  position: fixed; inset: 0; z-index: 30;
  display: flex; align-items: center; justify-content: center;
  background: rgba(0, 0, 0, 0.45);
}
.modal {
  width: 400px; max-width: 92vw;
  background: var(--panel);
  border: 1px solid var(--border);
  border-radius: 12px;
  padding: 18px 20px;
  box-shadow: 0 12px 40px rgba(0, 0, 0, 0.4);
}
.modal.settings {
  width: 620px; max-width: 94vw;
  height: 480px; max-height: 86vh; /* 高度固定，切换分类页时窗口不跳动 */
  padding: 0; overflow: hidden;
  display: flex; flex-direction: column;
}
.modal.settings header {
  display: flex; align-items: center; justify-content: space-between;
  padding: 12px 20px;
  border-bottom: 1px solid var(--border);
  font-weight: 600;
}
.modal.settings .close {
  background: none; border: none; color: var(--fg-muted); cursor: pointer;
  font-size: 16px; line-height: 1; padding: 2px 6px; border-radius: 6px;
}
.modal.settings .close:hover { color: var(--fg); background: var(--hover); }

/* 设置弹窗主体：左分类菜单 + 右内容页 */
.settings-body { display: flex; flex: 1; min-height: 0; }
.settings-nav {
  width: 128px; flex-shrink: 0;
  display: flex; flex-direction: column; gap: 2px;
  padding: 10px 8px;
  border-right: 1px solid var(--border);
}
.settings-nav button {
  background: none; border: none; cursor: pointer;
  color: var(--fg-muted); font-size: 13px; text-align: left;
  padding: 7px 12px; border-radius: 8px;
}
.settings-nav button:hover { background: var(--hover); color: var(--fg); }
.settings-nav button.on { background: var(--hover); color: var(--primary); font-weight: 600; }
.settings-page { flex: 1; min-width: 0; min-height: 0; padding: 14px 20px; overflow-y: auto; display: flex; flex-direction: column; }
.settings-page h3 { margin: 0 0 4px; font-size: 14px; }
.page-head { display: flex; align-items: center; justify-content: space-between; gap: 12px; margin-bottom: 8px; }
.page-head h3 { margin: 0; }

/* 设置行：左标签右分段选择器；放不下时整组换行（seg 靠右），绝不挤压按钮 */
.row {
  display: flex; align-items: center; justify-content: space-between; gap: 6px 12px;
  flex-wrap: wrap;
  padding: 9px 0;
}
.row .label { flex-shrink: 0; }
.row .label small { display: block; color: var(--fg-muted); font-size: 11px; margin-top: 2px; }
.seg { display: flex; border: 1px solid var(--border); border-radius: 8px; overflow: hidden; flex-shrink: 0; margin-left: auto; }
.seg button {
  background: none; border: none; color: var(--fg-muted); cursor: pointer;
  font-size: 12px; padding: 4px 10px; white-space: nowrap; flex-shrink: 0;
}
.seg button + button { border-left: 1px solid var(--border); }
.seg button:hover { background: var(--hover); }
.seg button.on { background: var(--primary); color: #fff; }
/* 设置行里的文本输入（如逆地理 Key）：与 seg 同框风格 */
.row input.text {
  width: 240px;
  padding: 5px 9px;
  font: inherit;
  color: var(--fg);
  background: var(--card);
  border: 1px solid var(--border);
  border-radius: 8px;
  outline: none;
}
.row input.text:focus { border-color: var(--primary); }
.row input.text::placeholder { color: var(--fg-muted); }

/* 格式关联：工具行 + 格式列表（自动撑满弹窗剩余高度） */
.assoc-actions { display: flex; align-items: center; gap: 8px; }
.assoc-all { display: flex; align-items: center; gap: 4px; font-size: 12px; color: var(--fg-muted); cursor: pointer; }
.assoc-actions .mini {
  background: none; border: 1px solid var(--border); color: var(--fg); cursor: pointer;
  font-size: 12px; padding: 3px 10px; border-radius: 6px;
}
.assoc-actions .mini:hover:not(:disabled) { background: var(--hover); }
.assoc-actions .mini:disabled { opacity: 0.4; cursor: default; }
.assoc-list {
  flex: 1 1 auto; min-height: 120px; overflow-y: auto;
  border: 1px solid var(--border); border-radius: 8px;
}
.assoc-item {
  display: flex; align-items: center; gap: 8px;
  padding: 4px 10px; font-size: 12px; cursor: pointer;
}
.assoc-item:hover { background: var(--hover); }
.assoc-item .ext { width: 52px; }
.assoc-item .app { margin-left: auto; color: var(--fg-muted); }
.assoc-item .app.ours { color: var(--primary); }
.assoc-tip { margin: 6px 0 0; font-size: 11px; line-height: 1.6; color: var(--fg-muted); }
</style>
