<script setup lang="ts">
/**
 * 批量转换窗口的根组件（batch.ts 挂载）：
 * 应用主题，与主窗口经 localStorage / storage 事件同步设置。
 */
import { watchEffect } from 'vue'
import { resolvedTheme, watchExternalSettings } from './settings'
import BatchConvertDialog from './BatchConvertDialog.vue'

watchEffect(() => {
  document.documentElement.dataset.theme = resolvedTheme.value
})
watchExternalSettings()
</script>

<template>
  <BatchConvertDialog />
</template>
