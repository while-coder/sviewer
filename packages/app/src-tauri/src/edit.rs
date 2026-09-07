//! 编辑管线与编码落盘：另存为 / 保存到原图 / 批量转换共用。
//!
//! 管线：EXIF 归一化 → 旋转 → 镜像 → 裁剪 → 改尺寸 → 标记烘焙（marks 模块）
//! → 按目标格式 + 质量编码写盘。

use std::path::{Path, PathBuf};

use crate::decode::decode_any;
use crate::formats_gen;
use crate::image_info::exif_orientation;
use crate::marks::bake_marks;

/// 裁剪矩形：显示空间（EXIF 归一化 + 旋转 + 镜像之后）的像素坐标，左上原点。
#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CropRect {
    pub(crate) x: u32,
    pub(crate) y: u32,
    pub(crate) w: u32,
    pub(crate) h: u32,
}

/// 一条标记笔画：显示空间（EXIF 归一化 + 旋转 + 镜像之后，裁剪前）的像素坐标。
/// kind：rect / ellipse（pts 为对角两点）、arrow（起点→终点）、pen（折线点集）。
#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Mark {
    pub(crate) kind: String,
    /// "#rrggbb"
    pub(crate) color: String,
    /// 线宽（显示空间像素）
    pub(crate) width: f64,
    pub(crate) pts: Vec<(f64, f64)>,
}

/// 一次编辑/转换的完整参数。crop/resize/quality 为 None 即不做该步。
#[derive(Default, serde::Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub(crate) struct ImageEdits {
    pub(crate) rotation: u32,
    pub(crate) flip: bool,
    pub(crate) crop: Option<CropRect>,
    pub(crate) resize: Option<(u32, u32)>,
    pub(crate) quality: Option<u8>,
    #[serde(default)]
    pub(crate) marks: Vec<Mark>,
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SaveOutcome {
    dest: String,
    size: u64,
}

/// 目标格式名 → ImageFormat。
pub(crate) fn parse_format(format: &str) -> Result<image::ImageFormat, String> {
    Ok(match format {
        "jpeg" => image::ImageFormat::Jpeg,
        "png" => image::ImageFormat::Png,
        "webp" => image::ImageFormat::WebP,
        "bmp" => image::ImageFormat::Bmp,
        "tiff" => image::ImageFormat::Tiff,
        "gif" => image::ImageFormat::Gif,
        "ico" => image::ImageFormat::Ico,
        "tga" => image::ImageFormat::Tga,
        "ppm" => image::ImageFormat::Pnm,
        "qoi" => image::ImageFormat::Qoi,
        "ff" => image::ImageFormat::Farbfeld,
        "avif" => image::ImageFormat::Avif,
        "exr" => image::ImageFormat::OpenExr,
        _ => return Err(format!("不支持的保存格式：{format}")),
    })
}

/// 编辑管线：EXIF 归一化 → 旋转 → 镜像 → 裁剪 → 改尺寸。
///
/// 裁剪矩形定义在「显示空间」（前三步之后的轴对齐坐标系）——前端 WebView 显示
/// 原生格式时会应用 EXIF Orientation，裁剪框在摆正后的画面上框选，编码侧必须
/// 做同样的归一化才能对齐；旋转/镜像之后像素与所见一致，两边零换算。
fn process_image(
    mut img: image::DynamicImage,
    path: &Path,
    edits: &ImageEdits,
) -> Result<image::DynamicImage, String> {
    if let Some(o) = exif_orientation(path) {
        img.apply_orientation(o);
    }
    if !matches!(edits.rotation, 0 | 90 | 180 | 270) {
        return Err(format!("无效的旋转角度：{}", edits.rotation));
    }
    match edits.rotation {
        90 => img = img.rotate90(),
        180 => img = img.rotate180(),
        270 => img = img.rotate270(),
        _ => {}
    }
    if edits.flip {
        img = img.fliph();
    }
    // 标记坐标换算基准：裁剪偏移 + 改尺寸缩放（见 marks::bake_marks）
    let (mut crop_x, mut crop_y) = (0.0f64, 0.0f64);
    if let Some(c) = &edits.crop {
        // 与图片边界求交集；交集为空说明是空框/完全越界，静默忽略
        let x = c.x.min(img.width());
        let y = c.y.min(img.height());
        let w = c.w.min(img.width() - x);
        let h = c.h.min(img.height() - y);
        if w > 0 && h > 0 {
            img = img.crop_imm(x, y, w, h);
            crop_x = x as f64;
            crop_y = y as f64;
        }
    }
    let (mut sx, mut sy) = (1.0f64, 1.0f64);
    if let Some((rw, rh)) = edits.resize {
        let (rw, rh) = (rw.max(1), rh.max(1));
        if rw != img.width() || rh != img.height() {
            sx = rw as f64 / img.width() as f64;
            sy = rh as f64 / img.height() as f64;
            img = img.resize_exact(rw, rh, image::imageops::FilterType::Lanczos3);
        }
    }
    if !edits.marks.is_empty() {
        img = bake_marks(img, &edits.marks, crop_x, crop_y, sx, sy);
    }
    Ok(img)
}

