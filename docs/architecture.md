# RustChain Lab Architecture

## Overview

RustChain Lab is organized as a Rust workspace with focused crates for infrastructure, blockchain runtime, and user-facing interfaces.

## Crate Responsibilities

### `crates/core`

- Transaction, block, and blockchain data structures
- Hashing, Merkle tree calculation, PoW, block validation
- Mempool and chain verification

### `crates/storage`

- Storage traits and adapters
- RocksDB-backed state storage
- LevelDB-backed historical persistence

### `crates/crypto`

- Key generation and wallet management
- Transaction signing and signature verification

### `crates/p2p`

- Peer metadata and network messages
- Node discovery and synchronization
- Serialization and transport abstractions

### `crates/vm`

- Simulated contract language model
- Bytecode compilation
- Contract execution environment and event model

### `crates/apps`

- DeFi lending domain logic
- NFT minting and marketplace flows

### `crates/cli`

- Command entry points for wallets, transactions, mining, and queries

### `crates/api`

- REST endpoints for chain info, balances, transactions, contracts, and apps

## Phase 1 Engineering Principles

1. Keep interfaces small and explicit.
2. Build the blockchain core before advanced application logic.
3. Prefer testable abstractions over deep coupling between crates.
4. Keep the smart-contract engine intentionally minimal for the MVP.
