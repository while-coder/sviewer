import { createApp } from 'vue'
import App from './App.vue'
import { setupLogger } from './logger'
import './common.css'

setupLogger()

createApp(App).mount('#app')
