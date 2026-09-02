import { createApp } from 'vue'
import BatchWindow from './BatchWindow.vue'
import { setupLogger } from './logger'
import './common.css'

setupLogger()

createApp(BatchWindow).mount('#app')
