use crate::bytecode::Opcode;
use thiserror::Error;

/// 合约文本编译错误。
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum CompileError {
    /// 指令名称未知。
    #[error("未知指令: line={line}, opcode={opcode}")]
    UnknownOpcode { line: usize, opcode: String },
    /// 缺少操作数。
    #[error("缺少操作数: line={line}, opcode={opcode}")]
    MissingOperand { line: usize, opcode: &'static str },
    /// 操作数格式错误。
    #[error("操作数格式错误: line={line}, opcode={opcode}, value={value}")]
    InvalidOperand {
        line: usize,
        opcode: &'static str,
        value: String,
    },
    /// 不接受多余操作数。
    #[error("存在多余操作数: line={line}, opcode={opcode}, value={value}")]
    UnexpectedOperand {
        line: usize,
        opcode: &'static str,
        value: String,
    },
}

/// 将简单合约文本编译为字节码指令序列。
pub fn compile(source: &str) -> Result<Vec<Opcode>, CompileError> {
    let mut program = Vec::new();

    for (index, raw_line) in source.lines().enumerate() {
        let line_no = index.saturating_add(1);
        let line = raw_line.split('#').next().unwrap_or_default().trim();
        if line.is_empty() {
            continue;
        }

        let mut split = line.splitn(2, char::is_whitespace);
        let opcode_raw = split.next().unwrap_or_default();
        let opcode = opcode_raw.to_ascii_uppercase();
        let operand = split.next().map(str::trim).unwrap_or_default();

        let instruction = match opcode.as_str() {
            "LOAD_CONST" => {
                let raw_value = required_operand(line_no, "LOAD_CONST", operand)?;
                ensure_no_space_operand(line_no, "LOAD_CONST", raw_value)?;

                let value = raw_value
                    .parse::<i64>()
                    .map_err(|_| CompileError::InvalidOperand {
                        line: line_no,
                        opcode: "LOAD_CONST",
                        value: raw_value.to_string(),
                    })?;
                Opcode::LoadConst(value)
            }
            "STORE" => {
                let key = required_operand(line_no, "STORE", operand)?;
                ensure_no_space_operand(line_no, "STORE", key)?;
                Opcode::Store(key.to_string())
            }
            "JUMP" => {
                let raw_target = required_operand(line_no, "JUMP", operand)?;
                ensure_no_space_operand(line_no, "JUMP", raw_target)?;

                let target =
                    raw_target
                        .parse::<usize>()
                        .map_err(|_| CompileError::InvalidOperand {
                            line: line_no,
                            opcode: "JUMP",
                            value: raw_target.to_string(),
                        })?;
                Opcode::Jump(target)
            }
            "JUMP_IF_FALSE" => {
                let raw_target = required_operand(line_no, "JUMP_IF_FALSE", operand)?;
                ensure_no_space_operand(line_no, "JUMP_IF_FALSE", raw_target)?;

                let target =
                    raw_target
                        .parse::<usize>()
                        .map_err(|_| CompileError::InvalidOperand {
                            line: line_no,
                            opcode: "JUMP_IF_FALSE",
                            value: raw_target.to_string(),
                        })?;
                Opcode::JumpIfFalse(target)
            }
            "EMIT" => {
                let event = required_operand(line_no, "EMIT", operand)?;
                Opcode::Emit(trim_wrapped_quotes(event).to_string())
            }
            "HALT" => {
                if !operand.is_empty() {
                    return Err(CompileError::UnexpectedOperand {
                        line: line_no,
                        opcode: "HALT",
                        value: operand.to_string(),
                    });
                }
                Opcode::Halt
            }
            _ => {
                return Err(CompileError::UnknownOpcode {
                    line: line_no,
                    opcode,
                });
            }
        };

        program.push(instruction);
    }

    Ok(program)
}

/// 检查操作数是否存在。
fn required_operand<'a>(
    line: usize,
    opcode: &'static str,
    operand: &'a str,
) -> Result<&'a str, CompileError> {
    if operand.is_empty() {
        return Err(CompileError::MissingOperand { line, opcode });
    }
    Ok(operand)
}

/// 限定单一 token 操作数（如整数、变量名）。
fn ensure_no_space_operand(
    line: usize,
    opcode: &'static str,
    operand: &str,
) -> Result<(), CompileError> {
    if operand.split_whitespace().count() > 1 {
        return Err(CompileError::UnexpectedOperand {
            line,
            opcode,
            value: operand.to_string(),
        });
    }
    Ok(())
}

/// 去掉包裹型双引号，便于编写可读脚本。
fn trim_wrapped_quotes(raw: &str) -> &str {
    if raw.starts_with('"') && raw.ends_with('"') && raw.len() >= 2 {
        return &raw[1..raw.len() - 1];
    }
    raw
}

#[cfg(test)]
mod tests {
    use super::{compile, CompileError};
    use crate::bytecode::Opcode;

    /// 验证基础文本程序可以被编译为字节码。
    #[test]
    fn simple_program_should_compile() {
        let source = r#"
            LOAD_CONST 7
            STORE counter
            EMIT counter_initialized
            HALT
        "#;

        let program = compile(source).expect("编译应当成功");
        assert_eq!(
            program,
            vec![
                Opcode::LoadConst(7),
                Opcode::Store("counter".to_string()),
                Opcode::Emit("counter_initialized".to_string()),
                Opcode::Halt
            ]
        );
    }

    /// 验证未知指令会返回错误。
    #[test]
    fn unknown_opcode_should_fail() {
        let source = "WARP 1";
        let result = compile(source);
        assert_eq!(
            result,
            Err(CompileError::UnknownOpcode {
                line: 1,
                opcode: "WARP".to_string(),
            })
        );
    }

    /// 验证缺少操作数会返回错误。
    #[test]
    fn missing_operand_should_fail() {
        let source = "LOAD_CONST";
        let result = compile(source);
        assert_eq!(
            result,
            Err(CompileError::MissingOperand {
                line: 1,
                opcode: "LOAD_CONST",
            })
        );
    }

    /// 验证注释和大小写指令也可以正确解析。
    #[test]
    fn comment_and_lowercase_should_work() {
        let source = r#"
            # 初始化状态
            load_const 1
            store flag
            emit "flag initialized"
            halt
        "#;

        let result = compile(source).expect("编译应当成功");
        assert_eq!(
            result,
            vec![
                Opcode::LoadConst(1),
                Opcode::Store("flag".to_string()),
                Opcode::Emit("flag initialized".to_string()),
                Opcode::Halt,
            ]
        );
    }
}
