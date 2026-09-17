//! Windows 图片查看器注册与格式关联。
//!
//! 只写 HKCU（免管理员），覆盖三处：
//! 1. ProgID `SViewer.Image`：`shell\open\command` 指向当前 exe，双击/「打开方式」用；
//! 2. `Applications\sviewer.exe`：让 SViewer 出现在资源管理器「打开方式」候选里；
//! 3. `RegisteredApplications` + Capabilities：让 SViewer 出现在「设置 → 默认应用」。
//!
//! 设置弹窗的「一键关联」把选中扩展名的默认打开方式指到 SViewer：
//! 写 `Classes\<ext>` 默认值 + OpenWithProgids，并直接接管 `FileExts\<ext>\UserChoice`。
//! UserChoice 是 Win8+ 带哈希保护的「默认应用」记录，优先级高于 Classes 默认值；
//! 删掉它 Windows 会立刻重置回系统默认（如 UWP 照片），所以必须算出合法哈希一并写入
//! （Chrome / SetUserFTA 同款方案，见 [`user_choice`]）。官方没有提供第三方设置默认应用的 API。
//! 每次启动都重写一遍候选注册，exe 挪位置（dev ↔ 安装版）后路径自动跟上。

/// 受支持的扩展名（带点、小写），从 formats_gen::SUPPORTED_EXT 派生（单一事实来源 formats.json）。
fn exts() -> impl Iterator<Item = String> {
    crate::formats_gen::SUPPORTED_EXT.iter().map(|e| format!(".{e}"))
}

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

/// UserChoice 哈希（仅 Windows）。
///
/// Win8 起 `FileExts\<ext>\UserChoice` 的 `ProgId` 受 `Hash` 值保护，Windows 验证
/// 失败或缺席时会把默认应用重置回系统选择。算法未公开，本模块是 PS-SFTA
/// （github.com/DanysysTeam/PS-SFTA，源自 kolbi.cz 的逆向）的 Rust 移植：
/// 以「扩展名 + 用户 SID + ProgID + 分钟级 FILETIME + 固定盐」为输入，
/// MD5 后经两轮混淆得到 8 字节哈希，Base64 编码。验证时 Windows 用 UserChoice
/// 键的最后写入时间（截断到分钟）重算，所以写入必须紧跟着哈希计算、同一分钟内完成。
#[cfg(windows)]
mod user_choice {
    use base64::Engine as _;
    use md5::{Digest, Md5};

    /// 固定盐（Windows 自身硬编码在 shell32 里的字符串）。
    const EXPERIENCE: &str =
        "User Choice set via Windows User Experience {D18B6DD5-6124-4341-9318-804003BAFA0B}";

    /// 当前时间取整到分钟的 FILETIME，16 位小写十六进制（PS-SFTA 的 Get-HexDateTime）。
    fn minute_filetime_hex() -> String {
        let secs = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        // FILETIME 从 1601-01-01 起以 100ns 计，与 UNIX 纪元差 11644473600 秒
        let ft = (secs + 11_644_473_600) * 10_000_000;
        let ft = ft - ft % 600_000_000; // 截断到分钟（60s × 10⁷）
        format!("{ft:016x}")
    }