/// 把处理好的图片按目标格式 + 质量编码落盘，返回文件字节数。
fn encode_image_to(
    mut img: image::DynamicImage,
    dest: &Path,
    format: image::ImageFormat,
    quality: Option<u8>,
) -> Result<u64, String> {
    // ICO 规范要求尺寸 ≤256×256：等比缩进 256 内再编码
    if format == image::ImageFormat::Ico {
        let m = img.width().max(img.height());
        if m > 256 {
            img = img.thumbnail(256, 256);
        }
    }
    // farbfeld 编码器只接受 16 位 RGBA
    if format == image::ImageFormat::Farbfeld {
        img = image::DynamicImage::ImageRgba16(img.to_rgba16());
    }
    if format == image::ImageFormat::Jpeg {
        // save_with_format 不带质量参数，JPEG 走编码器指定（1~100）
        let f = std::fs::File::create(dest).map_err(|e| format!("保存失败：{e}"))?;
        let enc = image::codecs::jpeg::JpegEncoder::new_with_quality(
            std::io::BufWriter::new(f),
            quality.unwrap_or(85),
        );
        img.to_rgb8()
            .write_with_encoder(enc)
            .map_err(|e| format!("保存失败：{e}"))?;
    } else {
        img.save_with_format(dest, format)
            .map_err(|e| format!("保存失败：{e}"))?;
    }
    std::fs::metadata(dest)
        .map(|m| m.len())
        .map_err(|e| format!("读取结果失败：{e}"))
}

/// 统一编码入口：解码 src → 编辑管线 → 按格式+质量写入 dest。
/// 另存为 / 批量转换 / 单图编辑共用；edits=None 等价于不做任何编辑。
/// HEIC 源走平台原生解码（decode_any），因此 HEIC 可转出为任意可编码格式。
#[tauri::command]
pub(crate) fn encode_image(
    src: String,
    dest: String,
    format: String,
    quality: Option<u8>,
    edits: Option<ImageEdits>,
) -> Result<SaveOutcome, String> {
    let fmt = parse_format(&format)?;
    let mut img = decode_any(&src)?;
    if let Some(e) = &edits {
        img = process_image(img, Path::new(&src), e)?;
    }
    let size = encode_image_to(img, Path::new(&dest), fmt, quality)?;
    Ok(SaveOutcome { dest, size })
}

/// 把编辑（旋转/镜像/裁剪/改尺寸）烘焙后写回原图。
#[tauri::command]
pub(crate) fn save_edits(path: String, edits: ImageEdits) -> Result<SaveOutcome, String> {
    let p = PathBuf::from(&path);
    let ext = p
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_lowercase();
    // HEIC 无编码器、SVG 是矢量、GIF 动图会丢帧：前端禁用按钮，这里兜底拒绝
    let Some(format) = formats_gen::editable_ext_format(&ext) else {
        return Err(format!(".{ext} 格式不支持直接修改原图"));
    };
    let img = decode_any(&path)?;
    let img = process_image(img, &p, &edits)?;
    let size = encode_image_to(img, &p, parse_format(format)?, edits.quality)?;
    Ok(SaveOutcome { dest: path, size })
}

