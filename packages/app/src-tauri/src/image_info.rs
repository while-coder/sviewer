//! 图片元信息：同目录列表、文件大小、尺寸、格式、EXIF。

use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::launch::is_supported;

#[derive(Serialize)]
pub(crate) struct ExifEntry {
    tag: String,
    value: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ImageInfo {
    path: String,
    file_name: String,
    size: u64,
    width: u32,
    height: u32,
    format: String,
    exif: Vec<ExifEntry>,
}

/// 列出与给定图片同目录、受支持的全部图片（按文件名排序），用于左右切换。
#[tauri::command]
pub(crate) fn list_dir_images(path: String) -> Vec<String> {
    let p = PathBuf::from(&path);
    let dir = match p.parent() {
        Some(d) => d,
        None => return vec![path],
    };
    let mut files: Vec<PathBuf> = match std::fs::read_dir(dir) {
        Ok(rd) => rd
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| p.is_file() && is_supported(p))
            .collect(),
        Err(_) => return vec![path],
    };
    files.sort_by(|a, b| {
        a.file_name()
            .unwrap_or_default()
            .to_ascii_lowercase()
            .cmp(&b.file_name().unwrap_or_default().to_ascii_lowercase())
    });
    files
        .into_iter()
        .map(|p| p.to_string_lossy().into_owned())
        .collect()
}

/// 读取 EXIF（失败或无 EXIF 时返回空）。跳过过长字段（如 MakerNote）避免污染面板。
fn read_exif(path: &Path) -> Vec<ExifEntry> {
    let file = match std::fs::File::open(path) {
        Ok(f) => f,
        Err(_) => return Vec::new(),
    };
    let mut reader = std::io::BufReader::new(&file);
    let exif = match exif::Reader::new().read_from_container(&mut reader) {
        Ok(e) => e,
        Err(_) => return Vec::new(),
    };
    exif.fields()
        .map(|f| ExifEntry {
            tag: f.tag.to_string(),
            value: f.display_value().with_unit(&exif).to_string(),
        })
        .filter(|e| e.value.len() <= 200)
        .collect()
}

/// 读取图片元信息：文件大小、尺寸、格式、EXIF。
#[tauri::command]
pub(crate) fn read_image_info(path: String) -> Result<ImageInfo, String> {
    let p = PathBuf::from(&path);
    let size = std::fs::metadata(&p).map(|m| m.len()).unwrap_or(0);
    let exif = read_exif(&p);
    // 按内容嗅探格式（jpe/jfif 等别名扩展名靠这一步识别），拿不到再退 EXIF 尺寸
    let reader = image::ImageReader::open(&p)
        .ok()
        .and_then(|r| r.with_guessed_format().ok());
    let fmt = reader.as_ref().and_then(|r| r.format());
    let (width, height) = match reader.map(|r| r.into_dimensions()) {
        Some(Ok(d)) => d,
        _ => exif_dimensions(&exif),
    };
    let format = fmt
        .map(|f| format!("{:?}", f).to_uppercase())
        .unwrap_or_else(|| {
            p.extension()
                .and_then(|e| e.to_str())
                .unwrap_or("?")
                .to_uppercase()
        });
    Ok(ImageInfo {
        file_name: p
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default(),
        path,
        size,
        width,
        height,
        format,
        exif,
    })
}

/// 从 EXIF 字段里取 PixelXDimension / PixelYDimension（HEIC 等格式的尺寸来源）。
/// 值形如 "4032 pixels"，只取首段数字。
fn exif_dimensions(exif: &[ExifEntry]) -> (u32, u32) {
    let num = |v: &str| v.split_whitespace().next().and_then(|s| s.parse().ok()).unwrap_or(0);
    let mut dim = (0u32, 0u32);
    for e in exif {
        match e.tag.as_str() {
            "PixelXDimension" => dim.0 = num(&e.value),
            "PixelYDimension" => dim.1 = num(&e.value),
            _ => {}
        }
    }
    dim
}

/// 读取 EXIF Orientation（无则 None）。编辑管线的 EXIF 归一化用。
pub(crate) fn exif_orientation(path: &Path) -> Option<image::metadata::Orientation> {
    let file = std::fs::File::open(path).ok()?;
    let mut reader = std::io::BufReader::new(&file);
    let exif = exif::Reader::new().read_from_container(&mut reader).ok()?;
    let field = exif.get_field(exif::Tag::Orientation, exif::In::PRIMARY)?;
    let v = field.value.get_uint(0)?;
    image::metadata::Orientation::from_exif(u8::try_from(v).ok()?)
}
