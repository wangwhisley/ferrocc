# 第一階段：編譯
FROM rust:1.99-slim-bookworm AS builder

WORKDIR /app
COPY Cargo.toml ./
RUN mkdir src && echo "fn main() {}" > src/main.rs && cargo build --release && rm -rf src

COPY src ./src
RUN touch src/main.rs && cargo build --release

# 第二階段：精簡執行環境
FROM debian:bookworm-slim

WORKDIR /app
COPY --from=builder /app/target/release/ferrocc /usr/local/bin/ferrocc

CMD ["ferrocc"]
