# Changelog

SViewer（素阅）的所有显著变更都将记录在此文件中。

格式参考 [Keep a Changelog](https://keepachangelog.com/zh-CN/1.1.0/)。

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
