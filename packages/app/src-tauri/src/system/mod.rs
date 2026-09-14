//! 系统集成域：应用与操作系统的交接（无对应前端 feature）。
//!
//! - [`launch`]：启动文件交接（双击关联 / 命令行 / Apple Event）、单实例/多开、
//!   受支持格式判定；
//! - [`assoc`]：Windows 图片查看器注册与格式关联（只写 HKCU）。

pub mod assoc;
pub mod launch;
