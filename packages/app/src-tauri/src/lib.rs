//! SViewer —— 独立图片查看器后端。
//!
//! 职责与模块划分（按功能域归组，与前端 features/ 思路一致）：
//! - [`launch`]：启动文件交接（双击关联 / 命令行）、单实例/多开、受支持格式判定；
//! - [`image`]：图像读取域（查看 / 编辑 / 转换共用）——目录列表与 EXIF、
//!   非原生格式解码、平台原生 HEIC 解码；
//! - [`edit`]：编辑落盘域（编辑窗口与批量转换共用）——编辑管线与编码落盘、标记光栅化；
//! - [`assoc`] / [`geo`]：格式关联、逆地理编码（Tauri 命令定义在各自模块内）；
//! - `formats_gen.rs`：由 scripts/gen-formats.cjs 生成，勿手改。

mod assoc;
mod edit;
mod formats_gen;
mod geo;
mod image;
mod launch;

use tauri::{Emitter, Manager};

use launch::LaunchFile;

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
    let builder = if launch::multi_instance_enabled() {
        builder
    } else {
        builder.plugin(tauri_plugin_single_instance::init(|app, argv, _cwd| {
            if let Some(file) = launch::pick_image_arg(&argv) {
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

    let app = builder
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
            if let Some(file) = launch::pick_image_arg(&std::env::args().collect::<Vec<_>>()) {
                if let Some(state) = app.try_state::<LaunchFile>() {
                    *state.0.lock().unwrap() = Some(file);
                }
            }
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            launch::get_launch_file,
            launch::set_multi_instance,
            image::info::list_dir_images,
            image::info::read_image_info,
            image::decode::decode_to_png,
            image::decode::decode_heic,
            image::decode::decode_raw,
            image::decode::decode_thumb,
            edit::pipeline::save_image_as,
            edit::pipeline::encode_image,
            edit::pipeline::save_edits,
            edit::pipeline::unique_dest,
            assoc::assoc_status,
            assoc::assoc_set,
            geo::reverse_geocode,
        ])
        .build(tauri::generate_context!())
        .expect("error while running sviewer");

    // macOS 不走 argv 传文件：Finder 双击 / 右键打开经 Apple Event 投递，
    // Tauri 转成 RunEvent::Opened。存入 LaunchFile（前端 onMounted 取走）并 emit
    // open-file（已运行实例再打开时前端监听直接收），与 single-instance 转交路径一致。
    #[cfg(any(target_os = "macos", target_os = "ios", target_os = "android"))]
    app.run(|app, event| {
        if let tauri::RunEvent::Opened { urls } = &event {
            for url in urls {
                let Ok(path) = url.to_file_path() else { continue };
                if !path.is_file() || !launch::is_supported(&path) {
                    continue;
                }
                let file = path.to_string_lossy().into_owned();
                log::info!("系统打开文件：{file}");
                if let Some(state) = app.try_state::<LaunchFile>() {
                    *state.0.lock().unwrap() = Some(file.clone());
                }
                let _ = app.emit("open-file", &file);
                break;
            }
        }
    });
    #[cfg(not(any(target_os = "macos", target_os = "ios", target_os = "android")))]
    app.run(|_app, _event| {});
}
