//! SViewer —— 独立图片查看器后端。
//!
//! 职责：
//! - 接收启动文件（双击关联 / 命令行）并在单实例下把后续打开请求转交给已运行的窗口；
//! - 提供同目录图片列表、元信息（尺寸 / 格式 / EXIF）读取；
//! - 对 WebView 无法直接显示的格式用 `image` crate 解码为 PNG data URL。

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use base64::{engine::general_purpose::STANDARD, Engine as _};
use serde::Serialize;
use tauri::{Emitter, Manager, State};

mod assoc;
mod native_heic;

/// 受支持的图片扩展名（小写，不含点）。用于目录列举与启动参数识别。
/// - jpe/jfif 是 JPEG 别名、hif 是 HEIF 容器，分别沿用 JPEG/HEIC 的解码通道；
/// - svg 交给 WebView 渲染；
/// - tga/pbm/pgm/ppm/pnm/dds/hdr/exr/qoi 由 image crate 解码（默认 feature 已带）。
const SUPPORTED_EXT: &[&str] = &[
    "jpg", "jpeg", "jpe", "jfif", "png", "gif", "webp", "bmp", "ico", "svg", "tiff", "tif",
    "avif", "heic", "heif", "hif", "tga", "pbm", "pgm", "ppm", "pnm", "dds", "hdr", "exr", "qoi",
];

/// 启动时待打开的文件路径，前端 onMounted 取走一次后清空。
#[derive(Default)]
struct LaunchFile(Mutex<Option<String>>);

/// 「允许多开」标记文件：%APPDATA%/com.while.sviewer/allow-multi-instance。
/// 前端设置里开关时由 set_multi_instance 写/删；启动时按它决定是否注册
/// single-instance 插件（localStorage 读不到，Rust 侧只认文件）。
fn multi_instance_flag_path() -> Option<PathBuf> {
    std::env::var("APPDATA")
        .ok()
        .map(|d| PathBuf::from(d).join("com.while.sviewer").join("allow-multi-instance"))
}

/// 当前是否允许多开。
fn multi_instance_enabled() -> bool {
    multi_instance_flag_path().is_some_and(|p| p.exists())
}

/// 设置「允许多开」：写/删标记文件，下次启动生效。
#[tauri::command]
fn set_multi_instance(enabled: bool) -> Result<(), String> {
    let Some(p) = multi_instance_flag_path() else {
        return Err("无法定位配置目录".into());
    };
    if let Some(dir) = p.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if enabled {
        std::fs::File::create(&p).map_err(|e| format!("写入标记失败：{e}"))?;
    } else {
        match std::fs::remove_file(&p) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(format!("删除标记失败：{e}")),
        }
    }
    Ok(())
}

#[derive(Serialize)]
struct ExifEntry {
    tag: String,
    value: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ImageInfo {
    path: String,
    file_name: String,
    size: u64,
    width: u32,
    height: u32,
    format: String,
    exif: Vec<ExifEntry>,
}

/// 判断扩展名是否受支持。
fn is_supported(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| SUPPORTED_EXT.contains(&e.to_lowercase().as_str()))
        .unwrap_or(false)
}

/// 从命令行参数中挑出第一个存在且受支持的图片路径。
fn pick_image_arg(argv: &[String]) -> Option<String> {
    argv.iter()
        .skip(1) // 跳过程序自身路径
        .map(PathBuf::from)
        .find(|p| p.is_file() && is_supported(p))
        .map(|p| p.to_string_lossy().into_owned())
}

/// 取走启动文件（取后清空，保证只触发一次）。
#[tauri::command]
fn get_launch_file(state: State<'_, LaunchFile>) -> Option<String> {
    state.0.lock().ok().and_then(|mut g| g.take())
}

