// 不弹控制台窗口（dev/release 都不弹）。日志仍写入文件与 webview 控制台，
// 位置（Windows）：%LOCALAPPDATA%/com.while.sviewer/logs/
#![cfg_attr(windows, windows_subsystem = "windows")]

fn main() {
    sviewer_lib::run()
}
