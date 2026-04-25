#!/usr/bin/env bash

# 失败即退出；未定义变量报错；管道中任意命令失败即失败。
set -euo pipefail

# 解析脚本与仓库目录。
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"
TEMPLATE_PATH="${REPO_ROOT}/scripts/systemd/rustchain-api.service.template"

# 服务基础配置（均可通过环境变量覆盖）。
SERVICE_NAME="${SERVICE_NAME:-rustchain-api}"
SERVICE_USER="${SERVICE_USER:-$(id -un)}"
WORK_DIR="${WORK_DIR:-${REPO_ROOT}}"
INSTALL_BINARY_PATH="${INSTALL_BINARY_PATH:-/usr/local/bin/rustchain-api}"
SERVICE_FILE_PATH="/etc/systemd/system/${SERVICE_NAME}.service"
ENV_FILE_PATH="${ENV_FILE_PATH:-/etc/rustchain/rustchain-api.env}"
ENV_DIR_PATH="$(dirname "${ENV_FILE_PATH}")"

# 服务运行配置（写入 EnvironmentFile）。
API_HOST="${RUSTCHAIN_API_HOST:-0.0.0.0}"
API_PORT="${RUSTCHAIN_API_PORT:-8080}"
P2P_BIND_ADDR="${RUSTCHAIN_P2P_BIND_ADDR:-0.0.0.0:7000}"
LOG_LEVEL="${RUSTCHAIN_LOG_LEVEL:-info}"

if [[ "${SERVICE_USER}" == "root" ]]; then
  DEFAULT_DATA_DIR="/root/.rustchain-data"
else
  DEFAULT_DATA_DIR="/home/${SERVICE_USER}/.rustchain-data"
fi
DATA_DIR="${RUSTCHAIN_DATA_DIR:-${DEFAULT_DATA_DIR}}"

# 是否安装前自动构建，默认开启；构建时默认启用 rocksdb-backend。
AUTO_BUILD="${AUTO_BUILD:-1}"
ENABLE_ROCKSDB="${ENABLE_ROCKSDB:-1}"
BUILD_OUTPUT_PATH="${REPO_ROOT}/target/release/rustchain-api"

usage() {
  cat <<'EOF'
用法:
  ./scripts/install_systemd_service.sh install     # 安装并启动 systemd 服务
  ./scripts/install_systemd_service.sh status      # 查看服务状态
  ./scripts/install_systemd_service.sh restart     # 重启服务
  ./scripts/install_systemd_service.sh stop        # 停止服务
  ./scripts/install_systemd_service.sh logs        # 实时查看日志
  ./scripts/install_systemd_service.sh uninstall   # 卸载服务（保留数据目录）

常用环境变量:
  SERVICE_NAME            默认 rustchain-api
  SERVICE_USER            默认当前用户
  INSTALL_BINARY_PATH     默认 /usr/local/bin/rustchain-api
  ENV_FILE_PATH           默认 /etc/rustchain/rustchain-api.env
  WORK_DIR                默认仓库根目录
  AUTO_BUILD              默认 1（安装前自动构建）
  ENABLE_ROCKSDB          默认 1（传递给 build_api.sh）

运行时配置（写入 env 文件）:
  RUSTCHAIN_API_HOST      默认 0.0.0.0
  RUSTCHAIN_API_PORT      默认 8080
  RUSTCHAIN_P2P_BIND_ADDR 默认 0.0.0.0:7000
  RUSTCHAIN_LOG_LEVEL     默认 info
  RUSTCHAIN_DATA_DIR      默认 /home/<SERVICE_USER>/.rustchain-data（root 为 /root/.rustchain-data）
EOF
}

need_root() {
  if [[ "${EUID}" -eq 0 ]]; then
    "$@"
  else
    sudo "$@"
  fi
}

ensure_dependency() {
  if ! command -v systemctl >/dev/null 2>&1; then
    echo "[systemd] 未找到 systemctl，请在支持 systemd 的 Linux 环境执行。"
    exit 1
  fi

  if [[ ! -f "${TEMPLATE_PATH}" ]]; then
    echo "[systemd] 服务模板不存在: ${TEMPLATE_PATH}"
    exit 1
  fi

  if ! id "${SERVICE_USER}" >/dev/null 2>&1; then
    echo "[systemd] 用户不存在: ${SERVICE_USER}"
    exit 1
  fi
}

