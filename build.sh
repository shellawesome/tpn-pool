#!/bin/bash
set -e
cargo build --release
cp target/release/tpn-pool .
echo "build ok: ./tpn-pool ($(du -h tpn-pool | cut -f1))"
