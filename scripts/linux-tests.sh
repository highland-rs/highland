#!/bin/sh
# Runs the checks that need a real Linux kernel, in a container.
#
# `highland-net`'s Netlink backend is `cfg(target_os = "linux")`, so on a Mac
# it is never compiled, let alone executed. This script is how that code gets
# checked: it builds the workspace for Linux, runs the whole suite, and runs the
# Netlink tests against a real kernel with CAP_NET_ADMIN.
#
# Usage:
#   scripts/linux-tests.sh            # build, clippy, test, and the Netlink tests
#   scripts/linux-tests.sh --no-net   # everything except the privileged tests
set -eu

IMAGE="${HIGHLAND_TEST_IMAGE:-rust:1-slim}"
SOURCE="${HIGHLAND_TEST_SOURCE:-rust:slim}"
ROOT="$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)"
CARGO_CACHE="${HIGHLAND_CARGO_CACHE:-highland-cargo}"
TARGET_VOLUME="${HIGHLAND_TARGET_VOLUME:-highland-target}"

run_netlink=1
[ "${1:-}" = "--no-net" ] && run_netlink=0

# `--privileged` gives CAP_NET_ADMIN and the namespace ability the tests need.
# `--cap-add` is listed as well so the intent survives a `--cap-drop`-style
# change later.
docker run --rm --privileged --cap-add=NET_ADMIN --cap-add=NET_RAW \
	--volume "$ROOT":/src:ro \
	--volume "$CARGO_CACHE":/usr/local/cargo \
	--volume "$TARGET_VOLUME":/target \
	--workdir /src \
	--env CARGO_TARGET_DIR=/target \
	"$IMAGE" \
	sh -c "
		set -eu
		cargo fmt --all --check
		cargo clippy --workspace --all-targets --all-features -- -D warnings
		cargo test --workspace
		RUSTDOCFLAGS='-D warnings' cargo doc --workspace --no-deps
		$( [ "$run_netlink" = 1 ] && echo "cargo test -p highland-net --features netlink-tests" )
	"

# The source is mounted read-only, so a `cargo fmt` fix has to happen on the
# host. The Netlink tests create dummy interfaces and do not touch the tree.
echo "linux: everything above passed in $IMAGE"
