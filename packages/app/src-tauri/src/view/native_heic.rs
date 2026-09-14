//! 平台原生 HEIC/HEIF 解码：Windows 走 WIC，macOS 走 Image I/O。
//!
//! 输出统一为：8 字节头（宽 u32 LE + 高 u32 LE）+ RGBA8 像素数据，
//! 前端在子线程包成 ImageBitmap 供 canvas 直显（见 heic-worker.ts）。
//!
//! 失败（Linux、系统没装 HEIF/HEVC 解码扩展等）返回 Err，前端自动回退
//! libheif WASM 解码，功能无损。
//!
//! 关于方向：HEIF 容器级变换（irot/imir）由各平台解码器按格式规范处理，
//! 与 libheif WASM 的行为一致；这里不再叠加 EXIF Orientation 标签，避免双重旋转。

/// 头部长度：宽 u32 + 高 u32（小端）
pub const HEADER_LEN: usize = 8;

/// 解码为「头 + RGBA8」。仅用于 HEIC/HEIF（前端 heic/heif 分支才会调用）。
pub fn decode(path: &str) -> Result<Vec<u8>, String> {
    imp::decode(path)
}

#[cfg(any(target_os = "windows", target_os = "macos"))]
fn pack_rgba(width: u32, height: u32, mut pixels: Vec<u8>) -> Result<Vec<u8>, String> {
    let expected = HEADER_LEN + width as usize * height as usize * 4;
    if width == 0 || height == 0 || pixels.len() < expected - HEADER_LEN {
        return Err("解码结果尺寸异常".into());
    }
    let mut out = Vec::with_capacity(expected);
    out.extend_from_slice(&width.to_le_bytes());
    out.extend_from_slice(&height.to_le_bytes());
    out.append(&mut pixels);
    Ok(out)
}

// ── Windows：WIC ──────────────────────────────────────────

#[cfg(target_os = "windows")]
mod imp {
    use windows::core::HSTRING;
    use windows::Win32::Foundation::GENERIC_READ;
    use windows::Win32::Graphics::Imaging::{
        CLSID_WICImagingFactory, GUID_WICPixelFormat32bppRGBA, IWICBitmapSource, IWICImagingFactory,
        WICBitmapDitherTypeNone, WICBitmapPaletteTypeCustom, WICDecodeMetadataCacheOnDemand,
    };
    use windows::Win32::System::Com::{
        CoCreateInstance, CoInitializeEx, CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED,
    };

    pub fn decode(path: &str) -> Result<Vec<u8>, String> {
        unsafe {
            // command 线程池不保证 COM 已初始化；失败（如已是 STA）也继续，仅探测
            let _ = CoInitializeEx(None, COINIT_MULTITHREADED);

            let factory: IWICImagingFactory =
                CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER)
                    .map_err(|e| format!("WIC 初始化失败：{e}"))?;

            // 系统没装「HEIF 图像扩展」时这一步报 0x88982F07（找不到组件）
            let decoder = factory
                .CreateDecoderFromFilename(
                    &HSTRING::from(path),
                    None,
                    GENERIC_READ,
                    WICDecodeMetadataCacheOnDemand,
                )
                .map_err(|e| format!("WIC 无法创建 HEIF 解码器（可能缺少系统 HEIF/HEVC 扩展）：{e}"))?;
            let frame: IWICBitmapSource = decoder
                .GetFrame(0)
                .map_err(|e| format!("读取 HEIF 首帧失败：{e}"))?
                .into();

            let (mut width, mut height) = (0u32, 0u32);
            frame
                .GetSize(&mut width, &mut height)
                .map_err(|e| format!("读取尺寸失败：{e}"))?;
            if width == 0 || height == 0 {
                return Err("HEIF 尺寸为 0".into());
            }

            // 统一转成 32bppRGBA（前端 ImageData 是 RGBA 序）。多数 HEIF 帧原生是
            // BGRA，由 WIC 的格式转换器一次性转好，省掉 Rust 侧逐像素交换。
            let source: IWICBitmapSource =
                if frame.GetPixelFormat().map_err(|e| e.to_string())? == GUID_WICPixelFormat32bppRGBA {
                    frame
                } else {
                    let conv = factory
                        .CreateFormatConverter()
                        .map_err(|e| format!("创建格式转换器失败：{e}"))?;
                    conv.Initialize(
                        &frame,
                        &GUID_WICPixelFormat32bppRGBA,
                        WICBitmapDitherTypeNone,
                        None,
                        0.0,
                        WICBitmapPaletteTypeCustom,
                    )
                    .map_err(|e| format!("像素格式转换失败：{e}"))?;
                    conv.into()
                };

            let stride = width as usize * 4;
            let mut pixels = vec![0u8; stride * height as usize];
            source
                .CopyPixels(std::ptr::null(), stride as u32, &mut pixels)
                .map_err(|e| format!("拷贝像素失败：{e}"))?;

            super::pack_rgba(width, height, pixels)
        }
    }
}

