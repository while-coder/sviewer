//! 编辑落盘域：编辑窗口与批量转换窗口共用。
//!
//! - [`pipeline`]：编辑管线与编码落盘（另存为 / 保存到原图 / 批量转换共用）；
//! - [`marks`]：标记绘制光栅化。

pub mod marks;
pub mod pipeline;
