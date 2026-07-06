//! 输入格式解析器。
//!
//! 每个子模块负责一种文件格式, 最终都把自己的数据结构转成 [`crate::Circuit`]:
//! - [`footprint`]: KiCad `.kicad_mod` 封装文件 (S-expression)
//! - [`netlist`]:  KiCad 简化 netlist `.net` (S-expression)
//!
//! [`sexp`] 是后面两个格式共用的 S-expression 解析器。

pub mod footprint;
pub mod netlist;
pub mod sexp;
