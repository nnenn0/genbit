FROM rust:1.98.1-bookworm
RUN rustup component add rustfmt clippy
WORKDIR /workspace
