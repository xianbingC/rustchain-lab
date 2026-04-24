#!/usr/bin/env bash

# 失败即退出；未定义变量报错；管道中任意命令失败即失败。
set -euo pipefail

# 解析脚本所在目录，并回到仓库根目录执行构建。
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"

# 是否启用 RocksDB 后端，默认启用（符合当前完整部署形态）。
ENABLE_ROCKSDB="${ENABLE_ROCKSDB:-1}"

if [[ "${ENABLE_ROCKSDB}" == "1" ]]; then
  echo "[build] 使用 rocksdb-backend 构建 rustchain-api (release)"
  (
    cd "${REPO_ROOT}"
    cargo build --release -p rustchain-api --features rocksdb-backend
  )
else
  echo "[build] 使用默认特性构建 rustchain-api (release)"
  (
    cd "${REPO_ROOT}"
    cargo build --release -p rustchain-api
  )
fi

echo "[build] 完成: ${REPO_ROOT}/target/release/rustchain-api"
