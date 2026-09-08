//! 图像读取域：查看 / 编辑 / 批量转换三个窗口共用。
//!
//! - [`info`]：同目录图片列表、元信息（尺寸 / 格式 / EXIF）读取；
//! - [`decode`]：WebView 无法直接显示的格式解码（PNG data URL / RGBA8 裸像素）；
//! - [`native_heic`]：平台原生 HEIC 解码（WIC / Image I/O）。

pub mod decode;
pub mod info;
pub mod native_heic;
