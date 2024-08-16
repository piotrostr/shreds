#!/bin/bash

RUST_LOG=info cargo run --release --bin jito-shredstream-proxy -- shredstream \
    --block-engine-url https://frankfurt.mainnet.block-engine.jito.wtf \
    --auth-keypair /root/keys/auth.json \
    --desired-regions frankfurt,amsterdam \
    --dest-ip-ports 127.0.0.1:8001