    /// 当前用户 SID（小写字符串）。哈希绑定了 SID，换账号后哈希不同。
    pub fn user_sid() -> Result<String, String> {
        use windows::core::PWSTR;
        use windows::Win32::Foundation::{CloseHandle, LocalFree, HLOCAL, HANDLE};
        use windows::Win32::Security::{GetTokenInformation, TokenUser, TOKEN_QUERY, TOKEN_USER};
        use windows::Win32::Security::Authorization::ConvertSidToStringSidW;
        use windows::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

        unsafe {
            let mut token = HANDLE::default();
            OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token)
                .map_err(|e| format!("打开进程令牌失败：{e}"))?;
            let result = (|| {
                let mut len = 0u32;
                // 第一次调用探出所需缓冲区大小
                if GetTokenInformation(token, TokenUser, None, 0, &mut len).is_err() && len == 0 {
                    return Err("查询令牌用户信息失败".into());
                }
                let mut buf = vec![0u8; len as usize];
                GetTokenInformation(token, TokenUser, Some(buf.as_mut_ptr().cast()), len, &mut len)
                    .map_err(|e| format!("读取令牌用户信息失败：{e}"))?;
                let user = &*(buf.as_ptr() as *const TOKEN_USER);
                let mut pwstr = PWSTR::null();
                ConvertSidToStringSidW(user.User.Sid, &mut pwstr)
                    .map_err(|e| format!("转换 SID 失败：{e}"))?;
                let sid = pwstr.to_string().map_err(|e| format!("SID 转字符串失败：{e}"))?;
                let _ = LocalFree(Some(HLOCAL(pwstr.as_ptr().cast())));
                Ok(sid)
            })();
            let _ = CloseHandle(token);
            result.map(|s| s.to_lowercase())
        }
    }

    /// PS-SFTA `Get-Hash` 的逐行移植。PS 脚本里 Int32/Int64 混算在 mod 2³² 意义下
    /// 等价于 u32 回绕运算；其 Get-ShiftRight 的异或补丁实为 32 位逻辑右移。
    /// 供测试对照 PS-SFTA 原版输出（pub(super)：tests 模块引用）。
    pub(super) fn base_info_hash(base_info: &str) -> String {
        let mut base: Vec<u8> =
            base_info.encode_utf16().flat_map(u16::to_le_bytes).collect();
        base.extend_from_slice(&[0, 0]); // PS：bytesBaseInfo += 0x00, 0x00

        let digest = Md5::digest(&base);
        let word = |b: &[u8], i: usize| u32::from_le_bytes([b[i], b[i + 1], b[i + 2], b[i + 3]]);

        // PS：length = ((lengthBase -band 4) -le 1) + (lengthBase >>> 2) - 1
        let len_base = base_info.encode_utf16().count() * 2 + 2;
        let length = ((len_base & 4 == 0) as i64) + ((len_base >> 2) as i64) - 1;
        if length <= 1 {
            return String::new(); // 输入过短，实际不会发生
        }
        let index = ((length - 2) >> 1) as usize;

        let mut out = [0u8; 16];
        let (mut outhash1, mut outhash2, mut cache) = (0u32, 0u32, 0u32);

        // 第一轮
        let md51 = (word(&digest, 0) | 1).wrapping_add(0x69FB_0000);
        let md52 = (word(&digest, 4) | 1).wrapping_add(0x13DB_0000);
        let mut pdata = 0usize;
        for _ in 0..=index {
            let r0 = word(&base, pdata).wrapping_add(outhash1);
            let r1 = word(&base, pdata + 4);
            pdata += 8;
            let r2a = r0.wrapping_mul(md51).wrapping_sub(0x10FA_9605u32.wrapping_mul(r0 >> 16));
            let r2b = 0x79F8_A395u32.wrapping_mul(r2a).wrapping_add(0x689B_6B9Fu32.wrapping_mul(r2a >> 16));
            let r3 = 0xEA97_0001u32.wrapping_mul(r2b).wrapping_sub(0x3C10_1569u32.wrapping_mul(r2b >> 16));
            let r4 = r3.wrapping_add(r1);
            let r5 = cache.wrapping_add(r3);
            let r6a = r4.wrapping_mul(md52).wrapping_sub(0x3CE8_EC25u32.wrapping_mul(r4 >> 16));
            let r6b = 0x59C3_AF2Du32.wrapping_mul(r6a).wrapping_sub(0x2232_E0F1u32.wrapping_mul(r6a >> 16));
            outhash1 = 0x1EC9_0001u32.wrapping_mul(r6b).wrapping_add(0x35BD_1EC9u32.wrapping_mul(r6b >> 16));
            outhash2 = r5.wrapping_add(outhash1);
            cache = outhash2;
        }
        out[0..4].copy_from_slice(&outhash1.to_le_bytes());
        out[4..8].copy_from_slice(&outhash2.to_le_bytes());

        // 第二轮
        let md51 = word(&digest, 0) | 1;
        let md52 = word(&digest, 4) | 1;
        cache = 0;
        outhash1 = 0;
        pdata = 0;
        for _ in 0..=index {
            let r0 = word(&base, pdata).wrapping_add(outhash1);
            pdata += 8;
            let r1a = r0.wrapping_mul(md51);
            let r1b = 0xB111_0000u32.wrapping_mul(r1a).wrapping_sub(0x3067_4EEFu32.wrapping_mul(r1a >> 16));
            let r2a = 0x5B9F_0000u32.wrapping_mul(r1b).wrapping_sub(0x78F7_A461u32.wrapping_mul(r1b >> 16));
            let r2b = 0x12CE_B96Du32.wrapping_mul(r2a >> 16).wrapping_sub(0x4693_0000u32.wrapping_mul(r2a));
            let r3 = 0x1D83_0000u32.wrapping_mul(r2b).wrapping_add(0x257E_1D83u32.wrapping_mul(r2b >> 16));
            let r4 = md52.wrapping_mul(r3.wrapping_add(word(&base, pdata - 4)));
            let r4b = 0x16F5_0000u32.wrapping_mul(r4).wrapping_sub(0x5D8B_E90Bu32.wrapping_mul(r4 >> 16));
            let r5a = 0x96FF_0000u32.wrapping_mul(r4b).wrapping_sub(0x2C7C_6901u32.wrapping_mul(r4b >> 16));
            let r5b = 0x2B89_0000u32.wrapping_mul(r5a).wrapping_add(0x7C93_2B89u32.wrapping_mul(r5a >> 16));
            outhash1 = 0x9F69_0000u32.wrapping_mul(r5b).wrapping_sub(0x405B_6097u32.wrapping_mul(r5b >> 16));
            outhash2 = outhash1.wrapping_add(cache).wrapping_add(r3);
            cache = outhash2;
        }
        out[8..12].copy_from_slice(&outhash1.to_le_bytes());
        out[12..16].copy_from_slice(&outhash2.to_le_bytes());

        let mut tail = [0u8; 8];
        tail[0..4].copy_from_slice(&(word(&out, 8) ^ word(&out, 0)).to_le_bytes());
        tail[4..8].copy_from_slice(&(word(&out, 12) ^ word(&out, 4)).to_le_bytes());
        base64::engine::general_purpose::STANDARD.encode(tail)
    }

    /// 计算某个扩展名的 UserChoice Hash。`ext` 形如 ".png"。
    pub fn hash(ext: &str, progid: &str, sid: &str) -> String {
        // PS-SFTA：baseInfo = ext + sid + progid + 分钟级 FILETIME + 盐，整体小写
        let base_info =
            format!("{ext}{sid}{progid}{}{EXPERIENCE}", minute_filetime_hex()).to_lowercase();
        base_info_hash(&base_info)
    }
}