/// 列出与给定图片同目录、受支持的全部图片（按文件名排序），用于左右切换。
#[tauri::command]
fn list_dir_images(path: String) -> Vec<String> {
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
fn read_image_info(path: String) -> Result<ImageInfo, String> {
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

/// 把 WebView 无法直接渲染的格式解码为 PNG，返回 data URL。
///
/// HEIC/HEIF 需要 native libheif，`image` crate 目前不支持，会在此返回明确错误，
/// 由前端展示提示。后续若要支持，可接入 libheif-rs 并在此分支处理。
#[tauri::command]
fn decode_to_png(path: String) -> Result<String, String> {
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
fn decode_thumb(path: String, max_px: u32) -> Result<String, String> {
    let img = decode_any(&path)?;
    to_png_data_url(&img.thumbnail(max_px.max(1), max_px.max(1)))
}

/// 原生解码 HEIC/HEIF：Windows 走 WIC、macOS 走 Image I/O，
/// 返回 8 字节头（宽、高，u32 LE）+ RGBA8 裸像素（ipc::Response 零序列化开销）。
/// Linux 或系统未装 HEIF/HEVC 解码扩展时返回 Err，前端自动回退 libheif WASM。
#[tauri::command]
fn decode_heic(path: String) -> Result<tauri::ipc::Response, String> {
    native_heic::decode(&path).map(tauri::ipc::Response::new)
}

/// 裁剪矩形：显示空间（EXIF 归一化 + 旋转 + 镜像之后）的像素坐标，左上原点。
#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct CropRect {
    x: u32,
    y: u32,
    w: u32,
    h: u32,
}

/// 一条标记笔画：显示空间（EXIF 归一化 + 旋转 + 镜像之后，裁剪前）的像素坐标。
/// kind：rect / ellipse（pts 为对角两点）、arrow（起点→终点）、pen（折线点集）。
#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct Mark {
    kind: String,
    /// "#rrggbb"
    color: String,
    /// 线宽（显示空间像素）
    width: f64,
    pts: Vec<(f64, f64)>,
}

/// 一次编辑/转换的完整参数。crop/resize/quality 为 None 即不做该步。
#[derive(Default, serde::Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct ImageEdits {
    rotation: u32,
    flip: bool,
    crop: Option<CropRect>,
    resize: Option<(u32, u32)>,
    quality: Option<u8>,
    #[serde(default)]
    marks: Vec<Mark>,
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct SaveOutcome {
    dest: String,
    size: u64,
}

/// 目标格式名 → ImageFormat。
fn parse_format(format: &str) -> Result<image::ImageFormat, String> {
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
    // 标记坐标换算基准：裁剪偏移 + 改尺寸缩放（见下方 marks 绘制）
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

// ── 标记绘制（矩形 / 椭圆 / 箭头 / 画笔，圆头笔触无抗锯齿）──

/// "#rgb" / "#rrggbb" → RGB 三元组。
fn parse_color(s: &str) -> Option<[u8; 3]> {
    let s = s.trim().trim_start_matches('#');
    let two = |b: &str| u8::from_str_radix(b, 16).ok();
    match s.len() {
        6 => Some([two(&s[0..2])?, two(&s[2..4])?, two(&s[4..6])?]),
        3 => {
            let mut c = [0u8; 3];
            for (i, ch) in s.chars().enumerate() {
                c[i] = two(&ch.to_string())?.wrapping_mul(17);
            }
            Some(c)
        }
        _ => None,
    }
}

/// 以 (cx, cy) 为心、r 为半径的实心圆盘刷上不透明颜色。
fn stamp_disc(img: &mut image::RgbaImage, cx: f64, cy: f64, r: f64, c: [u8; 3]) {
    let (w, h) = (img.width() as i64, img.height() as i64);
    for y in (cy - r).floor() as i64..=(cy + r).ceil() as i64 {
        for x in (cx - r).floor() as i64..=(cx + r).ceil() as i64 {
            if x < 0 || y < 0 || x >= w || y >= h {
                continue;
            }
            let dx = x as f64 - cx;
            let dy = y as f64 - cy;
            if dx * dx + dy * dy <= r * r {
                img.put_pixel(x as u32, y as u32, image::Rgba([c[0], c[1], c[2], 255]));
            }
        }
    }
}

/// 粗线段：沿线以半径 half 的圆盘步进盖章（步长 ≤ 半径/2 保证连续，圆头笔触）。
fn draw_stroke(img: &mut image::RgbaImage, x0: f64, y0: f64, x1: f64, y1: f64, half: f64, c: [u8; 3]) {
    let half = half.max(0.5);
    let len = ((x1 - x0).powi(2) + (y1 - y0).powi(2)).sqrt();
    let steps = (len / (half * 0.5)).ceil().max(1.0) as usize;
    for i in 0..=steps {
        let t = i as f64 / steps as f64;
        stamp_disc(img, x0 + (x1 - x0) * t, y0 + (y1 - y0) * t, half, c);
    }
}

/// 椭圆：对角两点定义。filled=false 时参数曲线采样描边。
fn draw_ellipse(
    img: &mut image::RgbaImage,
    x0: f64,
    y0: f64,
    x1: f64,
    y1: f64,
    half: f64,
    c: [u8; 3],
) {
    let (cx, cy) = ((x0 + x1) / 2.0, (y0 + y1) / 2.0);
    let (rx, ry) = (((x1 - x0) / 2.0).abs().max(0.5), ((y1 - y0) / 2.0).abs().max(0.5));
    let steps = ((rx + ry) * 2.0).ceil().max(16.0) as usize;
    let mut prev: Option<(f64, f64)> = None;
    for i in 0..=steps {
        let t = i as f64 / steps as f64 * std::f64::consts::TAU;
        let p = (cx + rx * t.cos(), cy + ry * t.sin());
        if let Some((px, py)) = prev {
            draw_stroke(img, px, py, p.0, p.1, half, c);
        }
        prev = Some(p);
    }
}

/// 箭头：主线 + 末端两条后掠的箭翼（翼长随线宽缩放，有下限保证细线也看得清）。
fn draw_arrow(
    img: &mut image::RgbaImage,
    x0: f64,
    y0: f64,
    x1: f64,
    y1: f64,
    half: f64,
    c: [u8; 3],
) {
    draw_stroke(img, x0, y0, x1, y1, half, c);
    let ang = (y1 - y0).atan2(x1 - x0);
    let hl = (half * 6.0).max(10.0);
    for da in [2.55, -2.55] {
        // ≈146° 后掠角
        let a = ang + da;
        draw_stroke(img, x1, y1, x1 + hl * a.cos(), y1 + hl * a.sin(), half, c);
    }
}

/// 把显示空间的标记烘焙进图片：坐标按裁剪偏移与改尺寸缩放换算到最终像素。
fn bake_marks(
    img: image::DynamicImage,
    marks: &[Mark],
    crop_x: f64,
    crop_y: f64,
    sx: f64,
    sy: f64,
) -> image::DynamicImage {
    let mut rgba = img.to_rgba8();
    for m in marks {
        let Some(c) = parse_color(&m.color) else {
            continue;
        };
        let half = (m.width * (sx + sy) / 2.0 / 2.0).max(0.5);
        let pts: Vec<(f64, f64)> = m
            .pts
            .iter()
            .map(|p| ((p.0 - crop_x) * sx, (p.1 - crop_y) * sy))
            .collect();
        match m.kind.as_str() {
            "pen" => {
                for w in pts.windows(2) {
                    draw_stroke(&mut rgba, w[0].0, w[0].1, w[1].0, w[1].1, half, c);
                }
            }
            "rect" if pts.len() >= 2 => {
                let (x0, y0, x1, y1) = (pts[0].0, pts[0].1, pts[1].0, pts[1].1);
                draw_stroke(&mut rgba, x0, y0, x1, y0, half, c);
                draw_stroke(&mut rgba, x1, y0, x1, y1, half, c);
                draw_stroke(&mut rgba, x1, y1, x0, y1, half, c);
                draw_stroke(&mut rgba, x0, y1, x0, y0, half, c);
            }
            "ellipse" if pts.len() >= 2 => {
                draw_ellipse(&mut rgba, pts[0].0, pts[0].1, pts[1].0, pts[1].1, half, c);
            }
            "arrow" if pts.len() >= 2 => {
                draw_arrow(&mut rgba, pts[0].0, pts[0].1, pts[1].0, pts[1].1, half, c);
            }
            _ => {}
        }
    }
    image::DynamicImage::ImageRgba8(rgba)
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
fn encode_image(
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
fn save_edits(path: String, edits: ImageEdits) -> Result<SaveOutcome, String> {
    let p = PathBuf::from(&path);
    let ext = p
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_lowercase();
    // HEIC 无编码器、SVG 是矢量、GIF 动图会丢帧：前端禁用按钮，这里兜底拒绝
    let format = match ext.as_str() {
        "jpg" | "jpeg" | "jpe" | "jfif" => "jpeg",
        "png" => "png",
        "webp" => "webp",
        "bmp" => "bmp",
        "tiff" | "tif" => "tiff",
        "avif" => "avif",
        "tga" => "tga",
        "qoi" => "qoi",
        "exr" => "exr",
        _ => return Err(format!(".{ext} 格式不支持直接修改原图")),
    };
    let img = decode_any(&path)?;
    let img = process_image(img, &p, &edits)?;
    let size = encode_image_to(img, &p, parse_format(format)?, edits.quality)?;
    Ok(SaveOutcome { dest: path, size })
}

/// 目标路径已存在时自动加 -2/-3… 后缀（p-2.jpg，不是 p.jpg-2），返回不冲突的路径。
/// 批量转换逐项串行调用：同批同名文件因前一项已落盘也能正确错开。
#[tauri::command]
fn unique_dest(dest: String) -> String {
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
fn save_image_as(src: String, dest: String, format: String) -> Result<(), String> {
    if format == "original" {
        return std::fs::copy(&src, &dest)
            .map(|_| ())
            .map_err(|e| format!("保存失败：{e}"));
    }
    encode_image(src, dest, format, None, None).map(|_| ())
}

/// 把旋转/镜像写回原图（原地覆盖）。save_edits 的简化包装（仅旋转/镜像）。
#[tauri::command]
fn apply_transform(path: String, rotation: u32, flip: bool) -> Result<(), String> {
    if rotation == 0 && !flip {
        return Ok(());
    }
    save_edits(
        path,
        ImageEdits {
            rotation,
            flip,
            crop: None,
            resize: None,
            quality: None,
            marks: Vec::new(),
        },
    )
    .map(|_| ())
}

/// 各扩展名当前默认应用状态（设置弹窗「格式关联」列表）。非 Windows 返回空列表。
#[tauri::command]
fn assoc_status() -> Vec<assoc::AssocStatus> {
    assoc::status()
}

/// 把所选扩展名的默认打开方式设为 SViewer（设置弹窗一键关联，只写 HKCU）。
#[tauri::command]
fn assoc_set(exts: Vec<String>) -> Result<(), String> {
    assoc::set(&exts)
}

/// 读取 EXIF Orientation（无则 None）。
fn exif_orientation(path: &Path) -> Option<image::metadata::Orientation> {
    let file = std::fs::File::open(path).ok()?;
    let mut reader = std::io::BufReader::new(&file);
    let exif = exif::Reader::new().read_from_container(&mut reader).ok()?;
    let field = exif.get_field(exif::Tag::Orientation, exif::In::PRIMARY)?;
    let v = field.value.get_uint(0)?;
    image::metadata::Orientation::from_exif(u8::try_from(v).ok()?)
}

/// 任意受支持格式 → DynamicImage（HEIC 走原生解码，其余走 image crate）。
fn decode_any(path: &str) -> Result<image::DynamicImage, String> {
    let p = PathBuf::from(path);
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

/// 日志插件：stdout + webview + 文件（系统日志目录），本地时区，10MB 轮转保留 3 份。
/// 日志位置（Windows）：%LOCALAPPDATA%/com.while.sviewer/logs/。
fn logging_plugin() -> tauri::plugin::TauriPlugin<tauri::Wry> {
    use tauri_plugin_log::{RotationStrategy, Target, TargetKind, TimezoneStrategy};

    let level = if cfg!(debug_assertions) {
        log::LevelFilter::Debug
    } else {
        log::LevelFilter::Info
    };

    tauri_plugin_log::Builder::new()
        .level(level)
        .targets([
            Target::new(TargetKind::Stdout),
            Target::new(TargetKind::Webview),
            Target::new(TargetKind::LogDir { file_name: None }),
        ])
        .timezone_strategy(TimezoneStrategy::UseLocal)
        .max_file_size(10_000_000)
        .rotation_strategy(RotationStrategy::KeepSome(3))
        .build()
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let builder = tauri::Builder::default();
    // single-instance 必须最先注册：第二次启动把图片路径转交给已有窗口。
    // 设置里开了「允许多开」则不注册，第二实例独立成窗。
    let builder = if multi_instance_enabled() {
        builder
    } else {
        builder.plugin(tauri_plugin_single_instance::init(|app, argv, _cwd| {
            if let Some(file) = pick_image_arg(&argv) {
                let _ = app.emit("open-file", file);
            }
            if let Some(w) = app.get_webview_window("main") {
                let _ = w.show();
                let _ = w.set_focus();
            }
        }))
    };
    let builder = builder
        .plugin(logging_plugin())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_process::init());
    // updater 插件仅在桌面端注册（移动端自动跳过）
    let builder = tauri_updater_kit::attach_updater(builder);

    builder
        .manage(LaunchFile::default())
        .setup(|app| {
            log::info!("SViewer v{} 启动", app.package_info().version);
            // 注册/刷新系统图片查看器关联（只写 HKCU）。每次启动都刷，exe 挪位置后路径自动跟上
            #[cfg(windows)]
            match assoc::register() {
                Ok(()) => log::info!("已注册系统图片查看器（HKCU）"),
                Err(e) => log::warn!("注册图片查看器失败：{e}"),
            }
            // 记录首次启动时命令行带入的图片，前端就绪后通过 get_launch_file 取走
            if let Some(file) = pick_image_arg(&std::env::args().collect::<Vec<_>>()) {
                if let Some(state) = app.try_state::<LaunchFile>() {
                    *state.0.lock().unwrap() = Some(file);
                }
            }
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            get_launch_file,
            list_dir_images,
            read_image_info,
            decode_to_png,
            decode_heic,
            decode_thumb,
            save_image_as,
            encode_image,
            save_edits,
            apply_transform,
            unique_dest,
            assoc_status,
            assoc_set,
            set_multi_instance,
        ])
        .run(tauri::generate_context!())
        .expect("error while running sviewer");
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
    fn apply_transform_rotates_in_place() {
        let path = std::env::temp_dir().join("sviewer_transform_test.jpg");
        image::RgbImage::from_pixel(40, 30, image::Rgb([255, 0, 0]))
            .save_with_format(&path, image::ImageFormat::Jpeg)
            .expect("生成测试图失败");
        super::apply_transform(path.to_string_lossy().into_owned(), 90, false)
            .expect("apply_transform 失败");
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

    /// 标记：矩形描边应染成指定颜色，框内保持原色。
    #[test]
    fn apply_edits_marks_rect() {
        let path = std::env::temp_dir().join("sviewer_marks_rect.png");
        image::RgbImage::from_pixel(40, 30, image::Rgb([255, 255, 255]))
            .save_with_format(&path, image::ImageFormat::Png)
            .expect("生成测试图失败");
        super::save_edits(
            path.to_string_lossy().into_owned(),
            super::ImageEdits {
                marks: vec![super::Mark {
                    kind: "rect".into(),
                    color: "#00ff00".into(),
                    width: 4.0,
                    pts: vec![(10.0, 10.0), (30.0, 20.0)],
                }],
                ..Default::default()
            },
        )
        .expect("save_edits 失败");
        let img = image::open(&path).unwrap().to_rgb8();
        let px = img.get_pixel(10, 15);
        assert_eq!((px[0], px[1], px[2]), (0, 255, 0), "矩形左边线应为绿色");
        let px = img.get_pixel(20, 15);
        assert_eq!((px[0], px[1], px[2]), (255, 255, 255), "框内不应着色");
        std::fs::remove_file(&path).ok();
    }

    /// 标记坐标随裁剪偏移换算：显示空间 (10,10) 裁掉 (5,5) 后应在成品 (5,5) 附近。
    #[test]
    fn apply_edits_marks_follow_crop() {
        let path = std::env::temp_dir().join("sviewer_marks_crop.png");
        image::RgbImage::from_pixel(40, 30, image::Rgb([255, 255, 255]))
            .save_with_format(&path, image::ImageFormat::Png)
            .expect("生成测试图失败");
        super::save_edits(
            path.to_string_lossy().into_owned(),
            super::ImageEdits {
                crop: Some(super::CropRect { x: 5, y: 5, w: 20, h: 15 }),
                marks: vec![super::Mark {
                    kind: "pen".into(),
                    color: "#ff0000".into(),
                    width: 2.0,
                    pts: vec![(10.0, 10.0), (15.0, 10.0)],
                }],
                ..Default::default()
            },
        )
        .expect("save_edits 失败");
        let (w, h) = image::image_dimensions(&path).unwrap();
        assert_eq!((w, h), (20, 15), "裁剪后应为 20×15");
        let img = image::open(&path).unwrap().to_rgb8();
        let px = img.get_pixel(5, 5);
        assert_eq!((px[0], px[1], px[2]), (255, 0, 0), "笔画应随裁剪平移到 (5,5)");
        let px = img.get_pixel(5, 9);
        assert_eq!((px[0], px[1], px[2]), (255, 255, 255), "笔画外不应着色");
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