/// 目标路径已存在时自动加 -2/-3… 后缀（p-2.jpg，不是 p.jpg-2），返回不冲突的路径。
/// 批量转换逐项串行调用：同批同名文件因前一项已落盘也能正确错开。
#[tauri::command]
pub(crate) fn unique_dest(dest: String) -> String {
    let p = PathBuf::from(&dest);
    if !p.exists() {
        return dest;
    }
    let dir = p.parent().map(|d| d.to_path_buf()).unwrap_or_default();
    let stem = p
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("image")
        .to_string();
    let ext = p
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| format!(".{e}"))
        .unwrap_or_default();
    for i in 2.. {
        let cand = dir.join(format!("{stem}-{i}{ext}"));
        if !cand.exists() {
            return cand.to_string_lossy().into_owned();
        }
    }
    unreachable!()
}

/// 另存为：把图片保存到目标路径。
/// format：original = 原样复制（保留原始字节）；其余为重编码的目标格式
/// （jpeg/png/webp/bmp/tiff/gif/ico/tga/ppm/qoi/ff/avif）。
#[tauri::command]
pub(crate) fn save_image_as(src: String, dest: String, format: String) -> Result<(), String> {
    if format == "original" {
        return std::fs::copy(&src, &dest)
            .map(|_| ())
            .map_err(|e| format!("保存失败：{e}"));
    }
    encode_image(src, dest, format, None, None).map(|_| ())
}

#[cfg(test)]
mod tests {
    /// 手动冒烟测试：`SVIEWER_SAVE_SRC=<图片路径> [SVIEWER_SAVE_FORMAT=jpeg|png|webp|bmp|tiff] cargo test --lib -- --nocapture`
    /// 验证另存为的重编码路径；未设环境变量时静默跳过。
    #[test]
    fn save_as_convert() {
        let Ok(src) = std::env::var("SVIEWER_SAVE_SRC") else {
            return;
        };
        let format = std::env::var("SVIEWER_SAVE_FORMAT").unwrap_or_else(|_| "jpeg".into());
        let ext = if format == "jpeg" { "jpg" } else { &format };
        let dest = std::env::var("SVIEWER_SAVE_DEST").unwrap_or_else(|_| {
            std::env::temp_dir()
                .join(format!("sviewer_save_test.{ext}"))
                .to_string_lossy()
                .into_owned()
        });
        super::save_image_as(src, dest.clone(), format).expect("转换保存失败");
        let meta = std::fs::metadata(&dest).expect("目标文件不存在");
        assert!(meta.len() > 0);
        println!("已保存 {dest}（{} 字节）", meta.len());
    }

    /// 旋转写回原图：40×30 的临时 JPEG 转 90° 后重开应为 30×40。
    #[test]
    fn save_edits_rotates_in_place() {
        let path = std::env::temp_dir().join("sviewer_transform_test.jpg");
        image::RgbImage::from_pixel(40, 30, image::Rgb([255, 0, 0]))
            .save_with_format(&path, image::ImageFormat::Jpeg)
            .expect("生成测试图失败");
        super::save_edits(
            path.to_string_lossy().into_owned(),
            super::ImageEdits {
                rotation: 90,
                flip: false,
                ..Default::default()
            },
        )
        .expect("save_edits 失败");
        let (w, h) = image::image_dimensions(&path).expect("重开失败");
        assert_eq!((w, h), (30, 40), "旋转 90° 后宽高应互换");
        std::fs::remove_file(&path).ok();
    }

    /// 编辑管线：旋转 90° 后裁剪 {5,5,10,10}，尺寸应为 10×10。
    #[test]
    fn apply_edits_pipeline() {
        let path = std::env::temp_dir().join("sviewer_edits_pipeline.jpg");
        image::RgbImage::from_pixel(40, 30, image::Rgb([255, 0, 0]))
            .save_with_format(&path, image::ImageFormat::Jpeg)
            .expect("生成测试图失败");
        super::save_edits(
            path.to_string_lossy().into_owned(),
            super::ImageEdits {
                rotation: 90,
                flip: false,
                crop: Some(super::CropRect { x: 5, y: 5, w: 10, h: 10 }),
                resize: None,
                quality: None,
                marks: Vec::new(),
            },
        )
        .expect("save_edits 失败");
        let (w, h) = image::image_dimensions(&path).expect("重开失败");
        assert_eq!((w, h), (10, 10), "旋转后裁剪应得 10×10");
        std::fs::remove_file(&path).ok();
    }

