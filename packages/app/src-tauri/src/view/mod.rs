//! 查看域：图像读取与展示信息（对应前端 features/view）。
//!
//! 同目录列表、解码、EXIF 由查看 / 编辑 / 批量转换三个窗口共用；
//! 逆地理编码与前端 view/lib/geo.ts 同域。
//!
//! - [`info`]：同目录图片列表、元信息（尺寸 / 格式 / EXIF）读取；
//! - [`decode`]：WebView 无法直接显示的格式解码（分发 + heic / psd 特殊格式子模块）；
//! - [`geo`]：EXIF GPS 坐标 → 简略地名（详情抽屉用）。

pub mod decode;
pub mod geo;
pub mod info;
