#!/bin/sh
set -eu
source_dir=$1
build_dir="$HOME/.cache/aiterm-wsl-build"
mkdir -p "$build_dir" "$source_dir/windows/resources"
tar -C "$source_dir" --exclude=target -cf - wsl-backend wsl-protocol src-tauri/src/git.rs src-tauri/src/fsx.rs scripts/test-wsl-backend.py | tar -C "$build_dir" -xf -
cd "$build_dir"
export PATH="$HOME/.cargo/bin:$PATH"
cargo test --locked --manifest-path wsl-protocol/Cargo.toml
cargo test --locked --manifest-path wsl-backend/Cargo.toml
cargo build --locked --release --manifest-path wsl-backend/Cargo.toml
python3 scripts/test-wsl-backend.py wsl-backend/target/release/aiterm-wsl-backend
cp wsl-backend/target/release/aiterm-wsl-backend "$source_dir/windows/resources/aiterm-wsl-backend"
