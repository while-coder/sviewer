# Changelog

SViewer（素阅）的所有显著变更都将记录在此文件中。

格式参考 [Keep a Changelog](https://keepachangelog.com/zh-CN/1.1.0/)。

## 0.0.3

### 新增

- 信息面板新增「位置」区块：解析 EXIF 中的 GPS 经纬度，显示十进制坐标，并提供高德、百度、Google、Apple 四个地图按钮，点击后在浏览器中打开对应地图定位（自动声明 WGS-84 坐标系，由地图侧转换）。
- 详情抽屉「位置」区块新增地名解析（逆地理编码）：把 GPS 坐标转成一行简略地名。服务可选 OSM/Nominatim（免 Key，默认）、高德、百度（各自在设置里填 Key），也可关闭保持纯离线。仅在打开详情抽屉时查询，结果按坐标缓存去重，请求限速约 1 条/秒；高德坐标自动做 WGS-84 → GCJ-02 纠偏，百度直接声明 WGS-84。

### 变更

- 非 web 原生格式（HEIC/HEIF、TIFF/EXR/TGA 等）显示链路统一重做：解码像素在子线程直接转成 ImageBitmap 交 canvas 绘制，去掉原来的 WebP/PNG 重编码与二次解码。HEIC 打开速度从约 1-2 秒降到零点几秒（12MP 实测系统解码 231ms），大 TIFF 同样受益；Windows 下改由 WIC 直接输出 RGBA，省掉一次逐像素颜色序交换。
- 翻页时在空闲时段预载同目录左右相邻图片（含全部非原生格式），来回切换直接命中缓存出图；位图缓存带像素总量上限，超大图（48MP）自动少缓存防内存失控。

## 0.0.2

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

## 0.0.1

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
