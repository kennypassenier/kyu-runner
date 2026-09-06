# Two stages on the same Debian the LXCs run (T8): a glibc binary that
# also works copied out of the image. The runtime stage has no shell
# tools, so the container HEALTHCHECK uses the binary's own --healthcheck.
FROM rust:1.97-slim-trixie AS build
RUN apt-get update -qq && apt-get install -y -qq --no-install-recommends pkg-config libssl-dev && rm -rf /var/lib/apt/lists/*
WORKDIR /src
COPY . .
RUN cargo build --release --locked

FROM debian:trixie-slim
RUN apt-get update -qq && apt-get install -y -qq --no-install-recommends ca-certificates libssl3t64 && rm -rf /var/lib/apt/lists/* \
    && useradd --system --uid 10001 --home /var/lib/kyu-runner --shell /usr/sbin/nologin kyu-runner \
    && mkdir -p /var/lib/kyu-runner && chown kyu-runner:kyu-runner /var/lib/kyu-runner
COPY --from=build /src/target/release/kyu-runner /usr/local/bin/kyu-runner
USER kyu-runner
ENV KYU_RUNNER_LISTEN=0.0.0.0:8080 KYU_RUNNER_STATE_DIR=/var/lib/kyu-runner
EXPOSE 8080
VOLUME ["/var/lib/kyu-runner"]
# Self-update is off inside an image by detection (AR8); updates are a new image.
HEALTHCHECK --interval=30s --timeout=5s --retries=3 CMD ["/usr/local/bin/kyu-runner", "--healthcheck"]
ENTRYPOINT ["/usr/local/bin/kyu-runner"]
