# ---- build stage: compile the server and the bots ----
FROM rust:1-slim-bookworm AS build
WORKDIR /app
COPY . .
RUN cargo build --release -p server -p bots

# ---- run stage: small image with just the binaries and the web page ----
FROM debian:bookworm-slim
WORKDIR /app
COPY --from=build /app/target/release/server /app/target/release/bots /app/target/release/market_maker ./
COPY web ./web
COPY start.sh ./
RUN chmod +x start.sh
CMD ["./start.sh"]
