//! S-expression 解析, 基于 [`lexpr`] 库。
//!
//! [`lexpr`] 把 KiCad 文件解析成自己的 [`lexpr::Value`] 树 (cons cell 风格)。
//! 我们再把它展平成一个轻量的 [`Sexp`] 树 (只有 Atom / List 两个变体),
//! 给 [`super::footprint`] 和 [`super::netlist`] 用 — 它们都按"字符串形式的 atom"
//! 处理, 数字在需要时自己 `s.parse()`。
//!
//! KiCad 文件不会出现 lexpr 里那些花哨语法 (keyword / dotted list / vector / char / ...),
//! 遇到就当作错误报出来, 方便早期发现文件格式假设被打破。

#[derive(Debug, Clone, PartialEq)]
pub enum Sexp {
    Atom(String),
    List(Vec<Sexp>),
}

#[derive(Debug, Clone, PartialEq)]
pub struct ParseError {
    pub message: String,
}

pub fn parse(text: &str) -> Result<Sexp, ParseError> {
    let value = lexpr::from_str(text).map_err(|e| ParseError {
        message: format!("lexpr parse error: {e}"),
    })?;
    value_to_sexp(&value)
}

/// 把一个 [`lexpr::Value`] 递归转成 [`Sexp`]。
///
/// 关键: lexpr 把列表表示成一串 cons cell (`(1 2 3)` = `(1 . (2 . (3 . ())))`),
/// 这里的 [`cons_to_vec`] 先把它拍平成 `Vec<Value>`, 再逐个递归。
fn value_to_sexp(value: &lexpr::Value) -> Result<Sexp, ParseError> {
    match value {
        lexpr::Value::Null => Ok(Sexp::List(Vec::new())),
        lexpr::Value::Cons(_) => {
            let mut items: Vec<lexpr::Value> = Vec::new();
            cons_to_vec(value, &mut items)?;
            let children = items
                .iter()
                .map(value_to_sexp)
                .collect::<Result<Vec<_>, _>>()?;
            Ok(Sexp::List(children))
        }
        lexpr::Value::Symbol(s) => Ok(Sexp::Atom(s.to_string())),
        lexpr::Value::String(s) => Ok(Sexp::Atom(s.to_string())),
        lexpr::Value::Number(n) => Ok(Sexp::Atom(n.to_string())),
        other => Err(ParseError {
            message: format!("unsupported s-expression value: {other}"),
        }),
    }
}

/// 沿 cons cell 把列表元素收集到 `out`。遇到非 `Cons`/`Null` (即 dotted list)
/// 或根本不是列表的东西就报错 — KiCad 文件不应出现。
fn cons_to_vec(value: &lexpr::Value, out: &mut Vec<lexpr::Value>) -> Result<(), ParseError> {
    match value {
        lexpr::Value::Null => Ok(()),
        lexpr::Value::Cons(cons) => {
            out.push(cons.car().clone());
            cons_to_vec(cons.cdr(), out)
        }
        _ => Err(ParseError {
            message: format!("expected proper list, got: {value}"),
        }),
    }
}