    /// 裁剪完全越界：取交集（5×5）而不是报错。
    #[test]
    fn apply_edits_crop_out_of_bounds() {
        let path = std::env::temp_dir().join("sviewer_edits_oob.jpg");
        image::RgbImage::from_pixel(40, 30, image::Rgb([0, 255, 0]))
            .save_with_format(&path, image::ImageFormat::Jpeg)
            .expect("生成测试图失败");
        super::save_edits(
            path.to_string_lossy().into_owned(),
            super::ImageEdits {
                rotation: 0,
                flip: false,
                crop: Some(super::CropRect { x: 35, y: 25, w: 100, h: 100 }),
                resize: None,
                quality: None,
                marks: Vec::new(),
            },
        )
        .expect("越界裁剪不应报错");
        let (w, h) = image::image_dimensions(&path).expect("重开失败");
        assert_eq!((w, h), (5, 5), "越界裁剪应取交集 5×5");
        std::fs::remove_file(&path).ok();
    }

    /// 改尺寸：40×30 → 20×15。
    #[test]
    fn apply_edits_resize() {
        let path = std::env::temp_dir().join("sviewer_edits_resize.png");
        image::RgbImage::from_pixel(40, 30, image::Rgb([0, 0, 255]))
            .save_with_format(&path, image::ImageFormat::Png)
            .expect("生成测试图失败");
        super::save_edits(
            path.to_string_lossy().into_owned(),
            super::ImageEdits {
                rotation: 0,
                flip: false,
                crop: None,
                resize: Some((20, 15)),
                quality: None,
                marks: Vec::new(),
            },
        )
        .expect("save_edits 失败");
        let (w, h) = image::image_dimensions(&path).expect("重开失败");
        assert_eq!((w, h), (20, 15));
        std::fs::remove_file(&path).ok();
    }

    /// JPEG 质量：同一张噪声图 q10 应显著小于 q95。
    #[test]
    fn encode_image_jpeg_quality() {
        let src = std::env::temp_dir().join("sviewer_quality_src.png");
        let mut img = image::RgbImage::new(200, 200);
        let mut seed = 0x1234_5678u32;
        for px in img.pixels_mut() {
            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            *px = image::Rgb([seed as u8, (seed >> 8) as u8, (seed >> 16) as u8]);
        }
        img.save_with_format(&src, image::ImageFormat::Png)
            .expect("生成测试图失败");
        let src = src.to_string_lossy().into_owned();
        let small = super::encode_image(
            src.clone(),
            std::env::temp_dir().join("sviewer_q10.jpg").to_string_lossy().into_owned(),
            "jpeg".into(),
            Some(10),
            None,
        )
        .expect("q10 编码失败");
        let big = super::encode_image(
            src,
            std::env::temp_dir().join("sviewer_q95.jpg").to_string_lossy().into_owned(),
            "jpeg".into(),
            Some(95),
            None,
        )
        .expect("q95 编码失败");
        assert!(small.size < big.size, "q10({}) 应小于 q95({})", small.size, big.size);
    }

    /// 重名自动 -2/-3 后缀。
    #[test]
    fn unique_dest_suffix() {
        let dir = std::env::temp_dir().join(format!("sviewer_utest_{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("建临时目录失败");
        let a = dir.join("a.jpg");
        let u = |p: &std::path::Path| super::unique_dest(p.to_string_lossy().into_owned());
        assert_eq!(u(&a), a.to_string_lossy(), "不存在时原样返回");
        std::fs::write(&a, b"x").unwrap();
        assert_eq!(u(&a), dir.join("a-2.jpg").to_string_lossy());
        std::fs::write(dir.join("a-2.jpg"), b"x").unwrap();
        assert_eq!(u(&a), dir.join("a-3.jpg").to_string_lossy());
        std::fs::remove_dir_all(&dir).ok();
    }
}
