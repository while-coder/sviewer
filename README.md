# 速阅

轻量级本地图片查看器（Windows / macOS），Tauri 2 + Vue 3 实现。
不联网上传任何图片；HEIC/HEIF 走系统原生解码（Windows WIC / macOS Image I/O），
解码失败时前端自动回退 libheif WASM。

## 功能

- 看图：适应窗口 / 1:1 / 滚轮与快捷键缩放拖拽、同目录左右切换（空闲预载邻居）、全屏
- 快捷编辑：旋转 / 镜像后直接写回原图（仅可编码格式）
- 编辑窗口：框选裁剪、改尺寸、标记（画笔 / 矩形 / 椭圆 / 箭头），SVG 预览所见即所得
- 另存为：对话框里直接换格式（JPEG 可调质量）；无编辑且同格式时原样复制字节不变
- 信息面板：EXIF 中文标签、GPS 坐标 + 逆地理地名（OSM / 高德 / 百度，可配 Key）
- 批量转换：独立窗口，重名自动 `-2` 后缀，单项失败不中断
- 格式关联（Windows，只写 HKCU 免管理员）：设置弹窗内一键设为默认打开方式
- 单实例：默认多开收敛为一个窗口，「允许」多开可在设置里开关

## 支持格式

清单的唯一来源是 [packages/app/src/lib/formats/formats.json](packages/app/src/lib/formats/formats.json)：

- **可查看**（25 种）：jpg / jpeg / jpe / jfif、png、gif、webp、bmp、ico、svg、
  tiff / tif、avif、heic / heif / hif、tga、pbm / pgm / ppm / pnm、dds、hdr、exr、qoi
- **可直接写回原图**：jpg、png、webp、bmp、tiff、avif、tga、qoi、exr
  （heic 无编码器、svg 是矢量、gif 动图会丢帧，前端禁用保存按钮）
- **批量转换可作源**：web 原生格式 + tiff + heic / heif（svg 矢量不参与）

## 开发

要求：Node 20+、pnpm 10、Rust（含各平台 Tauri 依赖）。

```bash
pnpm install          # 安装依赖
pnpm dev              # 前端 dev server（浏览器预览，Tauri API 不可用）
pnpm tauri dev        # 桌面应用开发模式
pnpm build:web        # 仅构建前端（vite MPA：index / edit / batch 三入口）
pnpm build:app        # 打包 Windows 安装包（build:app:mac 打 macOS）
```

Rust 单元测试：

```bash
cd packages/app/src-tauri
cargo test --lib
```

## 目录结构

```
packages/
├── app/                        # 应用主体
│   ├── index.html / edit.html / batch.html   # 三个 MPA 入口（留在包根）
│   └── src/
│       ├── entries/            # 窗口入口：create-window-app.ts 工厂 + main/edit/batch 各 3 行
│       ├── features/           # 窗口层按功能域划分
│       │   ├── view/           # 主窗口：ViewerWindow + settings/（设置/关于）+ lib/（菜单/EXIF/地理）
│       │   ├── edit/           # 编辑窗口
│       │   └── convert/        # 批量转换窗口
│       ├── composables/        # 跨窗口共享逻辑：视图变换 / 保存可用性 / 主题同步 / 原生菜单
│       ├── lib/                # 无 UI 的领域层，按功能域归组：
│       │   ├── formats/        #   格式事实源 formats.json + 派生判定 formats.ts
│       │   ├── decode/         #   显示策略 decode.ts + 解码子线程 decode-worker.ts
│       │   ├── bridge/         #   invoke 封装 bridge.ts + 另存为流程 save.ts
│       │   └── types / settings / logger / util.ts   # 跨域通用单件
│       └── styles/common.css   # 主题变量 + 两窗口共用的画布基础样式
└── icon/                       # 图标源文件（scripts/sync-icons.js 分发到各平台）

packages/app/src-tauri/src/     # Rust 后端
├── lib.rs                      # run() / 插件注册 / invoke_handler（~160 行）
├── launch.rs                   # 启动文件交接、单实例/多开、受支持格式判定
├── image/                      # 图像读取域（查看/编辑/转换共用）
│   ├── info.rs                 #   同目录列表、尺寸 / 格式 / EXIF
│   ├── decode.rs               #   解码：PNG data URL / RGBA8 裸像素 / HEIC 原生
│   └── native_heic.rs          #   平台原生 HEIC 解码（WIC / Image I/O）
├── edit/                       # 编辑落盘域（编辑窗口与批量转换共用）
│   ├── pipeline.rs             #   编辑管线与编码落盘
│   └── marks.rs                #   标记绘制光栅化
├── assoc.rs / geo.rs           # 格式关联 / 逆地理编码（Tauri 命令定义在各自模块内）
└── formats_gen.rs              # 生成物（scripts/gen-formats.cjs 生成，勿手改）
```

## 修改支持格式清单

格式清单是「单一事实源 + 生成物」模式：

1. 改 [packages/app/src/lib/formats/formats.json](packages/app/src/lib/formats/formats.json)；
2. 跑 `pnpm gen:formats` —— 自动重写 Rust 的 `formats_gen.rs` 和
   `tauri.conf.json` 的 `fileAssociations`；
3. 前端侧的 `lib/formats/formats.ts` 全部从 formats.json 派生，无需手改；
4. 生成物提交入库（不用构建钩子，避免拖慢每次构建）。

## 发版

```bash
pnpm release     # 交互式 bump 版本：根 package.json、Cargo.toml 一起改并 git add
```

打包产物由 CI 发布（见 `.github/workflows`），更新走 tauri-updater-kit。