// ── macOS：Image I/O（Core Foundation / Core Graphics 纯 C API，无需额外 crate）──

#[cfg(target_os = "macos")]
mod imp {
    use std::ffi::{c_char, c_void, CString};

    type CFAllocatorRef = *const c_void;
    type CFStringRef = *const c_void;
    type CFURLRef = *const c_void;
    type CGColorSpaceRef = *const c_void;
    type CGImageRef = *const c_void;
    type CGImageSourceRef = *const c_void;
    type CGContextRef = *mut c_void;

    #[repr(C)]
    #[derive(Clone, Copy)]
    struct CGRect {
        x: f64,
        y: f64,
        width: f64,
        height: f64,
    }

    const K_CF_UTF8: u32 = 0x0800_0100;
    const K_CF_URL_POSIX_PATH_STYLE: i64 = 0;
    // CGImageAlphaInfo：kCGImageAlphaLast（非预乘 RGBA）
    const K_ALPHA_LAST: u32 = 3;
    // CGBitmapInfo 高位字节序字段：kCGBitmapByteOrder32Big = 4 << 12
    const K_BYTE_ORDER_32_BIG: u32 = 4 << 12;

    #[link(name = "CoreFoundation", kind = "framework")]
    extern "C" {
        fn CFStringCreateWithCString(
            alloc: CFAllocatorRef,
            c_str: *const c_char,
            encoding: u32,
        ) -> CFStringRef;
        fn CFURLCreateWithFileSystemPath(
            alloc: CFAllocatorRef,
            path: CFStringRef,
            style: i64,
            is_directory: bool,
        ) -> CFURLRef;
        fn CFRelease(cf: *const c_void);
    }

    #[link(name = "CoreGraphics", kind = "framework")]
    extern "C" {
        fn CGColorSpaceCreateDeviceRGB() -> CGColorSpaceRef;
        fn CGColorSpaceRelease(cs: CGColorSpaceRef);
        fn CGBitmapContextCreate(
            data: *mut c_void,
            width: usize,
            height: usize,
            bits_per_component: usize,
            bytes_per_row: usize,
            space: CGColorSpaceRef,
            bitmap_info: u32,
        ) -> CGContextRef;
        fn CGContextDrawImage(ctx: CGContextRef, rect: CGRect, img: CGImageRef);
        fn CGContextTranslateCTM(ctx: CGContextRef, x: f64, y: f64);
        fn CGContextScaleCTM(ctx: CGContextRef, x: f64, y: f64);
        fn CGBitmapContextGetData(ctx: CGContextRef) -> *mut c_void;
        fn CGContextRelease(ctx: CGContextRef);
        fn CGImageRelease(img: CGImageRef);
    }

    #[link(name = "ImageIO", kind = "framework")]
    extern "C" {
        fn CGImageSourceCreateWithURL(url: CFURLRef, options: *const c_void) -> CGImageSourceRef;
        fn CGImageSourceCreateImageAtIndex(
            src: CGImageSourceRef,
            index: usize,
            options: *const c_void,
        ) -> CGImageRef;
        fn CGImageGetWidth(img: CGImageRef) -> usize;
        fn CGImageGetHeight(img: CGImageRef) -> usize;
    }

