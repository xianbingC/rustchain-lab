# RustChain Lab

RustChain Lab is a Rust-based blockchain system and application development project. The repository uses a monorepo workspace layout and is intended to host the blockchain core, storage adapters, cryptography, P2P networking, a simulated smart-contract VM, and application modules for DeFi and NFT scenarios.

## Project Goals

- Build a blockchain prototype in Rust with PoW, block validation, and chain validation
- Provide state storage with RocksDB and historical storage with LevelDB
- Support wallet management and transaction signing with ed25519
- Implement a simple smart-contract simulation runtime
- Expose both CLI and REST API interfaces
- Deliver DeFi lending and NFT marketplace demo applications on top of the chain

## Repository Layout

```text
rustchain-lab/
├── crates/
│   ├── api/        # REST API service
│   ├── apps/       # DeFi and NFT application logic
│   ├── cli/        # Command-line interface
│   ├── common/     # Shared config, logging, and app-level errors
│   ├── core/       # Blocks, transactions, chain, consensus
│   ├── crypto/     # Wallets, keys, signatures
│   ├── p2p/        # Peer discovery, messaging, sync
│   ├── storage/    # RocksDB and LevelDB abstraction
│   └── vm/         # Simulated smart-contract compiler/runtime
└── docs/
    └── architecture.md
```

## Development Plan

This project follows a six-week plan with four hours per day and five days per week. The initial phase focuses on creating a runnable MVP that covers:

1. Blockchain core
2. Storage and persistence
3. Wallet and signing
4. P2P synchronization
5. CLI and REST API
6. DeFi and NFT application flows

## Current Status

The project is runnable and deployable in Linux/WSL environments with `cargo` installed. Core modules, API/CLI flows, and DeFi/NFT demos are available for end-to-end testing.

## Deployment Scripts

The repository includes helper scripts under `scripts/` for build and deployment:

- `scripts/build_api.sh`: build `rustchain-api` in release mode (RocksDB enabled by default)
- `scripts/deploy_api.sh`: local process management (`start/stop/restart/status/logs/health [live|ready]/metrics`)
- `scripts/install_systemd_service.sh`: install and manage a `systemd` service on Linux servers
- `scripts/backup_data.sh`: data backup/restore for `RUSTCHAIN_DATA_DIR`

### Quick Start (Local/WSL)

```bash
cd /path/to/rustchain-lab
./scripts/build_api.sh
./scripts/deploy_api.sh start
./scripts/deploy_api.sh health
./scripts/deploy_api.sh health live
./scripts/deploy_api.sh metrics
```

### Quick Start (systemd)

```bash
cd /path/to/rustchain-lab
./scripts/install_systemd_service.sh install
./scripts/install_systemd_service.sh status
./scripts/install_systemd_service.sh health
./scripts/install_systemd_service.sh metrics
./scripts/install_systemd_service.sh logs
```

### Data Backup / Restore

```bash
./scripts/backup_data.sh backup before-upgrade
./scripts/backup_data.sh list
./scripts/backup_data.sh restore /path/to/archive.tar.gz
```

Example environment file:

- `scripts/systemd/rustchain-api.env.example`

## Health Probes

API endpoints:

- `GET /health`
- `GET /health/live`
- `GET /health/ready`
- `GET /metrics`

CLI probe commands:

```bash
cargo run -q -p rustchain-cli -- health
cargo run -q -p rustchain-cli -- health live
cargo run -q -p rustchain-cli -- health ready
cargo run -q -p rustchain-cli -- health metrics
```
