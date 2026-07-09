//! 输入格式解析器。
//!
//! 每个子模块负责一种文件格式, 最终都把自己的数据结构转成 [`crate::Circuit`]:
//! - [`pcb`]: KiCad `.kicad_pcb` 文件 (S-expression, 单文件包含全部信息)
//!
//! [`sexp`] 是 S-expression 解析器。

pub mod pcb;
pub mod sexp;
