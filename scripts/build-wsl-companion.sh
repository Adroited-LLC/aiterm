#!/bin/sh
set -eu
source_dir=$1
build_dir="$HOME/.cache/aiterm-wsl-build"
mkdir -p "$build_dir" "$source_dir/windows/resources"
# Use WSL's clock for extracted sources. Preserved NTFS/archive timestamps can
# otherwise make Cargo reuse an older companion after a checkout or sync.
tar -C "$source_dir" --exclude=target -cf - wsl-backend wsl-protocol relay-protocol src-tauri/src scripts/test-wsl-backend.py scripts/test-wsl-service.py | tar -C "$build_dir" -mxf -
cd "$build_dir"
export PATH="$HOME/.cargo/bin:$PATH"
cargo test --locked --manifest-path wsl-protocol/Cargo.toml
cargo test --locked --manifest-path wsl-backend/Cargo.toml -- --test-threads=4
cargo build --locked --release --manifest-path wsl-backend/Cargo.toml
python3 scripts/test-wsl-backend.py wsl-backend/target/release/aiterm-wsl-backend
python3 scripts/test-wsl-service.py wsl-backend/target/release/aiterm-wsl-backend
cp wsl-backend/target/release/aiterm-wsl-backend "$source_dir/windows/resources/aiterm-wsl-backend"
