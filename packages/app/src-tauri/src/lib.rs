//! SViewer —— 独立图片查看器后端。
//!
//! 职责与模块划分（按功能域归组，与前端 features/ 思路一致）：
//! - [`system`]：系统集成域——启动文件交接（双击关联 / 命令行）、单实例/多开、
//!   Windows 格式关联、日志（Tauri 命令定义在各自模块内）；
//! - [`view`]：查看域（对应前端 features/view，目录列表 / 解码 / EXIF 三个窗口共用）
//!   ——HEIC 平台原生解码、PSD 合成图解码（自实现）、逆地理编码对应前端 view/lib/geo.ts；
//! - [`edit`]：编辑落盘域（编辑窗口与批量转换共用）——编辑管线与编码落盘、标记光栅化；
//! - `formats_gen.rs`：由 scripts/gen-formats.cjs 生成，勿手改。
//!
//! 本文件只做应用装配：插件注册、单实例、Tauri 命令注册。

mod edit;
mod formats_gen;
mod system;
mod view;

use tauri::{Emitter, Manager};

use system::launch::LaunchFile;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let builder = tauri::Builder::default();
    // single-instance 必须最先注册：第二次启动把图片路径转交给已有窗口。
    // 设置里开了「允许多开」则不注册，第二实例独立成窗。
    let builder = if system::launch::multi_instance_enabled() {
        builder
    } else {
        builder.plugin(tauri_plugin_single_instance::init(|app, argv, _cwd| {
            if let Some(file) = system::launch::pick_image_arg(&argv) {
                let _ = app.emit("open-file", file);
            }
            if let Some(w) = app.get_webview_window("main") {
                let _ = w.show();
                let _ = w.set_focus();
            }
        }))
    };
    let builder = builder
        .plugin(system::logging::plugin())
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
            match system::assoc::register() {
                Ok(()) => log::info!("已注册系统图片查看器（HKCU）"),
                Err(e) => log::warn!("注册图片查看器失败：{e}"),
            }
            // 记录首次启动时命令行带入的图片，前端就绪后通过 get_launch_file 取走
            if let Some(file) = system::launch::pick_image_arg(&std::env::args().collect::<Vec<_>>()) {
                if let Some(state) = app.try_state::<LaunchFile>() {
                    *state.0.lock().unwrap() = Some(file);
                }
            }
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            system::launch::get_launch_file,
            system::launch::set_multi_instance,
            view::info::list_dir_images,
            view::info::read_image_info,
            view::info::delete_image,
            view::decode::decode_to_png,
            view::decode::decode_heic,
            view::decode::decode_raw,
            view::decode::decode_thumb,
            edit::pipeline::save_image_as,
            edit::pipeline::encode_image,
            edit::pipeline::save_edits,
            edit::pipeline::unique_dest,
            system::assoc::assoc_status,
            system::assoc::assoc_set,
            view::geo::reverse_geocode,
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
                if !path.is_file() || !system::launch::is_supported(&path) {
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
