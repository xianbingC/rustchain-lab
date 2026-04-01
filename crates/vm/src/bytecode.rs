use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Opcode {
    LoadConst(i64),
    Store(String),
    Jump(usize),
    JumpIfFalse(usize),
    Emit(String),
    Halt,
}
