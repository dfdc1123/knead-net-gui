//! 输入格式解析器。
//!
//! 每个子模块负责一种文件格式, 最终都把自己的数据结构转成 [`crate::Circuit`]。
//! 目前:
//! - [`json`]: 自己设计的简单 JSON 格式
//!
//! 以后可能加:
//! - `netlist`: KiCad 导出的简化 netlist (S-expression)

pub mod json;
