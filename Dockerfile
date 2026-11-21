FROM rust:1.100.0-bookworm
RUN rustup component add rustfmt clippy
WORKDIR /workspace
