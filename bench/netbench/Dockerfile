# Linux-Umgebung mit `tc netem`, damit TCP und QUIC unter identischem
# Kernel-Paketverlust gemessen werden können.
#
#   docker build -t netbench .
#   docker run --rm --cap-add NET_ADMIN netbench --link netem --loss 0,1,5
FROM rust:1.99.1-slim-trixie
RUN apt-get update \
    && apt-get install -y --no-install-recommends iproute2 \
    && rm -rf /var/lib/apt/lists/*
WORKDIR /src
COPY Cargo.toml Cargo.lock ./
COPY src ./src
RUN cargo build --release
ENTRYPOINT ["/src/target/release/netbench"]
