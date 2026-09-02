//! Windows 图片查看器注册与格式关联。
//!
//! 只写 HKCU（免管理员），覆盖三处：
//! 1. ProgID `SViewer.Image`：`shell\open\command` 指向当前 exe，双击/「打开方式」用；
//! 2. `Applications\sviewer.exe`：让 SViewer 出现在资源管理器「打开方式」候选里；
//! 3. `RegisteredApplications` + Capabilities：让 SViewer 出现在「设置 → 默认应用」。
//!
//! 设置弹窗的「一键关联」把选中扩展名的默认打开方式指到 SViewer：
//! 写 `Classes\<ext>` 默认值 + OpenWithProgids；若该扩展名已有别的 UserChoice
//! （哈希保护的「默认应用」记录），删除之让 Classes 默认值生效（HKCU 内无需管理员）。
//! 每次启动都重写一遍候选注册，exe 挪位置（dev ↔ 安装版）后路径自动跟上。

/// 受支持的扩展名（带点、小写），与 lib.rs 的 SUPPORTED_EXT 保持一致。
const EXTS: &[&str] = &[
    ".jpg", ".jpeg", ".jpe", ".jfif", ".png", ".gif", ".webp", ".bmp", ".ico", ".svg", ".tiff",
    ".tif", ".avif", ".heic", ".heif", ".hif", ".tga", ".pbm", ".pgm", ".ppm", ".pnm", ".dds",
    ".hdr", ".exr", ".qoi",
];

/// ProgID：给系统看的关联标识名。
const PROG_ID: &str = "SViewer.Image";

/// 单个扩展名的关联状态（assoc_status 命令返回，设置弹窗格式关联列表用）。
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AssocStatus {
    /// 扩展名（小写、不含点）
    pub ext: String,
    /// 当前默认应用显示名（识别不出的 ProgID 原样展示）
    pub app: String,
    /// 默认应用是否是 SViewer
    pub is_sviewer: bool,
}

#[cfg(windows)]
mod imp {
    use std::io::Error as IoError;
    use std::path::PathBuf;

    use winreg::enums::{HKEY_CLASSES_ROOT, HKEY_CURRENT_USER};
    use winreg::RegKey;

    use super::{AssocStatus, EXTS, PROG_ID};

    fn reg_err(e: IoError) -> String {
        format!("写注册表失败：{e}")
    }

    /// 当前 exe 完整路径。
    fn exe_path() -> Result<PathBuf, String> {
        std::env::current_exe().map_err(|e| format!("无法定位当前 exe：{e}"))
    }

    /// 注册/刷新图片查看器关联。
    pub fn register() -> Result<(), String> {
        let exe = exe_path()?;
        let cmd = format!("\"{}\" \"%1\"", exe.display());
        let hkcu = RegKey::predef(HKEY_CURRENT_USER);

        // 1) ProgID：文件类型 → 打开命令
        let (prog, _) = hkcu
            .create_subkey(format!("Software\\Classes\\{PROG_ID}"))
            .map_err(reg_err)?;
        prog.set_value("", &"SViewer 图片").map_err(reg_err)?;
        prog.set_value("FriendlyTypeName", &"SViewer 图片")
            .map_err(reg_err)?;
        let (icon, _) = prog
            .create_subkey("DefaultIcon")
            .map_err(reg_err)?;
        icon.set_value("", &format!("{},0", exe.display()))
            .map_err(reg_err)?;
        let (open, _) = prog
            .create_subkey("shell\\open\\command")
            .map_err(reg_err)?;
        open.set_value("", &cmd).map_err(reg_err)?;

        // 2) 「打开方式」候选列表：SupportedTypes 声明 SViewer 认识哪些扩展名
        let (app, _) = hkcu
            .create_subkey("Software\\Classes\\Applications\\sviewer.exe")
            .map_err(reg_err)?;
        let (appopen, _) = app
            .create_subkey("shell\\open\\command")
            .map_err(reg_err)?;
        appopen.set_value("", &cmd).map_err(reg_err)?;
        let (types, _) = app.create_subkey("SupportedTypes").map_err(reg_err)?;
        for ext in EXTS {
            types.set_value(ext, &"").map_err(reg_err)?;
        }

        // 3) 「设置 → 默认应用」候选：Capabilities + RegisteredApplications
        let (cap, _) = hkcu
            .create_subkey("Software\\SViewer\\Capabilities")
            .map_err(reg_err)?;
        cap.set_value("ApplicationName", &"SViewer").map_err(reg_err)?;
        cap.set_value("ApplicationDescription", &"轻量级本地图片查看器")
            .map_err(reg_err)?;
        let (fa, _) = cap.create_subkey("FileAssociations").map_err(reg_err)?;
        for ext in EXTS {
            fa.set_value(ext, &PROG_ID).map_err(reg_err)?;
        }
        let (ra, _) = hkcu
            .create_subkey("Software\\RegisteredApplications")
            .map_err(reg_err)?;
        ra.set_value("SViewer", &"Software\\SViewer\\Capabilities")
            .map_err(reg_err)?;

        Ok(())
    }

