FROM rust:1.99.1-bookworm
RUN rustup component add rustfmt clippy
WORKDIR /workspace
