# Changelog

SViewer（素阅）的所有显著变更都将记录在此文件中。

格式参考 [Keep a Changelog](https://keepachangelog.com/zh-CN/1.1.0/)。

## 0.0.2 - 2026-09-03

### 修复

**macOS**
- 修复 Finder 双击 / 右键「打开方式」打开图片时只显示空白默认界面的问题（macOS 经 Apple Event 投递文件，此前未处理）。
- 修复 HEIC/HEIF、AVIF、TGA、EXR 等格式在系统「打开方式」中找不到素阅的问题（安装包现按标准 UTI 声明各图片类型）。

**Linux**
- 修复安装 deb 后文件管理器「打开方式」中不出现素阅、以及双击关联文件不传入图片路径的问题（补齐 .desktop 的 MimeType 声明与 `%f` 参数）。

**通用**
- 修复「另存为」可能重复打开保存对话框的问题。

### 新增

- 原生菜单各菜单项补充快捷键（打开 Cmd/Ctrl+O、另存为 Cmd/Ctrl+S、适应窗口 Cmd/Ctrl+0、原始大小 Cmd/Ctrl+1、信息面板 Cmd/Ctrl+I、设置 Cmd/Ctrl+,）。
- macOS 应用菜单本地化：简文系统下隐藏/退出等系统菜单项显示中文；菜单栏应用名与 Finder 显示名改为「素阅」。
- 右键菜单快捷键提示在 macOS 上显示为 ⌘S。

### 变更

- 设置中的「格式关联」页签仅在 Windows 显示（macOS 经系统「打开方式」关联，Linux 经系统设置关联）。
- macOS 上按 Esc 仅逐层关闭浮层，不再最小化或退出窗口。

## 0.0.1 - 2026-09-02

首个公开版本。

### 新增

**图片查看**
- 主查看窗口，支持打开单个图片文件。
- 缩放、平移、旋转、镜像，提供「适应窗口」与「原始大小」快捷视图。
- 信息面板：显示图片尺寸、格式、文件大小等详情。

**格式支持**
- 常见格式：JPEG（jpg/jpeg/jpe/jfif）、PNG、GIF、WebP、BMP、ICO、SVG、AVIF、TIFF、TGA、PNM（pbm/pgm/ppm/pnm）。
- 专业 / 游戏格式：DDS、HDR、EXR、QOI。
- HEIC / HEIF / HIF：基于 libheif（Web Worker 中解码），iPhone 拍摄的 HEIC 照片可直接查看。

**编辑与转换**
- 编辑窗口：对图片进行编辑后另存为其他格式。
- 批量转换窗口：批量将多张图片转换为目标格式。

**系统集成**
- 文件格式关联：安装后可将上述图片格式设为默认打开方式，双击文件直接用素阅查看。
- 原生系统菜单（文件 / 视图，macOS 为应用菜单），含「打开…」「另存为…」「批量转换…」「设置…」「关于素阅」等项。

**其他**
- 设置持久化。
- 自动更新：内置更新器，从 GitHub Releases 检查并安装新版本。
- CI/CD：GitHub Actions 自动构建发布，`scripts/release.cjs` 一键发版。
- 多平台打包：Windows（NSIS）、macOS（App / DMG）、Linux（deb / AppImage）。
