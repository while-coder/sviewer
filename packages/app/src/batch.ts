import { createApp } from 'vue'
import BatchWindow from './BatchWindow.vue'
import { setupLogger } from './lib/logger'
import './common.css'

setupLogger()

createApp(BatchWindow).mount('#app')
