FROM rust:1.99.0-bookworm
RUN rustup component add rustfmt clippy
WORKDIR /workspace
