#!/usr/bin/env node

import { execFileSync } from 'node:child_process'
import fs from 'node:fs'
import path from 'node:path'
import { createRequire } from 'node:module'
import { fileURLToPath } from 'node:url'

const scriptDirectory = path.dirname(fileURLToPath(import.meta.url))
const rootDirectory = path.resolve(scriptDirectory, '..')
const appDirectory = path.join(rootDirectory, 'packages', 'app')
const sourceIcon = path.join(rootDirectory, 'packages', 'icon', 'icon.png')
const tauriIconsDirectory = path.join(appDirectory, 'src-tauri', 'icons')
const publicDirectory = path.join(appDirectory, 'public')
const favicon = path.join(publicDirectory, 'sviewer-icon.png')

if (!fs.existsSync(sourceIcon)) {
  console.error(`源图标不存在：${sourceIcon}`)
  process.exit(1)
}

const require = createRequire(import.meta.url)
const tauriCli = require.resolve('@tauri-apps/cli/tauri.js', {
  paths: [appDirectory],
})

execFileSync(
  process.execPath,
  [
    tauriCli,
    'icon',
    sourceIcon,
    '--output',
    tauriIconsDirectory,
    '--ios-color',
    '#161616',
  ],
  { cwd: rootDirectory, stdio: 'inherit' },
)

fs.mkdirSync(publicDirectory, { recursive: true })
fs.copyFileSync(path.join(tauriIconsDirectory, '128x128.png'), favicon)

console.log(`已更新 Web 图标：${path.relative(rootDirectory, favicon)}`)
console.log(`已从 ${path.relative(rootDirectory, sourceIcon)} 刷新全部图标`)
