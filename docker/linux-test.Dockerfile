# Linux-Tests von Qross mit virtuellem X-Server (Xvfb).
#
#   docker build -f docker/linux-test.Dockerfile -t qross-linux-test docker
#   docker run --rm -v "<workspace>:/src" -v qross-cargo:/usr/local/cargo/registry -v qross-target:/target qross-linux-test
FROM rust:1.99.1-slim-trixie
RUN apt-get update \
    && apt-get install -y --no-install-recommends xvfb \
    && rm -rf /var/lib/apt/lists/*
ENV CARGO_TARGET_DIR=/target DISPLAY=:99
WORKDIR /src
# Ohne GUI-Crate (braucht unter Linux GTK/WebKit)
CMD Xvfb :99 -screen 0 1280x800x24 -nolisten tcp >/dev/null 2>&1 & sleep 1 && \
    cargo test --locked -p qross-proto -p qross-core -p qross-platform -p qross-net -p qross
