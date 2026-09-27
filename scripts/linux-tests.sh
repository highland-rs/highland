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

# `--privileged` is required, not for the capabilities but for the *namespaces*:
# `ip netns add` and `unshare --net` need CAP_SYS_ADMIN, and the two-node suite
# builds one namespace per node. `--cap-add` is listed as well so the intent
# survives a later restriction.
docker run --rm --privileged --cap-add=NET_ADMIN --cap-add=NET_RAW \
	--volume "$ROOT":/src:ro \
	--volume "$CARGO_CACHE":/usr/local/cargo \
	--volume "$TARGET_VOLUME":/target \
	--workdir /src \
	--env CARGO_TARGET_DIR=/target \
	"$IMAGE" \
	sh -c "
		set -eu
		# The namespace tests build the topology with iproute2, and the rust
		# image is slim, so it has to be installed. A Debian image with the
		# toolchain already present can override IMAGE.
		if ! command -v ip >/dev/null 2>&1; then
			apt-get update -qq
			# curl scrapes the metrics endpoint from inside a namespace, since
			# a namespace has its own loopback and its own routes.
			apt-get install -y -qq --no-install-recommends iproute2 curl
		fi
		cargo fmt --all --check
		cargo clippy --workspace --all-targets --all-features -- -D warnings
		cargo test --workspace
		RUSTDOCFLAGS='-D warnings' cargo doc --workspace --no-deps
		$( [ "$run_netlink" = 1 ] && echo "cargo test -p highland-net --features netlink-tests" )
		# The daemon suites that build namespaces run one test at a time: they
		# each create a bridge and a pair of namespaces, and their assertions are
		# about elapsed time, so running several at once would have them
		# competing for the same CPU and failing for the wrong reason.
		$( [ "$run_netlink" = 1 ] && echo "cargo test -p highland-daemon --features netlink-tests -- --test-threads=1" )
		$( [ "$run_netlink" = 1 ] && echo "cargo test -p highland-control" )
	"

# The source is mounted read-only, so a `cargo fmt` fix has to happen on the
# host. The Netlink tests create dummy interfaces and do not touch the tree.
echo "linux: everything above passed in $IMAGE"
