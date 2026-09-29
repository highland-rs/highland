#!/usr/bin/env bash
# Publishes the workspace to crates.io in dependency order.
#
# crates.io requires a crate's dependencies to exist before it will accept the
# crate, and the nine packages are not a stack but a DAG: highland-observe
# depends on nothing, while highland-daemon depends on six of the other eight.
# The order below is therefore not the directory order and not alphabetical.
#
# It is the topological order of the graph in the root Cargo.toml's
# [workspace.dependencies], verified against every crate's own manifest:
#
#   highland-core     -> nothing
#   highland-vrrp     -> nothing
#   highland-observe  -> nothing
#   highland-control  -> nothing
#   highland-config   -> highland-vrrp
#   highland-checks   -> highland-core
#   highland-net      -> highland-checks, highland-vrrp
#   highland-daemon   -> highland-checks, highland-config, highland-control,
#                        highland-core, highland-net, highland-observe, highland-vrrp
#   highland-cli      -> highland-config, highland-control
#
# To re-check the order after adding a crate, run `./publish.sh --dry-run`,
# which prints the sequence it would publish without publishing anything.
#
# The delay is not politeness. crates.io propagates a new release to the index
# asynchronously, and publishing a dependent too quickly fails with a
# "no matching package" error that looks like a broken version requirement and
# is not one. Five seconds is not a guarantee; if a publish does fail on a
# missing dependency, wait and re-run that one crate rather than raising a
# version.

set -euo pipefail

crates=(
	highland-core
	highland-vrrp
	highland-observe
	highland-control
	highland-config
	highland-checks
	highland-net
	highland-daemon
	highland-cli
)

dry_run=false
if [[ "${1:-}" == "--dry-run" ]]; then
	dry_run=true
fi

for crate in "${crates[@]}"; do
	if $dry_run; then
		printf 'would publish %s\n' "$crate"
		continue
	fi
	printf 'publishing %s\n' "$crate"
	cargo publish -p "$crate"
	sleep 5
done