    /// 展开间接字符串 "@C:\path\shell32.dll,-102" → 资源里的本地化文本（失败返回 None）。
    fn expand_indirect(s: &str) -> Option<String> {
        use windows::core::PCWSTR;
        use windows::Win32::UI::Shell::SHLoadIndirectString;

        let src: Vec<u16> = s.encode_utf16().chain(std::iter::once(0)).collect();
        let mut buf = [0u16; 512];
        // SAFETY：src 以 NUL 结尾，buf 为 512 长度的有效缓冲
        let hr = unsafe { SHLoadIndirectString(PCWSTR::from_raw(src.as_ptr()), &mut buf, None) };
        if hr.is_err() {
            return None;
        }
        let len = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
        let out = String::from_utf16_lossy(&buf[..len]);
        let out = out.trim();
        if out.is_empty() {
            None
        } else {
            Some(out.to_string())
        }
    }

    /// 描述性默认值（类型说明而非应用名），不直接显示。
    fn looks_like_type_desc(v: &str) -> bool {
        const MARKS: &[&str] = &["文件", "文档", "Document", "document", "File", "file"];
        MARKS.iter().any(|m| v.contains(m))
    }

    /// ProgID → 应用显示名。依次取：
    /// 1. FriendlyTypeName（最准，间接字符串 @dll,-id 先展开成「Windows 照片查看器」这类名字）；
    /// 2. 默认值（排除「图像 (jpg) 文件」这类类型描述）；
    /// 3. 打开命令里的 exe 名（BandiView / msedge 等）；
    /// 4. 兜底 ProgID 原文。
    fn progid_app(progid: &str) -> String {
        if progid == PROG_ID {
            return "SViewer".into();
        }
        let root = RegKey::predef(HKEY_CLASSES_ROOT);
        if let Ok(k) = root.open_subkey(progid) {
            if let Ok(v) = k.get_value::<String, _>("FriendlyTypeName") {
                if !v.is_empty() {
                    let name = if v.starts_with('@') { expand_indirect(&v) } else { Some(v) };
                    if let Some(name) = name {
                        return name;
                    }
                }
            }
            if let Ok(v) = k.get_value::<String, _>("") {
                if !v.is_empty() && !v.starts_with('@') && !looks_like_type_desc(&v) {
                    return v;
                }
            }
            // UWP 等应用 FriendlyTypeName 全是间接字符串：从打开命令里抠 exe 名
            let cmd = k
                .open_subkey("shell\\open\\command")
                .ok()
                .and_then(|c| c.get_value::<String, _>("").ok());
            if let Some(app) = cmd.as_deref().and_then(exe_name_from_command) {
                return app;
            }
        }
        progid.into()
    }

