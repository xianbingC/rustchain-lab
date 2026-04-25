#!/usr/bin/env bash

# 失败即退出；未定义变量报错；管道中任意命令失败即失败。
set -euo pipefail

# 两个节点 API 地址（可覆盖）。
NODE_A_URL="${NODE_A_URL:-http://127.0.0.1:8080}"
NODE_B_URL="${NODE_B_URL:-http://127.0.0.1:8081}"

# 两个节点用于注册的 peer 信息（可覆盖）。
NODE_A_PEER_ID="${NODE_A_PEER_ID:-node-a}"
NODE_B_PEER_ID="${NODE_B_PEER_ID:-node-b}"
NODE_A_P2P_ADDR="${NODE_A_P2P_ADDR:-127.0.0.1:7000}"
NODE_B_P2P_ADDR="${NODE_B_P2P_ADDR:-127.0.0.1:7001}"

usage() {
  cat <<'EOF'
用法:
  ./scripts/multi_node_check.sh smoke     # 双节点探针 + 指标 + 链摘要对比
  ./scripts/multi_node_check.sh register  # 双向 P2P 注册并校验节点列表
  ./scripts/multi_node_check.sh full      # 执行 smoke + register

可选环境变量:
  NODE_A_URL        默认 http://127.0.0.1:8080
  NODE_B_URL        默认 http://127.0.0.1:8081
  NODE_A_PEER_ID    默认 node-a
  NODE_B_PEER_ID    默认 node-b
  NODE_A_P2P_ADDR   默认 127.0.0.1:7000
  NODE_B_P2P_ADDR   默认 127.0.0.1:7001
EOF
}

log() {
  echo "[multi-node] $*"
}

get_json() {
  local url="$1"
  curl -fsS "${url}"
}

get_chain_field() {
  local base_url="$1"
  local field="$2"
  get_json "${base_url}/chain/info" | python3 -c "
import json,sys
data=json.load(sys.stdin)
chain=data.get('chain',{})
print(chain.get('${field}',''))
"
}

get_peers_count() {
  local base_url="$1"
  get_json "${base_url}/p2p/peers" | python3 -c "
import json,sys
data=json.load(sys.stdin)
peers=data.get('peers',[])
print(len(peers))
"
}

check_probe_and_metrics() {
  local base_url="$1"
  log "检查节点 ${base_url} live/ready/metrics"
  get_json "${base_url}/health/live" >/dev/null
  get_json "${base_url}/health/ready" >/dev/null
  if ! curl -fsS "${base_url}/metrics" | grep -q "rustchain_up 1"; then
    echo "[multi-node] 指标检查失败: ${base_url}/metrics 未包含 rustchain_up 1"
    exit 1
  fi
}

compare_chain_snapshot() {
  local a_chain_id a_height a_diff
  local b_chain_id b_height b_diff

  a_chain_id="$(get_chain_field "${NODE_A_URL}" "chain_id")"
  a_height="$(get_chain_field "${NODE_A_URL}" "height")"
  a_diff="$(get_chain_field "${NODE_A_URL}" "difficulty")"

  b_chain_id="$(get_chain_field "${NODE_B_URL}" "chain_id")"
  b_height="$(get_chain_field "${NODE_B_URL}" "height")"
  b_diff="$(get_chain_field "${NODE_B_URL}" "difficulty")"

  log "节点A: chain_id=${a_chain_id}, height=${a_height}, difficulty=${a_diff}"
  log "节点B: chain_id=${b_chain_id}, height=${b_height}, difficulty=${b_diff}"

  if [[ "${a_chain_id}" != "${b_chain_id}" ]]; then
    echo "[multi-node] chain_id 不一致，判定失败。"
    exit 1
  fi
  if [[ "${a_diff}" != "${b_diff}" ]]; then
    echo "[multi-node] difficulty 不一致，判定失败。"
    exit 1
  fi

  log "链摘要基础校验通过（chain_id/difficulty 一致）"
}

register_peer() {
  local from_url="$1"
  local peer_id="$2"
  local p2p_addr="$3"
  curl -fsS -X POST "${from_url}/p2p/peer/register" \
    -H "content-type: application/json" \
    -d "{\"peer_id\":\"${peer_id}\",\"address\":\"${p2p_addr}\"}" >/dev/null
}

register_both_sides() {
  log "执行双向 P2P 注册"
  register_peer "${NODE_A_URL}" "${NODE_B_PEER_ID}" "${NODE_B_P2P_ADDR}"
  register_peer "${NODE_B_URL}" "${NODE_A_PEER_ID}" "${NODE_A_P2P_ADDR}"

  local a_count b_count
  a_count="$(get_peers_count "${NODE_A_URL}")"
  b_count="$(get_peers_count "${NODE_B_URL}")"
  log "节点A peers=${a_count}"
  log "节点B peers=${b_count}"

  if [[ "${a_count}" -lt 1 || "${b_count}" -lt 1 ]]; then
    echo "[multi-node] peers 数量校验失败。"
    exit 1
  fi
}

run_smoke() {
  check_probe_and_metrics "${NODE_A_URL}"
  check_probe_and_metrics "${NODE_B_URL}"
  compare_chain_snapshot
  log "smoke 检查通过"
}

run_full() {
  run_smoke
  register_both_sides
  log "full 检查通过"
}

main() {
  local cmd="${1:-}"
  case "${cmd}" in
    smoke)
      run_smoke
      ;;
    register)
      register_both_sides
      ;;
    full)
      run_full
      ;;
    ""|-h|--help|help)
      usage
      ;;
    *)
      echo "[multi-node] 未知命令: ${cmd}"
      usage
      exit 1
      ;;
  esac
}

main "$@"
