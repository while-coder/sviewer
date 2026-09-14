//! 日志插件：stdout + webview + 文件（系统日志目录），本地时区，10MB 轮转保留 3 份。
//!
//! 日志位置（Windows）：%LOCALAPPDATA%/com.while.sviewer/logs/。

use tauri::plugin::TauriPlugin;

/// 构建 tauri_plugin_log 插件。Debug 构建输出 Debug 级，发布构建 Info 级。
pub fn plugin() -> TauriPlugin<tauri::Wry> {
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