build_binary_if_needed() {
  if [[ "${AUTO_BUILD}" != "1" && ! -x "${BUILD_OUTPUT_PATH}" ]]; then
    echo "[systemd] 未找到构建产物: ${BUILD_OUTPUT_PATH}"
    echo "[systemd] 可启用 AUTO_BUILD=1 或先执行 ./scripts/build_api.sh"
    exit 1
  fi

  if [[ "${AUTO_BUILD}" == "1" ]]; then
    echo "[systemd] 开始构建 rustchain-api..."
    ENABLE_ROCKSDB="${ENABLE_ROCKSDB}" "${REPO_ROOT}/scripts/build_api.sh"
  fi

  if [[ ! -x "${BUILD_OUTPUT_PATH}" ]]; then
    echo "[systemd] 构建后仍未找到二进制: ${BUILD_OUTPUT_PATH}"
    exit 1
  fi
}

install_binary() {
  echo "[systemd] 安装二进制到 ${INSTALL_BINARY_PATH}"
  need_root install -d "$(dirname "${INSTALL_BINARY_PATH}")"
  need_root install -m 0755 "${BUILD_OUTPUT_PATH}" "${INSTALL_BINARY_PATH}"
}

write_env_file() {
  echo "[systemd] 写入环境配置: ${ENV_FILE_PATH}"
  need_root install -d "${ENV_DIR_PATH}"

  local tmp_env
  tmp_env="$(mktemp)"
  cat >"${tmp_env}" <<EOF
RUSTCHAIN_API_HOST=${API_HOST}
RUSTCHAIN_API_PORT=${API_PORT}
RUSTCHAIN_P2P_BIND_ADDR=${P2P_BIND_ADDR}
RUSTCHAIN_LOG_LEVEL=${LOG_LEVEL}
RUSTCHAIN_DATA_DIR=${DATA_DIR}
EOF
  need_root install -m 0644 "${tmp_env}" "${ENV_FILE_PATH}"
  rm -f "${tmp_env}"

  echo "[systemd] 创建数据目录: ${DATA_DIR}"
  need_root install -d -m 0755 -o "${SERVICE_USER}" -g "${SERVICE_USER}" "${DATA_DIR}"
}

render_service_file() {
  echo "[systemd] 生成服务文件: ${SERVICE_FILE_PATH}"
  local tmp_service
  tmp_service="$(mktemp)"

  sed \
    -e "s|__SERVICE_USER__|${SERVICE_USER}|g" \
    -e "s|__ENV_FILE__|${ENV_FILE_PATH}|g" \
    -e "s|__WORK_DIR__|${WORK_DIR}|g" \
    -e "s|__BINARY_PATH__|${INSTALL_BINARY_PATH}|g" \
    "${TEMPLATE_PATH}" >"${tmp_service}"

  need_root install -m 0644 "${tmp_service}" "${SERVICE_FILE_PATH}"
  rm -f "${tmp_service}"
}

install_service() {
  ensure_dependency
  build_binary_if_needed
  install_binary
  write_env_file
  render_service_file

  echo "[systemd] 重新加载并启动服务..."
  need_root systemctl daemon-reload
  need_root systemctl enable --now "${SERVICE_NAME}"

  echo "[systemd] 安装完成。"
  echo "[systemd] 查看状态: systemctl status ${SERVICE_NAME}"
  echo "[systemd] 查看日志: journalctl -u ${SERVICE_NAME} -f"
  echo "[systemd] 健康检查: curl http://127.0.0.1:${API_PORT}/health"
}

status_service() {
  ensure_dependency
  systemctl status "${SERVICE_NAME}" --no-pager
}

restart_service() {
  ensure_dependency
  need_root systemctl restart "${SERVICE_NAME}"
  echo "[systemd] 已重启 ${SERVICE_NAME}"
}

stop_service() {
  ensure_dependency
  need_root systemctl stop "${SERVICE_NAME}"
  echo "[systemd] 已停止 ${SERVICE_NAME}"
}

logs_service() {
  ensure_dependency
  journalctl -u "${SERVICE_NAME}" -f
}

uninstall_service() {
  ensure_dependency
  echo "[systemd] 卸载服务: ${SERVICE_NAME}"
  need_root systemctl disable --now "${SERVICE_NAME}" >/dev/null 2>&1 || true
  need_root rm -f "${SERVICE_FILE_PATH}"
  need_root systemctl daemon-reload

  # 卸载时默认保留环境文件和数据目录，便于后续恢复。
  echo "[systemd] 已卸载 service 文件，保留如下路径:"
  echo "  - ${ENV_FILE_PATH}"
  echo "  - ${DATA_DIR}"
  echo "  - ${INSTALL_BINARY_PATH}"
}

main() {
  local cmd="${1:-}"
  case "${cmd}" in
    install)
      install_service
      ;;
    status)
      status_service
      ;;
    restart)
      restart_service
      ;;
    stop)
      stop_service
      ;;
    logs)
      logs_service
      ;;
    uninstall)
      uninstall_service
      ;;
    ""|-h|--help|help)
      usage
      ;;
    *)
      echo "[systemd] 未知命令: ${cmd}"
      usage
      exit 1
      ;;
  esac
}

main "$@"