    /// 从 shell\open\command 命令行里提取 exe 显示名（去引号、去参数、去 .exe 后缀）。
    fn exe_name_from_command(cmd: &str) -> Option<String> {
        let first = if let Some(rest) = cmd.strip_prefix('"') {
            rest.split('"').next()?
        } else {
            cmd.split_whitespace().next()?
        };
        let name = PathBuf::from(first)
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| first.to_string());
        if name.is_empty() {
            None
        } else {
            Some(name)
        }
    }

    /// 该字符串是否为「真的能打开文件」的 ProgID：存在且带 shell\open\command。
    /// 防止把 HKCR\<ext> 默认值里的描述文本（如「图像 (jpg) 文件」）当成应用名。
    fn is_valid_progid(progid: &str) -> bool {
        RegKey::predef(HKEY_CLASSES_ROOT)
            .open_subkey(progid)
            .map(|k| k.open_subkey("shell\\open\\command").is_ok())
            .unwrap_or(false)
    }

    /// 查询各扩展名的当前默认应用。
    pub fn status() -> Vec<AssocStatus> {
        EXTS
            .iter()
            .map(|ext| {
                let hkcu = RegKey::predef(HKEY_CURRENT_USER);
                let hkcr = RegKey::predef(HKEY_CLASSES_ROOT);
                // 依次尝试：用户选择（设置 → 默认应用，最优先）→
                // HKCR\<ext> 默认值（须是带打开命令的 ProgID）→ OpenWithProgids 候选
                let user_choice = hkcu
                    .open_subkey(format!(
                        "Software\\Microsoft\\Windows\\CurrentVersion\\Explorer\\FileExts\\{ext}\\UserChoice"
                    ))
                    .and_then(|k| k.get_value::<String, _>("ProgId"))
                    .ok();
                let classes_default = hkcr
                    .open_subkey(ext)
                    .ok()
                    .and_then(|k| k.get_value::<String, _>("").ok())
                    .filter(|p| !p.is_empty());
                let progid = match user_choice {
                    Some(p) => Some(p),
                    None => match classes_default.filter(|p| is_valid_progid(p)) {
                        Some(p) => Some(p),
                        None => hkcr
                            .open_subkey(ext)
                            .ok()
                            .and_then(|k| k.open_subkey("OpenWithProgids").ok())
                            .map(|owp| {
                                owp.enum_keys().flatten().find(|pid| is_valid_progid(pid))
                            })
                            .flatten(),
                    },
                };
                match progid {
                    Some(p) => AssocStatus {
                        ext: ext.trim_start_matches('.').into(),
                        app: progid_app(&p),
                        is_sviewer: p == PROG_ID,
                    },
                    None => AssocStatus {
                        ext: ext.trim_start_matches('.').into(),
                        app: "（未关联）".into(),
                        is_sviewer: false,
                    },
                }
            })
            .collect()
    }

    /// 把所选扩展名的默认打开方式设为 SViewer。
    pub fn set(exts: &[String]) -> Result<(), String> {
        // 先刷新候选注册，保证 ProgID 的打开命令指向当前 exe
        register()?;
        let hkcu = RegKey::predef(HKEY_CURRENT_USER);
        let mut errs: Vec<String> = Vec::new();
        for ext in exts {
            let ext = ext.trim_start_matches('.').to_lowercase();
            let Ok((key, _)) = hkcu.create_subkey(format!("Software\\Classes\\.{ext}")) else {
                errs.push(format!(".{ext}: 创建注册表键失败"));
                continue;
            };
            if let Err(e) = key.set_value("", &PROG_ID) {
                errs.push(format!(".{ext}: {}", reg_err(e)));
                continue;
            }
            // OpenWithProgids：即使默认值被系统改写，也保证「打开方式」里能选回 SViewer
            if let Ok((owp, _)) = key.create_subkey("OpenWithProgids") {
                let _ = owp.set_value(PROG_ID, &"");
            }
            // 别的程序占着 UserChoice 时 Classes 默认值不生效：删掉它（HKCU 内，免管理员）
            match hkcu.open_subkey_with_flags(
                format!(
                    "Software\\Microsoft\\Windows\\CurrentVersion\\Explorer\\FileExts\\.{ext}"
                ),
                winreg::enums::KEY_READ | winreg::enums::KEY_WRITE,
            ) {
                Ok(fe) => {
                    if let Ok(uc) = fe.open_subkey("UserChoice") {
                        let ours: Option<String> = uc.get_value("ProgId").ok();
                        if ours.as_deref() != Some(PROG_ID) {
                            let _ = fe.delete_subkey("UserChoice");
                        }
                    }
                }
                Err(_) => {} // 没有 FileExts 记录（从未打开过该类型），无需处理
            }
        }
        if errs.is_empty() {
            Ok(())
        } else {
            Err(errs.join("；"))
        }
    }
}

/// 非 Windows 平台无事可做。
#[cfg(not(windows))]
mod stub {
    use super::AssocStatus;

    pub fn register() -> Result<(), String> {
        Ok(())
    }
    pub fn status() -> Vec<AssocStatus> {
        Vec::new()
    }
    pub fn set(_exts: &[String]) -> Result<(), String> {
        Ok(())
    }
}

#[cfg(windows)]
pub use imp::{register, set, status};
#[cfg(not(windows))]
pub use stub::{register, set, status};
