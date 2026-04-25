#!/usr/bin/env bash

# 失败即退出；未定义变量报错；管道中任意命令失败即失败。
set -euo pipefail

# 默认目录（可通过环境变量覆盖）。
DATA_DIR="${RUSTCHAIN_DATA_DIR:-$HOME/.rustchain-data}"
BACKUP_ROOT="${RUSTCHAIN_BACKUP_DIR:-$HOME/.rustchain-backups}"
SERVICE_NAME="${SERVICE_NAME:-rustchain-api}"

usage() {
  cat <<'EOF'
用法:
  ./scripts/backup_data.sh backup [tag]        # 备份数据目录为 tar.gz
  ./scripts/backup_data.sh list                # 列出已有备份
  ./scripts/backup_data.sh restore <archive>   # 从备份恢复（会覆盖 DATA_DIR）

可选环境变量:
  RUSTCHAIN_DATA_DIR     默认 $HOME/.rustchain-data
  RUSTCHAIN_BACKUP_DIR   默认 $HOME/.rustchain-backups
  SERVICE_NAME           默认 rustchain-api（用于提示停服）

示例:
  ./scripts/backup_data.sh backup before-upgrade
  ./scripts/backup_data.sh list
  ./scripts/backup_data.sh restore /home/user/.rustchain-backups/rustchain-data-20260425-120000-before-upgrade.tar.gz
EOF
}

ensure_backup_root() {
  mkdir -p "${BACKUP_ROOT}"
}

backup_data() {
  local tag="${1:-manual}"
  local ts
  ts="$(date +%Y%m%d-%H%M%S)"
  local archive="${BACKUP_ROOT}/rustchain-data-${ts}-${tag}.tar.gz"

  if [[ ! -d "${DATA_DIR}" ]]; then
    echo "[backup] 数据目录不存在: ${DATA_DIR}"
    exit 1
  fi

  echo "[backup] 开始备份..."
  echo "[backup] data_dir=${DATA_DIR}"
  echo "[backup] archive=${archive}"

  ensure_backup_root
  tar -C "$(dirname "${DATA_DIR}")" -czf "${archive}" "$(basename "${DATA_DIR}")"

  echo "[backup] 完成: ${archive}"
}

list_backups() {
  ensure_backup_root
  echo "[backup] 备份目录: ${BACKUP_ROOT}"
  ls -lh "${BACKUP_ROOT}"/*.tar.gz 2>/dev/null || echo "[backup] 暂无备份文件"
}

restore_data() {
  local archive="${1:-}"
  if [[ -z "${archive}" ]]; then
    echo "[restore] 缺少备份文件路径"
    usage
    exit 1
  fi
  if [[ ! -f "${archive}" ]]; then
    echo "[restore] 备份文件不存在: ${archive}"
    exit 1
  fi

  echo "[restore] 即将恢复数据目录:"
  echo "  archive=${archive}"
  echo "  data_dir=${DATA_DIR}"
  echo "[restore] 强烈建议先停服务: systemctl stop ${SERVICE_NAME}"

  local parent
  parent="$(dirname "${DATA_DIR}")"
  mkdir -p "${parent}"
  rm -rf "${DATA_DIR}"
  tar -C "${parent}" -xzf "${archive}"

  echo "[restore] 完成。请确认后启动服务。"
}

main() {
  local cmd="${1:-}"
  case "${cmd}" in
    backup)
      backup_data "${2:-manual}"
      ;;
    list)
      list_backups
      ;;
    restore)
      restore_data "${2:-}"
      ;;
    ""|-h|--help|help)
      usage
      ;;
    *)
      echo "[backup] 未知命令: ${cmd}"
      usage
      exit 1
      ;;
  esac
}

main "$@"
