#!/bin/bash

# MC-Link Relay Server Linux Build Script
# Build Linux x86_64 binary using Docker

set -e

IMAGE=rust:1.75
CONTAINER_NAME=mc-link-relay-builder
PROJECT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
VERSION="0.2.1"

echo "==========================================="
echo "Building MC-Link Relay Server for Linux"
echo "==========================================="

docker build -t mc-link-relay-builder -f - . <<EOF
FROM rust:1.75 as builder
WORKDIR /build
RUN apt-get update && apt-get install -y musl-tools && rustup target add x86_64-unknown-linux-musl
COPY . .
RUN cargo build --release --target x86_64-unknown-linux-musl
RUN mv target/x86_64-unknown-linux-musl/release/mc-link-relay /build/mc-link-relay
EOF

docker create --name $CONTAINER_NAME $IMAGE
docker cp $CONTAINER_NAME:/build/mc-link-relay "$PROJECT_DIR/mc-link-relay-$VERSION-linux-x86_64"
docker rm $CONTAINER_NAME

echo "Build complete!"
echo "Output: $PROJECT_DIR/mc-link-relay-$VERSION-linux-x86_64"