# ------------------------------------------------------------------------------
# Cargo Build Stage
# ------------------------------------------------------------------------------

FROM rust:slim-buster as cargo-build

RUN apt-get update

RUN apt-get install musl-tools -y

RUN rustup target add x86_64-unknown-linux-musl

WORKDIR /usr/src/megallery

COPY Cargo.toml Cargo.toml

RUN mkdir src/

RUN echo "fn main() {println!(\"if you see this, the build broke\")}" > src/main.rs

RUN cargo build --release

RUN rm -f target/release/deps/megallery*

COPY . .

RUN cargo build --release

# ------------------------------------------------------------------------------
# Final Stage
# ------------------------------------------------------------------------------

FROM debian:buster

RUN groupadd -g 1000 megallery

RUN useradd -u 1000 -g 1000 megallery

WORKDIR /home/megallery/bin/

COPY --from=cargo-build /usr/src/megallery/target/release/megallery .

RUN chown megallery:megallery megallery

USER megallery

CMD ["./megallery"]