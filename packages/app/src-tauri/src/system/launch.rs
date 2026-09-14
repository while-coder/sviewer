//! 启动文件与单实例/多开控制。
//!
//! - 双击关联 / 命令行带入的图片路径先落到 [`LaunchFile`]，前端就绪后取走；
//! - 「允许多开」靠标记文件跨进程传递（localStorage 读不到，Rust 侧只认文件）；
//! - [`is_supported`] 是「扩展名是否受支持」的统一入口（清单见 formats_gen.rs）。

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use tauri::State;

use crate::formats_gen;

/// 启动时待打开的文件路径，前端 onMounted 取走一次后清空。
#[derive(Default)]
pub(crate) struct LaunchFile(pub(crate) Mutex<Option<String>>);

/// 「允许多开」标记文件：<配置目录>/com.while.sviewer/allow-multi-instance。
/// 路径与 Tauri app_config_dir 约定一致：
/// Windows %APPDATA% · macOS ~/Library/Application Support · Linux $XDG_CONFIG_HOME（缺省 ~/.config）。
/// 前端设置里开关时由 set_multi_instance 写/删；启动时按它决定是否注册
/// single-instance 插件（localStorage 读不到，Rust 侧只认文件）。
fn multi_instance_flag_path() -> Option<PathBuf> {
    let base = if cfg!(target_os = "windows") {
        PathBuf::from(std::env::var("APPDATA").ok()?)
    } else if cfg!(target_os = "macos") {
        PathBuf::from(std::env::var("HOME").ok()?).join("Library").join("Application Support")
    } else {
        match std::env::var("XDG_CONFIG_HOME") {
            Ok(v) if !v.is_empty() => PathBuf::from(v),
            _ => PathBuf::from(std::env::var("HOME").ok()?).join(".config"),
        }
    };
    Some(base.join("com.while.sviewer").join("allow-multi-instance"))
}

/// 当前是否允许多开。
pub(crate) fn multi_instance_enabled() -> bool {
    multi_instance_flag_path().is_some_and(|p| p.exists())
}

/// 设置「允许多开」：写/删标记文件，下次启动生效。
#[tauri::command]
pub(crate) fn set_multi_instance(enabled: bool) -> Result<(), String> {
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

/// 判断扩展名是否受支持。
pub(crate) fn is_supported(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| formats_gen::SUPPORTED_EXT.contains(&e.to_lowercase().as_str()))
        .unwrap_or(false)
}

/// 从命令行参数中挑出第一个存在且受支持的图片路径。
pub(crate) fn pick_image_arg(argv: &[String]) -> Option<String> {
    argv.iter()
        .skip(1) // 跳过程序自身路径
        .map(PathBuf::from)
        .find(|p| p.is_file() && is_supported(p))
        .map(|p| p.to_string_lossy().into_owned())
}

/// 取走启动文件（取后清空，保证只触发一次）。
#[tauri::command]
pub(crate) fn get_launch_file(state: State<'_, LaunchFile>) -> Option<String> {
    state.0.lock().ok().and_then(|mut g| g.take())
}
