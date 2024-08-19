#!/bin/bash

RUST_LOG=info cargo run --release --bin jito-shredstream-proxy -- shredstream \
    --block-engine-url https://amsterdam.mainnet.block-engine.jito.wtf \
    --auth-keypair $HOME/solana/keys/auth.json \
    --desired-regions amsterdam \
    --dest-ip-ports 0.0.0.0:8001
