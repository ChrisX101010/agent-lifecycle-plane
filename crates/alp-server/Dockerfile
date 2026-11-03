FROM rust:bookworm AS build

WORKDIR /src
COPY . .

RUN cargo build --release -p alp-server

FROM debian:bookworm-slim

RUN useradd -r -u 10001 alp

COPY --from=build /src/target/release/alp-server /usr/local/bin/alp-server

USER alp
EXPOSE 8080

ENTRYPOINT ["alp-server"]