#[cfg(test)]
mod tests {
    // 期望值由 PS-SFTA 原版 Get-Hash（PowerShell）对同一 baseInfo 计算得出
    #[test]
    fn hash_matches_ps_sfta() {
        let base_info = ".pngs-1-5-21-3678184149-2768516898-2932038129-1001sviewer.image0166d2a1b2c3d4e5fuser choice set via windows user experience {d18b6dd5-6124-4341-9318-804003bafa0b}";
        let expected = "lTMMLlC4KSg=";
        let actual = super::user_choice::base_info_hash(base_info);
        println!("actual: {actual}");
        assert_eq!(actual, expected);
    }
}

#[cfg(windows)]
mod imp {
    use std::io::Error as IoError;
    use std::path::PathBuf;

    use winreg::enums::{HKEY_CLASSES_ROOT, HKEY_CURRENT_USER};
    use winreg::RegKey;

    use super::{exts, AssocStatus, PROG_ID};

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
        prog.set_value("", &"速阅 图片").map_err(reg_err)?;
        prog.set_value("FriendlyTypeName", &"速阅 图片")
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
        for ext in exts() {
            types.set_value(ext, &"").map_err(reg_err)?;
        }

        // 3) 「设置 → 默认应用」候选：Capabilities + RegisteredApplications
        let (cap, _) = hkcu
            .create_subkey("Software\\SViewer\\Capabilities")
            .map_err(reg_err)?;
        cap.set_value("ApplicationName", &"速阅").map_err(reg_err)?;
        cap.set_value("ApplicationDescription", &"轻量级本地图片查看器")
            .map_err(reg_err)?;
        let (fa, _) = cap.create_subkey("FileAssociations").map_err(reg_err)?;
        for ext in exts() {
            fa.set_value(ext, &PROG_ID).map_err(reg_err)?;
        }
        let (ra, _) = hkcu
            .create_subkey("Software\\RegisteredApplications")
            .map_err(reg_err)?;
        ra.set_value("速阅", &"Software\\SViewer\\Capabilities")
            .map_err(reg_err)?;
        // 清掉旧版用英文名注册的条目，避免「设置 → 默认应用」里出现两个
        let _ = ra.delete_value("SViewer");

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
        exts()
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
                    .open_subkey(&ext)
                    .ok()
                    .and_then(|k| k.get_value::<String, _>("").ok())
                    .filter(|p| !p.is_empty());
                let progid = match user_choice {
                    Some(p) => Some(p),
                    None => match classes_default.filter(|p| is_valid_progid(p)) {
                        Some(p) => Some(p),
                        None => hkcr
                            .open_subkey(&ext)
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
        let sid = super::user_choice::user_sid()?;
        // 哈希输入里的分钟级时间戳必须与 UserChoice 键最后写入时间同分钟，
        // 所以这里取一次，整个循环里所有写入都在同一分钟内紧跟完成
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
            // 接管 UserChoice：删除会被 Windows 重置回系统默认，必须带合法哈希直接写入
            if let Err(e) = write_user_choice(&hkcu, &ext, &sid) {
                errs.push(format!(".{ext}: {e}"));
                continue;
            }
            // 旧式「打开方式 → 始终使用此应用」写入的 Application 值会抢占
            // Classes 默认值，清掉
            match hkcu.open_subkey_with_flags(
                format!(
                    "Software\\Microsoft\\Windows\\CurrentVersion\\Explorer\\FileExts\\.{ext}"
                ),
                winreg::enums::KEY_WRITE,
            ) {
                Ok(fe) => {
                    let _ = fe.delete_value("Application");
                }
                Err(_) => {} // 没有 FileExts 记录（从未打开过该类型），无需处理
            }
            // 抑制「有新应用可以打开 xxx 文件」的重置提示
            if let Ok((toasts, _)) = hkcu.create_subkey(
                "Software\\Microsoft\\Windows\\CurrentVersion\\ApplicationAssociationToasts",
            ) {
                let _ = toasts.set_value(format!("{PROG_ID}_.{ext}"), &0u32);
            }
        }
        if errs.is_empty() {
            // 通知 Explorer 关联已变更：立即重读注册表并刷新图标缓存，
            // 否则双击行为和文件图标要等重启资源管理器才生效
            unsafe {
                use windows::Win32::UI::Shell::{SHCNE_ASSOCCHANGED, SHCNF_IDLIST, SHChangeNotify};
                SHChangeNotify(SHCNE_ASSOCCHANGED, SHCNF_IDLIST, None, None);
            }
            Ok(())
        } else {
            Err(errs.join("；"))
        }
    }

