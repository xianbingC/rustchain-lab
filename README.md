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

The workspace skeleton and module boundaries are initialized. Rust tooling is not installed in the current local environment yet, so the repository is scaffolded manually and ready to continue in an environment where `cargo` is available.
