//! 智能合约 VM 接口：编译与执行。

use crate::error::{map_vm_compile_error, map_vm_runtime_error};
use axum::{http::StatusCode, Json};
use rustchain_vm::{compiler::compile, runtime::Runtime};
use serde::Deserialize;
use serde_json::json;

/// VM 编译请求。
#[derive(Debug, Deserialize)]
pub(crate) struct VmCompileRequest {
    /// 合约源码文本。
    pub(crate) source: String,
}

/// VM 执行请求。
#[derive(Debug, Deserialize)]
pub(crate) struct VmExecuteRequest {
    /// 合约源码文本。
    pub(crate) source: String,
    /// 可选步数上限。
    pub(crate) max_steps: Option<usize>,
}

/// VM 编译接口：将源码编译为指令序列。
pub(crate) async fn vm_compile_handler(
    Json(payload): Json<VmCompileRequest>,
) -> (StatusCode, Json<serde_json::Value>) {
    if payload.source.trim().is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({
                "ok": false,
                "error": "source 不能为空"
            })),
        );
    }

    match compile(&payload.source) {
        Ok(program) => (
            StatusCode::OK,
            Json(json!({
                "ok": true,
                "instruction_count": program.len(),
                "program": program
            })),
        ),
        Err(error) => {
            let (status, body) = map_vm_compile_error(error);
            (status, Json(body))
        }
    }
}

/// VM 执行接口：编译源码并执行，返回状态与事件。
pub(crate) async fn vm_execute_handler(
    Json(payload): Json<VmExecuteRequest>,
) -> (StatusCode, Json<serde_json::Value>) {
    if payload.source.trim().is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({
                "ok": false,
                "error": "source 不能为空"
            })),
        );
    }

    let max_steps = payload.max_steps.unwrap_or(10_000);
    if max_steps == 0 {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({
                "ok": false,
                "error": "max_steps 必须大于 0"
            })),
        );
    }

    let program = match compile(&payload.source) {
        Ok(program) => program,
        Err(error) => {
            let (status, body) = map_vm_compile_error(error);
            return (status, Json(body));
        }
    };

    let mut runtime = Runtime::default();
    match runtime.execute_with_limit(&program, max_steps) {
        Ok(report) => (
            StatusCode::OK,
            Json(json!({
                "ok": true,
                "instruction_count": program.len(),
                "report": {
                    "halted": report.halted,
                    "steps_executed": report.steps_executed,
                    "final_pc": report.final_pc
                },
                "state": runtime.state(),
                "events": runtime.events()
            })),
        ),
        Err(error) => {
            let (status, body) = map_vm_runtime_error(error);
            (status, Json(body))
        }
    }
}
