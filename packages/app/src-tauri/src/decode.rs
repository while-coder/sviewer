//! 解码：WebView 无法直接渲染的格式转 PNG data URL / RGBA8 裸像素。
//!
//! HEIC/HEIF 走平台原生解码（native_heic），其余按内容嗅探走 `image` crate。

use base64::{engine::general_purpose::STANDARD, Engine as _};

use crate::native_heic;

/// 把 WebView 无法直接渲染的格式解码为 PNG，返回 data URL。
///
/// HEIC/HEIF 需要 native libheif，`image` crate 目前不支持，会在此返回明确错误，
/// 由前端展示提示。后续若要支持，可接入 libheif-rs 并在此分支处理。
#[tauri::command]
pub(crate) fn decode_to_png(path: String) -> Result<String, String> {
    let img = image::open(&path).map_err(|e| format!("解码失败：{e}"))?;
    to_png_data_url(&img)
}

/// DynamicImage → PNG data URL（decode_to_png / decode_thumb 共用）。
fn to_png_data_url(img: &image::DynamicImage) -> Result<String, String> {
    let mut buf = std::io::Cursor::new(Vec::new());
    img.write_to(&mut buf, image::ImageFormat::Png)
        .map_err(|e| format!("编码 PNG 失败：{e}"))?;
    let b64 = STANDARD.encode(buf.get_ref());
    Ok(format!("data:image/png;base64,{b64}"))
}

/// 解码为缩略图 PNG data URL（最长边 ≤ max_px），批量转换列表用。
/// web 原生格式前端直接走 asset 协议，只有 HEIC/TIFF 等才需要本命令。
#[tauri::command]
pub(crate) fn decode_thumb(path: String, max_px: u32) -> Result<String, String> {
    let img = decode_any(&path)?;
    to_png_data_url(&img.thumbnail(max_px.max(1), max_px.max(1)))
}

/// 原生解码 HEIC/HEIF：Windows 走 WIC、macOS 走 Image I/O，
/// 返回 8 字节头（宽、高，u32 LE）+ RGBA8 裸像素（ipc::Response 零序列化开销）。
/// Linux 或系统未装 HEIF/HEVC 解码扩展时返回 Err，前端自动回退 libheif WASM。
#[tauri::command]
pub(crate) fn decode_heic(path: String) -> Result<tauri::ipc::Response, String> {
    native_heic::decode(&path).map(tauri::ipc::Response::new)
}

/// 任意格式解码为「头 + RGBA8」裸像素（主窗口 canvas 直显）。
/// 与 decode_to_png 同源（decode_any），但省掉 PNG 编码 + 前端再解码两趟；
/// HEIC/HEIF 走 decode_heic，不经此命令。与 decode_to_png 一样不套用 EXIF 方向。
#[tauri::command]
pub(crate) fn decode_raw(path: String) -> Result<tauri::ipc::Response, String> {
    let img = decode_any(&path)?;
    let rgba = img.to_rgba8();
    let (w, h) = (rgba.width(), rgba.height());
    let mut out = Vec::with_capacity(native_heic::HEADER_LEN + w as usize * h as usize * 4);
    out.extend_from_slice(&w.to_le_bytes());
    out.extend_from_slice(&h.to_le_bytes());
    out.extend_from_slice(rgba.as_raw());
    Ok(tauri::ipc::Response::new(out))
}

/// 任意受支持格式 → DynamicImage（HEIC 走原生解码，其余走 image crate）。
/// 解码管线唯一入口：编辑 / 缩略图 / 裸像素显示共用。
pub(crate) fn decode_any(path: &str) -> Result<image::DynamicImage, String> {
    let p = std::path::PathBuf::from(path);
    let ext = p
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_lowercase();
    if ext == "heic" || ext == "heif" || ext == "hif" {
        let buf = native_heic::decode(path)?;
        if buf.len() < native_heic::HEADER_LEN {
            return Err("解码数据不完整".into());
        }
        let w = u32::from_le_bytes([buf[0], buf[1], buf[2], buf[3]]);
        let h = u32::from_le_bytes([buf[4], buf[5], buf[6], buf[7]]);
        image::RgbaImage::from_raw(w, h, buf[native_heic::HEADER_LEN..].to_vec())
            .ok_or_else(|| "解码数据长度不符".to_string())
            .map(Into::into)
    } else {
        // 按内容嗅探格式：jpe/jfif 等别名扩展名 image::open 认不出，内容识别都能走通
        image::ImageReader::open(&p)
            .map_err(|e| format!("打开失败：{e}"))?
            .with_guessed_format()
            .map_err(|e| format!("识别格式失败：{e}"))?
            .decode()
            .map_err(|e| format!("解码失败：{e}"))
    }
}
