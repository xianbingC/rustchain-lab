use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Opcode {
    /// 压入常量。
    LoadConst(i64),
    /// 从状态读取变量值并压栈。
    Load(String),
    /// 弹栈并写入状态变量。
    Store(String),
    /// 算术加法：弹出两个操作数，压入结果。
    Add,
    /// 算术减法：弹出两个操作数，压入结果。
    Sub,
    /// 算术乘法：弹出两个操作数，压入结果。
    Mul,
    /// 算术除法：弹出两个操作数，压入结果。
    Div,
    /// 比较相等：结果为 1（真）或 0（假）。
    Eq,
    /// 比较大于：结果为 1（真）或 0（假）。
    Gt,
    /// 无条件跳转。
    Jump(usize),
    /// 条件跳转：栈顶为 0 则跳转。
    JumpIfFalse(usize),
    /// 记录事件。
    Emit(String),
    /// 主动停止执行。
    Halt,
}
