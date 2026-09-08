//! 标记绘制光栅化：矩形 / 椭圆 / 箭头 / 画笔，圆头笔触无抗锯齿。
//!
//! 标记存「显示空间」坐标（前端 SVG 预览所见即所得），落盘时按裁剪偏移与
//! 改尺寸缩放换算到最终像素（见 [`bake_marks`]）。

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
pub(crate) fn bake_marks(
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

#[cfg(test)]
mod tests {
    use super::{bake_marks, Mark};
    use image::DynamicImage;

    /// 标记：矩形描边应染成指定颜色，框内保持原色。
    #[test]
    fn bake_marks_rect() {
        let img = DynamicImage::from(
            image::RgbaImage::from_pixel(40, 30, image::Rgba([255, 255, 255, 255])),
        );
        let out = bake_marks(
            img,
            &[Mark {
                kind: "rect".into(),
                color: "#00ff00".into(),
                width: 4.0,
                pts: vec![(10.0, 10.0), (30.0, 20.0)],
            }],
            0.0,
            0.0,
            1.0,
            1.0,
        );
        let img = out.to_rgb8();
        let px = img.get_pixel(10, 15);
        assert_eq!((px[0], px[1], px[2]), (0, 255, 0), "矩形左边线应为绿色");
        let px = img.get_pixel(20, 15);
        assert_eq!((px[0], px[1], px[2]), (255, 255, 255), "框内不应着色");
    }

    /// 标记坐标随裁剪偏移换算：显示空间 (10,10) 裁掉 (5,5) 后应在成品 (5,5) 附近。
    #[test]
    fn bake_marks_follow_crop() {
        let img = DynamicImage::from(
            image::RgbaImage::from_pixel(40, 30, image::Rgba([255, 255, 255, 255])),
        );
        // 与编辑管线一致：先裁剪，再按裁剪偏移换算标记坐标
        let cropped = img.crop_imm(5, 5, 20, 15);
        let out = bake_marks(
            cropped,
            &[Mark {
                kind: "pen".into(),
                color: "#ff0000".into(),
                width: 2.0,
                pts: vec![(10.0, 10.0), (15.0, 10.0)],
            }],
            5.0,
            5.0,
            1.0,
            1.0,
        );
        assert_eq!((out.width(), out.height()), (20, 15), "裁剪后应为 20×15");
        let img = out.to_rgb8();
        let px = img.get_pixel(5, 5);
        assert_eq!((px[0], px[1], px[2]), (255, 0, 0), "笔画应随裁剪平移到 (5,5)");
        let px = img.get_pixel(5, 9);
        assert_eq!((px[0], px[1], px[2]), (255, 255, 255), "笔画外不应着色");
    }
}
