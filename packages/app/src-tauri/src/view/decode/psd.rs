//! PSD 解码：Photoshop 文件的合成图预览（自实现，纯 Rust，无平台分叉）。
//!
//! 只解合成图、不解析图层（预览够用；PS 保存时默认勾选「最大兼容」，合成图都在）。
//! 支持 RLE（PackBits）与未压缩两种存储、8/16-bit、灰度 / 灰度+透明 / RGB / RGBA；
//! CMYK、索引色、PSB（大文件格式）不支持，明确报错，由前端展示失败提示。
//!
//! 不用 zune-psd crate：其未压缩 8-bit 路径有 bug（平面数据按交错索引散布、
//! 索引上限用 pixel_count 而非 pixel_count*channel_count，2/3 像素落空为黑），
//! 且 RLE + 16-bit 未实现；合成图部分结构简单，自实现更可控。

use image::DynamicImage;

/// PSD 文件头长度（签名 4 + 版本 2 + 保留 6 + 通道 2 + 高 4 + 宽 4 + 深度 2 + 模式 2）。
const HEADER_LEN: usize = 26;
/// PSD 魔数。
const MAGIC: &[u8; 4] = b"8BPS";
/// 色彩模式：PSD 头部模式字段。位图(0)/双色调(8)/索引(2)/CMYK(4) 等不支持。
const MODE_GRAYSCALE: u16 = 1;
const MODE_RGB: u16 = 3;

/// 文件头解析结果（版本恒为 1，PSB 在解析时即拒绝）。
#[derive(Debug)]
struct Header {
    channels: u16,
    width: u32,
    height: u32,
    depth: u16,
}

/// 解码 PSD 合成图为 DynamicImage。供 [`super::decode_any`] 的 psd 分支调用。
pub fn decode(path: &str) -> Result<DynamicImage, String> {
    let data = std::fs::read(path).map_err(|e| format!("打开失败：{e}"))?;
    decode_bytes(&data)
}

/// 从文件头直接取尺寸（只读 26 字节，不解整图）。
/// image crate 不认 PSD，这是 `read_image_info` 尺寸兜底链的一环。
pub fn dimensions(path: &str) -> Result<(u32, u32), String> {
    use std::io::Read;
    let mut head = [0u8; HEADER_LEN];
    let mut file = std::fs::File::open(path).map_err(|e| format!("打开失败：{e}"))?;
    file.read_exact(&mut head).map_err(|e| format!("PSD 文件过短：{e}"))?;
    let h = parse_header(&head)?;
    Ok((h.width, h.height))
}

/// 解析 26 字节文件头：魔数 `8BPS`、版本 1（2 是 PSB，明确拒绝）、
/// 通道数、u32 BE 高（偏移 14）/ 宽（偏移 18）、位深。
fn parse_header(head: &[u8; HEADER_LEN]) -> Result<Header, String> {
    let be16 = |b: &[u8]| u16::from_be_bytes([b[0], b[1]]);
    let be32 = |b: &[u8]| u32::from_be_bytes([b[0], b[1], b[2], b[3]]);
    if &head[0..4] != MAGIC {
        return Err("不是有效的 PSD 文件（魔数不符）".into());
    }
    if be16(&head[4..6]) != 1 {
        return Err("暂不支持 PSB（大文件格式）".into());
    }
    let header = Header {
        channels: be16(&head[12..14]),
        height: be32(&head[14..18]),
        width: be32(&head[18..22]),
        depth: be16(&head[22..24]),
    };
    if header.width == 0 || header.height == 0 {
        return Err("PSD 尺寸为 0".into());
    }
    if header.channels == 0 || header.channels > 4 {
        return Err(format!("PSD 通道数异常：{}", header.channels));
    }
    if header.depth != 8 && header.depth != 16 {
        return Err(format!("PSD 位深不支持：{}bit", header.depth));
    }
    Ok(header)
}

