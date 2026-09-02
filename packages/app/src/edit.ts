import { createApp } from 'vue'
import EditWindow from './EditWindow.vue'
import { setupLogger } from './logger'
import './common.css'

setupLogger()

createApp(EditWindow).mount('#app')