    /// 用合法哈希接管 `FileExts\<ext>\UserChoice`（先删旧键再重建写入，
    /// 写入紧随哈希计算完成，保证键时间戳与哈希输入的分钟一致）。
    fn write_user_choice(hkcu: &RegKey, ext: &str, sid: &str) -> Result<(), String> {
        // 删除旧的 UserChoice（先 drop 句柄再删，RegDeleteKeyExW 删不掉还开着的键）
        match hkcu.open_subkey_with_flags(
            format!(
                "Software\\Microsoft\\Windows\\CurrentVersion\\Explorer\\FileExts\\.{ext}"
            ),
            winreg::enums::KEY_READ | winreg::enums::KEY_WRITE,
        ) {
            Ok(fe) => {
                let _ = fe.delete_subkey("UserChoice");
            }
            Err(_) => {} // 没有 FileExts 记录，直接新建即可
        }
        let (uc, _) = hkcu
            .create_subkey(format!(
                "Software\\Microsoft\\Windows\\CurrentVersion\\Explorer\\FileExts\\.{ext}\\UserChoice"
            ))
            .map_err(reg_err)?;
        uc.set_value("Hash", &super::user_choice::hash(&format!(".{ext}"), PROG_ID, sid))
            .map_err(reg_err)?;
        uc.set_value("ProgId", &PROG_ID).map_err(reg_err)
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

// ── Tauri 命令（设置弹窗「格式关联」）──

/// 各扩展名当前默认应用状态（设置弹窗「格式关联」列表）。非 Windows 返回空列表。
#[tauri::command]
pub fn assoc_status() -> Vec<AssocStatus> {
    status()
}

/// 把所选扩展名的默认打开方式设为 SViewer（设置弹窗一键关联，只写 HKCU）。
#[tauri::command]
pub fn assoc_set(exts: Vec<String>) -> Result<(), String> {
    set(&exts)
}
