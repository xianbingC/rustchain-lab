use crate::bytecode::Opcode;
use std::collections::HashMap;
use thiserror::Error;

/// VM 执行结果摘要。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExecutionReport {
    /// 是否由 Halt 指令主动结束。
    pub halted: bool,
    /// 执行过的指令数。
    pub steps_executed: usize,
    /// 结束时的程序计数器位置。
    pub final_pc: usize,
}

/// VM 运行时错误。
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum VmError {
    /// 栈中缺少当前指令所需值。
    #[error("栈下溢: opcode={opcode}")]
    StackUnderflow { opcode: &'static str },
    /// 跳转目标越界。
    #[error("跳转目标越界: target={target}, program_len={program_len}")]
    InvalidJumpTarget { target: usize, program_len: usize },
    /// 超出执行步数限制，防止无限循环。
    #[error("执行步数超限: limit={limit}")]
    StepLimitExceeded { limit: usize },
}

/// 合约运行时，维护栈、状态和事件输出。
#[derive(Debug, Default)]
pub struct Runtime {
    stack: Vec<i64>,
    state: HashMap<String, i64>,
    events: Vec<String>,
}

impl Runtime {
    /// 默认执行入口，使用内置步数限制。
    pub fn execute(&mut self, program: &[Opcode]) -> Result<ExecutionReport, VmError> {
        self.execute_with_limit(program, 10_000)
    }

    /// 带步数限制执行入口。
    pub fn execute_with_limit(
        &mut self,
        program: &[Opcode],
        max_steps: usize,
    ) -> Result<ExecutionReport, VmError> {
        let mut pc = 0usize;
        let mut steps = 0usize;
        let mut halted = false;

        while pc < program.len() {
            if steps >= max_steps {
                return Err(VmError::StepLimitExceeded { limit: max_steps });
            }

            match &program[pc] {
                Opcode::LoadConst(value) => {
                    self.stack.push(*value);
                    pc = pc.saturating_add(1);
                }
                Opcode::Store(key) => {
                    let value = self
                        .stack
                        .pop()
                        .ok_or(VmError::StackUnderflow { opcode: "store" })?;
                    self.state.insert(key.clone(), value);
                    pc = pc.saturating_add(1);
                }
                Opcode::Jump(target) => {
                    if *target >= program.len() {
                        return Err(VmError::InvalidJumpTarget {
                            target: *target,
                            program_len: program.len(),
                        });
                    }
                    pc = *target;
                }
                Opcode::JumpIfFalse(target) => {
                    let condition = self.stack.pop().ok_or(VmError::StackUnderflow {
                        opcode: "jump_if_false",
                    })?;

                    if condition == 0 {
                        if *target >= program.len() {
                            return Err(VmError::InvalidJumpTarget {
                                target: *target,
                                program_len: program.len(),
                            });
                        }
                        pc = *target;
                    } else {
                        pc = pc.saturating_add(1);
                    }
                }
                Opcode::Emit(event) => {
                    self.events.push(event.clone());
                    pc = pc.saturating_add(1);
                }
                Opcode::Halt => {
                    halted = true;
                    steps = steps.saturating_add(1);
                    break;
                }
            }

            steps = steps.saturating_add(1);
        }

        Ok(ExecutionReport {
            halted,
            steps_executed: steps,
            final_pc: pc,
        })
    }

    /// 只读访问状态变量。
    pub fn state(&self) -> &HashMap<String, i64> {
        &self.state
    }

    /// 只读访问事件列表。
    pub fn events(&self) -> &[String] {
        &self.events
    }
}

#[cfg(test)]
mod tests {
    use super::{Runtime, VmError};
    use crate::bytecode::Opcode;

    /// 验证运行时可以执行常量加载与变量存储。
    #[test]
    fn runtime_should_store_value_into_state() {
        let mut runtime = Runtime::default();
        let program = vec![
            Opcode::LoadConst(42),
            Opcode::Store("answer".to_string()),
            Opcode::Halt,
        ];

        let report = runtime.execute(&program).expect("执行应当成功");
        assert!(report.halted);
        assert_eq!(runtime.state().get("answer"), Some(&42));
    }

    /// 验证条件跳转会跳过不应触发的事件。
    #[test]
    fn jump_if_false_should_skip_emit() {
        let mut runtime = Runtime::default();
        let program = vec![
            Opcode::LoadConst(0),
            Opcode::JumpIfFalse(4),
            Opcode::Emit("should-not-fire".to_string()),
            Opcode::LoadConst(1),
            Opcode::Emit("ok".to_string()),
            Opcode::Halt,
        ];

        runtime.execute(&program).expect("执行应当成功");
        assert_eq!(runtime.events(), &["ok".to_string()]);
    }

    /// 验证非法跳转目标会被拒绝。
    #[test]
    fn invalid_jump_target_should_fail() {
        let mut runtime = Runtime::default();
        let program = vec![Opcode::Jump(99)];

        let result = runtime.execute(&program);
        assert_eq!(
            result,
            Err(VmError::InvalidJumpTarget {
                target: 99,
                program_len: 1,
            })
        );
    }

    /// 验证无限循环会触发步数限制保护。
    #[test]
    fn infinite_loop_should_hit_step_limit() {
        let mut runtime = Runtime::default();
        let program = vec![Opcode::Jump(0)];

        let result = runtime.execute_with_limit(&program, 3);
        assert_eq!(result, Err(VmError::StepLimitExceeded { limit: 3 }));
    }
}
