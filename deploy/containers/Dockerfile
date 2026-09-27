# A container image for testing Highland against another instance.
#
# Not a production deployment: VRRP needs a layer-2 segment, which a bridge in
# one container cannot provide on its own. See tests/network-ns/.
FROM rust:1-slim AS build
WORKDIR /src
COPY . .
RUN cargo build --release --workspace

FROM debian:trixie-slim
RUN apt-get update \
 && apt-get install -y --no-install-recommends ca-certificates iproute2 \
 && rm -rf /var/lib/apt/lists/*
COPY --from=build /src/target/release/highland /usr/bin/highland
COPY --from=build /src/target/release/highland-daemon /usr/bin/highland-daemon
EXPOSE 9900
ENTRYPOINT ["/usr/bin/highland"]
CMD ["run", "--config", "/etc/highland/config.toml"]