fn decode_bytes(data: &[u8]) -> Result<DynamicImage, String> {
    if data.len() < HEADER_LEN {
        return Err("PSD 文件过短".into());
    }
    let mut head = [0u8; HEADER_LEN];
    head.copy_from_slice(&data[..HEADER_LEN]);
    let header = parse_header(&head)?;
    let mode = u16::from_be_bytes([data[24], data[25]]);
    if mode != MODE_RGB && mode != MODE_GRAYSCALE {
        return Err("暂不支持该 PSD 色彩模式（CMYK / 索引色等仅 Photoshop 可读）".into());
    }

    let mut c = HEADER_LEN;
    // 依次跳过三段定长块：色彩模式数据、图像资源、图层与蒙版（各 u32 BE 长度）
    for _ in 0..3 {
        let len = u32_len(data, c)?;
        c += 4 + len;
    }

    // 图像数据段：压缩方式 u16（0=未压缩，1=RLE），其后为逐通道平面的像素
    let compression = be16(data, c)?;
    c += 2;
    let (w, h) = (header.width as usize, header.height as usize);
    let bytes_per = header.depth as usize / 8;
    let plane_len = w * h * bytes_per;
    let planes: Vec<Vec<u8>> = match compression {
        0 => {
            if data.len() < c + plane_len * header.channels as usize {
                return Err("PSD 像素数据不完整".into());
            }
            (0..header.channels as usize)
                .map(|ch| data[c + ch * plane_len..c + (ch + 1) * plane_len].to_vec())
                .collect()
        }
        1 => {
            // 每扫描线一个 u16 BE 压缩长度（通道 0 全部行 → 通道 1 …），
            // 表后依次跟各通道各行的 PackBits 流
            let rows = h * header.channels as usize;
            if data.len() < c + rows * 2 {
                return Err("PSD RLE 长度表不完整".into());
            }
            let counts: Vec<usize> = (0..rows)
                .map(|r| be16(data, c + r * 2).map(|v| v as usize))
                .collect::<Result<_, _>>()?;
            c += rows * 2;
            let row_len = plane_len / h;
            let mut planes = vec![vec![0u8; plane_len]; header.channels as usize];
            for (r, count) in counts.iter().enumerate() {
                let src = data
                    .get(c..c + count)
                    .ok_or_else(|| "PSD RLE 数据不完整".to_string())?;
                c += count;
                let (ch, row) = (r / h, r % h);
                let at = row * row_len;
                planes[ch][at..at + row_len].copy_from_slice(&unpack_rle(src, row_len)?);
            }
            planes
        }
        2..=3 => return Err("PSD 使用 ZIP 压缩，暂不支持".into()),
        _ => return Err(format!("PSD 压缩方式未知：{compression}")),
    };
    if planes.len() != header.channels as usize
        || planes.iter().any(|p| p.len() != plane_len)
    {
        return Err("PSD 像素数据不完整".into());
    }
    build_image(w as u32, h as u32, header.depth, planes)
}

/// 读取偏移处的 u32 BE 段长度。
fn u32_len(data: &[u8], at: usize) -> Result<usize, String> {
    let b = data
        .get(at..at + 4)
        .ok_or_else(|| "PSD 文件结构不完整".to_string())?;
    Ok(u32::from_be_bytes([b[0], b[1], b[2], b[3]]) as usize)
}

fn be16(data: &[u8], at: usize) -> Result<u16, String> {
    let b = data
        .get(at..at + 2)
        .ok_or_else(|| "PSD 文件结构不完整".to_string())?;
    Ok(u16::from_be_bytes([b[0], b[1]]))
}

/// PackBits 解压（PSD RLE）：控制字节 ≥0 为「后随 n+1 字节字面量」，
/// -1..=-127 为「下一字节重复 1-n 次」（2~128，注意不是 -n，每个游程多 1），
/// -128 为空操作。
fn unpack_rle(src: &[u8], expected: usize) -> Result<Vec<u8>, String> {
    let mut out = Vec::with_capacity(expected);
    let mut i = 0;
    while out.len() < expected {
        let n = i8::from_be_bytes([*src.get(i).ok_or_else(|| "PSD RLE 数据截断".to_string())?]);
        i += 1;
        match n {
            0..=127 => {
                let len = n as usize + 1;
                out.extend_from_slice(
                    src.get(i..i + len)
                        .ok_or_else(|| "PSD RLE 数据截断".to_string())?,
                );
                i += len;
            }
            -127..=-1 => {
                let b = *src.get(i).ok_or_else(|| "PSD RLE 数据截断".to_string())?;
                i += 1;
                out.extend(std::iter::repeat(b).take((1 - n as i16) as usize));
            }
            _ => {}
        }
    }
    if out.len() != expected {
        return Err("PSD RLE 数据异常（解压超长）".into());
    }
    Ok(out)
}

