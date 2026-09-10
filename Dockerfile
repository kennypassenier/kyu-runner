# Two stages (T8). The build stage is the only place a compiler, a libc
# header or a package manager exists; the runtime stage is
# distroless/static, which has neither a shell nor apt, so the container
# HEALTHCHECK uses the binary's own --healthcheck.
#
# feat-build-1: the binary is linked statically against musl, so the host's
# glibc stops deciding whether the service starts — a glibc build made here
# needs GLIBC_2.39 and will not run on Debian 12. `ring` (rustls' crypto)
# compiles C for the target, hence musl-tools. The musl target itself comes
# from rust-toolchain.toml, which is only in scope after COPY; a
# `rustup target add` before that installs it against another toolchain
# instance and fails silently later (kyu, 2026-09-09).
FROM rust:1.97-slim-trixie AS build
RUN apt-get update -qq && apt-get install -y -qq --no-install-recommends musl-tools && rm -rf /var/lib/apt/lists/*
WORKDIR /src
COPY . .
RUN cargo build --release --locked --target x86_64-unknown-linux-musl
# The runtime stage cannot run a command, so the state directory is made
# here and copied with its owner.
RUN mkdir -p /state

# distroless/static ships ca-certificates and a `nonroot` account at uid
# 65532, so neither the apt line nor a useradd is needed; a static binary
# needs nothing else from the image.
FROM gcr.io/distroless/static:nonroot
COPY --from=build /src/target/x86_64-unknown-linux-musl/release/kyu-runner /usr/local/bin/kyu-runner
COPY --from=build --chown=65532:65532 /state /var/lib/kyu-runner
USER 65532:65532
ENV KYU_RUNNER_LISTEN=0.0.0.0:8080 KYU_RUNNER_STATE_DIR=/var/lib/kyu-runner
EXPOSE 8080
VOLUME ["/var/lib/kyu-runner"]
# Self-update is off inside an image by detection (AR8); updates are a new image.
HEALTHCHECK --interval=30s --timeout=5s --retries=3 CMD ["/usr/local/bin/kyu-runner", "--healthcheck"]
ENTRYPOINT ["/usr/local/bin/kyu-runner"]
