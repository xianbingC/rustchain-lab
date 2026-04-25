#!/usr/bin/env bash

# 失败即退出；未定义变量报错；管道中任意命令失败即失败。
set -euo pipefail

# 解析脚本所在目录，并定位仓库根目录。
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"

# 运行配置：可通过环境变量覆盖。
API_HOST="${RUSTCHAIN_API_HOST:-0.0.0.0}"
API_PORT="${RUSTCHAIN_API_PORT:-8080}"
DATA_DIR="${RUSTCHAIN_DATA_DIR:-$HOME/.rustchain-data}"
LOG_FILE="${RUSTCHAIN_API_LOG_FILE:-$HOME/rustchain-api.log}"
PID_FILE="${RUSTCHAIN_API_PID_FILE:-$HOME/.rustchain-api.pid}"
BINARY_PATH="${RUSTCHAIN_API_BIN:-${REPO_ROOT}/target/release/rustchain-api}"

# 是否自动构建，默认开启。
AUTO_BUILD="${AUTO_BUILD:-1}"

usage() {
  cat <<'EOF'
用法:
  ./scripts/deploy_api.sh start      # 启动服务（后台）
  ./scripts/deploy_api.sh stop       # 停止服务
  ./scripts/deploy_api.sh restart    # 重启服务
  ./scripts/deploy_api.sh status     # 查看服务状态
  ./scripts/deploy_api.sh logs       # 查看实时日志
  ./scripts/deploy_api.sh health [live|ready]  # 调用探针接口（默认 ready）

可选环境变量:
  RUSTCHAIN_API_HOST          默认 0.0.0.0
  RUSTCHAIN_API_PORT          默认 8080
  RUSTCHAIN_DATA_DIR          默认 $HOME/.rustchain-data
  RUSTCHAIN_API_LOG_FILE      默认 $HOME/rustchain-api.log
  RUSTCHAIN_API_PID_FILE      默认 $HOME/.rustchain-api.pid
  RUSTCHAIN_API_BIN           默认 <repo>/target/release/rustchain-api
  AUTO_BUILD                  默认 1；当二进制不存在时自动触发构建
  ENABLE_ROCKSDB              传递给 build_api.sh，默认 1
EOF
}

is_running() {
  [[ -f "${PID_FILE}" ]] || return 1
  local pid
  pid="$(cat "${PID_FILE}")"
  [[ -n "${pid}" ]] || return 1
  kill -0 "${pid}" >/dev/null 2>&1
}

build_if_needed() {
  if [[ -x "${BINARY_PATH}" ]]; then
    return 0
  fi

  if [[ "${AUTO_BUILD}" != "1" ]]; then
    echo "[deploy] 未找到可执行文件: ${BINARY_PATH}"
    echo "[deploy] 请先执行 ./scripts/build_api.sh"
    exit 1
  fi

  echo "[deploy] 未找到可执行文件，开始自动构建..."
  "${REPO_ROOT}/scripts/build_api.sh"
}

start_service() {
  if is_running; then
    echo "[deploy] 服务已在运行，PID=$(cat "${PID_FILE}")"
    return 0
  fi

  mkdir -p "${DATA_DIR}"
  build_if_needed

  echo "[deploy] 启动 rustchain-api..."
  nohup env \
    RUSTCHAIN_API_HOST="${API_HOST}" \
    RUSTCHAIN_API_PORT="${API_PORT}" \
    RUSTCHAIN_DATA_DIR="${DATA_DIR}" \
    "${BINARY_PATH}" \
    >"${LOG_FILE}" 2>&1 &

  local pid=$!
  echo "${pid}" >"${PID_FILE}"
  sleep 1

  if kill -0 "${pid}" >/dev/null 2>&1; then
    echo "[deploy] 启动成功，PID=${pid}"
    echo "[deploy] 日志文件: ${LOG_FILE}"
    echo "[deploy] 就绪检查: curl http://127.0.0.1:${API_PORT}/health/ready"
  else
    echo "[deploy] 启动失败，请检查日志: ${LOG_FILE}"
    exit 1
  fi
}

stop_service() {
  if ! is_running; then
    echo "[deploy] 服务未运行"
    rm -f "${PID_FILE}"
    return 0
  fi

  local pid
  pid="$(cat "${PID_FILE}")"
  echo "[deploy] 停止服务，PID=${pid}"
  kill "${pid}" >/dev/null 2>&1 || true

  # 等待进程优雅退出。
  for _ in {1..20}; do
    if ! kill -0 "${pid}" >/dev/null 2>&1; then
      rm -f "${PID_FILE}"
      echo "[deploy] 已停止"
      return 0
    fi
    sleep 0.2
  done

  # 超时后强制结束。
  kill -9 "${pid}" >/dev/null 2>&1 || true
  rm -f "${PID_FILE}"
  echo "[deploy] 已强制停止"
}

status_service() {
  if is_running; then
    echo "[deploy] 运行中，PID=$(cat "${PID_FILE}")"
    return 0
  fi
  echo "[deploy] 未运行"
}

health_check() {
  local probe="${1:-ready}"
  local path="/health/ready"
  if [[ "${probe}" == "live" ]]; then
    path="/health/live"
  elif [[ "${probe}" != "ready" ]]; then
    echo "[deploy] 未知 health 探针: ${probe}，可用: live/ready"
    exit 1
  fi

  curl -fsS "http://127.0.0.1:${API_PORT}${path}"
  echo
}

main() {
  local cmd="${1:-}"
  case "${cmd}" in
    start)
      start_service
      ;;
    stop)
      stop_service
      ;;
    restart)
      stop_service
      start_service
      ;;
    status)
      status_service
      ;;
    logs)
      touch "${LOG_FILE}"
      tail -f "${LOG_FILE}"
      ;;
    health)
      health_check "${2:-ready}"
      ;;
    ""|-h|--help|help)
      usage
      ;;
    *)
      echo "[deploy] 未知命令: ${cmd}"
      usage
      exit 1
      ;;
  esac
}

main "$@"