/// 平面像素 → 交错像素（8-bit：每通道一个 u8 平面）。
fn interleave(planes: &[Vec<u8>]) -> Vec<u8> {
    let px = planes[0].len();
    let mut out = Vec::with_capacity(px * planes.len());
    for i in 0..px {
        for p in planes {
            out.push(p[i]);
        }
    }
    out
}

/// 平面像素 → 交错像素（16-bit：每通道一个 u8 平面，按 u16 BE 解样本后交错）。
fn interleave_u16(planes: &[Vec<u8>]) -> Vec<u16> {
    let planes: Vec<Vec<u16>> = planes.iter().map(|p| to_u16(p)).collect();
    let px = planes[0].len();
    let mut out = Vec::with_capacity(px * planes.len());
    for i in 0..px {
        for p in &planes {
            out.push(p[i]);
        }
    }
    out
}

/// 16-bit 平面按 u16 BE 解样本。
fn to_u16(plane: &[u8]) -> Vec<u16> {
    plane
        .chunks_exact(2)
        .map(|c| u16::from_be_bytes([c[0], c[1]]))
        .collect()
}

/// 按通道数 + 位深把平面像素拼成 DynamicImage。
/// ImageBuffer::from_raw 会校验缓冲长度，不符（如遇到未预期的通道组合）报错。
fn build_image(w: u32, h: u32, depth: u16, planes: Vec<Vec<u8>>) -> Result<DynamicImage, String> {
    let img = match (depth, planes.len()) {
        (8, 1) => image::GrayImage::from_raw(w, h, planes.into_iter().next().unwrap())
            .map(DynamicImage::ImageLuma8),
        (8, 2) => image::GrayAlphaImage::from_raw(w, h, interleave(&planes))
            .map(DynamicImage::ImageLumaA8),
        (8, 3) => image::RgbImage::from_raw(w, h, interleave(&planes))
            .map(DynamicImage::ImageRgb8),
        (8, 4) => image::RgbaImage::from_raw(w, h, interleave(&planes))
            .map(DynamicImage::ImageRgba8),
        (16, 1) => image::ImageBuffer::from_raw(w, h, to_u16(&planes[0]))
            .map(DynamicImage::ImageLuma16),
        (16, 2) => image::ImageBuffer::from_raw(w, h, interleave_u16(&planes))
            .map(DynamicImage::ImageLumaA16),
        (16, 3) => image::ImageBuffer::from_raw(w, h, interleave_u16(&planes))
            .map(DynamicImage::ImageRgb16),
        (16, 4) => image::ImageBuffer::from_raw(w, h, interleave_u16(&planes))
            .map(DynamicImage::ImageRgba16),
        _ => None,
    };
    img.ok_or_else(|| "PSD 像素数据长度不符".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 构造一个最小 PSD 文件头（签名 + 版本 + 通道 3 + 高/宽 u32 BE + 8-bit）。
    fn header(version: u16, height: u32, width: u32) -> [u8; HEADER_LEN] {
        let mut head = [0u8; HEADER_LEN];
        head[0..4].copy_from_slice(MAGIC);
        head[4..6].copy_from_slice(&version.to_be_bytes());
        head[12..14].copy_from_slice(&3u16.to_be_bytes());
        head[14..18].copy_from_slice(&height.to_be_bytes());
        head[18..22].copy_from_slice(&width.to_be_bytes());
        head[22..24].copy_from_slice(&8u16.to_be_bytes());
        head
    }

    #[test]
    fn 头部解析_取宽高() {
        let head = header(1, 1080, 1920);
        let h = parse_header(&head).unwrap();
        assert_eq!((h.width, h.height), (1920, 1080));
    }

    #[test]
    fn 头部解析_魔数不符() {
        let mut head = header(1, 10, 10);
        head[0] = b'X';
        assert!(parse_header(&head).is_err());
    }

    #[test]
    fn 头部解析_PSB_明确报错() {
        let head = header(2, 10, 10);
        let err = parse_header(&head).unwrap_err();
        assert!(err.contains("PSB"));
    }

    #[test]
    fn packbits_字面量与重复() {
        // 字面量 3 字节 + 重复 'A'×4（控制字节 253 = -3 → 1-n=4）+ 空操作(-128)
        let src = [2, 1, 2, 3, 253u8, b'A', 128u8];
        assert_eq!(unpack_rle(&src, 7).unwrap(), vec![1, 2, 3, b'A', b'A', b'A', b'A']);
    }

    #[test]
    fn packbits_重复上限128() {
        // 控制字节 129（-127）→ 重复 1-n=128 次，这是规范上限
        let src = [129u8, 7];
        assert_eq!(unpack_rle(&src, 128).unwrap(), vec![7u8; 128]);
    }

    #[test]
    fn packbits_截断报错() {
        assert!(unpack_rle(&[5, 1, 2], 6).is_err());
    }

    /// 手工构造 2×2 RGB8 PSD（头 + 三段空长度 + 图像数据段），端到端验证。
    /// PSD 像素按通道平面存储：R 全部 → G → B。
    fn synth_psd(compression: u16, pixel_data: Vec<u8>) -> Vec<u8> {
        let mut psd = Vec::new();
        psd.extend_from_slice(MAGIC);
        psd.extend_from_slice(&1u16.to_be_bytes()); // 版本 1（PSD）
        psd.extend_from_slice(&[0u8; 6]); // 保留
        psd.extend_from_slice(&3u16.to_be_bytes()); // 通道数 RGB
        psd.extend_from_slice(&2u32.to_be_bytes()); // 高
        psd.extend_from_slice(&2u32.to_be_bytes()); // 宽
        psd.extend_from_slice(&8u16.to_be_bytes()); // 8-bit
        psd.extend_from_slice(&3u16.to_be_bytes()); // 色彩模式 RGB
        psd.extend_from_slice(&0u32.to_be_bytes()); // 色彩模式数据长度
        psd.extend_from_slice(&0u32.to_be_bytes()); // 图像资源长度
        psd.extend_from_slice(&0u32.to_be_bytes()); // 图层与蒙版长度
        psd.extend_from_slice(&compression.to_be_bytes());
        psd.extend_from_slice(&pixel_data);
        psd
    }

    fn decode_synth(name: &str, psd: &[u8]) -> DynamicImage {
        let dir = std::env::temp_dir().join("sviewer-psd-test");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(name);
        std::fs::write(&path, psd).unwrap();
        decode(path.to_str().unwrap()).unwrap()
    }

    #[test]
    fn 解码_未压缩_rgb8() {
        let psd = synth_psd(
            0,
            [[255u8, 128, 0, 64], [16, 32, 48, 64], [200, 100, 50, 0]].concat(),
        );
        let img = decode_synth("synth-raw.psd", &psd);
        assert_eq!((img.width(), img.height()), (2, 2));
        let rgb = img.to_rgb8();
        assert_eq!(rgb.get_pixel(0, 0), &image::Rgb([255u8, 16, 200]));
        assert_eq!(rgb.get_pixel(1, 1), &image::Rgb([64u8, 64, 0]));
    }

    #[test]
    fn 解码_rle_rgb8() {
        // 2 像素/行的字面量 PackBits 流：控制字节 1 = 后随 2 字节。
        // 数据按通道平面组织：R 两行 → G 两行 → B 两行，长度表通道 0 全部行在前。
        let rle_row = |a: u8, b: u8| vec![1u8, a, b];
        let mut data = Vec::new();
        for _ in 0..6 {
            data.extend_from_slice(&3u16.to_be_bytes()); // 长度表：3 通道 × 2 行，每行 3 字节
        }
        data.extend(rle_row(255, 128)); // R 行 0
        data.extend(rle_row(0, 64)); // R 行 1
        data.extend(rle_row(16, 32)); // G 行 0
        data.extend(rle_row(48, 64)); // G 行 1
        data.extend(rle_row(200, 100)); // B 行 0
        data.extend(rle_row(50, 0)); // B 行 1
        let img = decode_synth("synth-rle.psd", &synth_psd(1, data));
        assert_eq!((img.width(), img.height()), (2, 2));
        let rgb = img.to_rgb8();
        assert_eq!(rgb.get_pixel(0, 0), &image::Rgb([255u8, 16, 200]));
        assert_eq!(rgb.get_pixel(1, 1), &image::Rgb([64u8, 64, 0]));
    }
}
