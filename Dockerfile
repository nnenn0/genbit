FROM rust:1.94-bookworm
RUN rustup component add rustfmt clippy
WORKDIR /workspace