    pub fn decode(path: &str) -> Result<Vec<u8>, String> {
        unsafe {
            let c_path =
                CString::new(path).map_err(|_| "路径包含非法字符".to_string())?;
            let cf_path = CFStringCreateWithCString(
                std::ptr::null(),
                c_path.as_ptr(),
                K_CF_UTF8,
            );
            let url = CFURLCreateWithFileSystemPath(
                std::ptr::null(),
                cf_path,
                K_CF_URL_POSIX_PATH_STYLE,
                false,
            );
            CFRelease(cf_path);

            let source = CGImageSourceCreateWithURL(url, std::ptr::null());
            CFRelease(url);
            if source.is_null() {
                return Err("Image I/O 无法打开该 HEIC 文件".into());
            }

            let img = CGImageSourceCreateImageAtIndex(source, 0, std::ptr::null());
            CFRelease(source);
            if img.is_null() {
                return Err("Image I/O 无法解码该 HEIC（可能缺 HEVC 支持）".into());
            }

            let width = CGImageGetWidth(img);
            let height = CGImageGetHeight(img);
            if width == 0 || height == 0 {
                CGImageRelease(img);
                return Err("HEIC 尺寸为 0".into());
            }

            // 画进 RGBA 位图上下文。CG 坐标系原点在左下，先翻转 CTM
            // 让输出保持与常规自上而下位图一致。
            let cs = CGColorSpaceCreateDeviceRGB();
            let ctx = CGBitmapContextCreate(
                std::ptr::null_mut(),
                width,
                height,
                8,
                width * 4,
                cs,
                K_ALPHA_LAST | K_BYTE_ORDER_32_BIG,
            );
            CGColorSpaceRelease(cs);
            if ctx.is_null() {
                CGImageRelease(img);
                return Err("创建位图上下文失败".into());
            }
            CGContextTranslateCTM(ctx, 0.0, height as f64);
            CGContextScaleCTM(ctx, 1.0, -1.0);
            CGContextDrawImage(
                ctx,
                CGRect {
                    x: 0.0,
                    y: 0.0,
                    width: width as f64,
                    height: height as f64,
                },
                img,
            );
            CGImageRelease(img);

            let len = width * height * 4;
            let pixels = std::slice::from_raw_parts(CGBitmapContextGetData(ctx) as *const u8, len).to_vec();
            CGContextRelease(ctx);
            super::pack_rgba(width as u32, height as u32, pixels)
        }
    }
}

// ── 其它平台：没有系统级解码器，直接报错让前端回退 WASM ──

#[cfg(not(any(target_os = "windows", target_os = "macos")))]
mod imp {
    pub fn decode(_path: &str) -> Result<Vec<u8>, String> {
        Err("此平台没有系统级 HEIC 解码器".into())
    }
}

#[cfg(test)]
mod tests {
    /// 手动冒烟测试：`HEIC_TEST_FILE=<heic 路径> cargo test --lib -- --nocapture`。
    /// 未设置环境变量时静默跳过。
    #[test]
    fn decode_sample() {
        let Ok(path) = std::env::var("HEIC_TEST_FILE") else {
            return;
        };
        let t0 = std::time::Instant::now();
        let data = super::decode(&path).expect("原生解码失败");
        let ms = t0.elapsed().as_millis();
        let w = u32::from_le_bytes([data[0], data[1], data[2], data[3]]);
        let h = u32::from_le_bytes([data[4], data[5], data[6], data[7]]);
        assert_eq!(
            data.len(),
            super::HEADER_LEN + w as usize * h as usize * 4,
            "像素数据长度与宽高不符"
        );
        println!("原生解码 {w}×{h} 用时 {ms} ms");
        // SVIEWER_DUMP=<png 路径> 时导出解码结果，人工核对方向 / 颜色
        if let Ok(dump) = std::env::var("SVIEWER_DUMP") {
            let px = data[super::HEADER_LEN..].to_vec();
            image::RgbaImage::from_raw(w, h, px)
                .expect("构造图像失败")
                .save(&dump)
                .expect("导出 PNG 失败");
            println!("已导出 {dump}");
        }
    }
}
