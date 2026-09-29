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
# To re-check the order without publishing, run `./publish.sh --dry-run`.

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

# How long to wait for a newly published crate to appear in the registry index.
index_timeout="${HIGHLAND_INDEX_TIMEOUT:-180}"

dry_run=false
if [[ "${1:-}" == "--dry-run" ]]; then
	dry_run=true
fi

# Every crate is published at the workspace version, read once.
workspace_version() {
	cargo metadata --no-deps --format-version 1 |
		python3 -c "
import json, sys
name = sys.argv[1]
meta = json.load(sys.stdin)
print(next(p['version'] for p in meta['packages'] if p['name'] == name))
" "$1"
}

# Returns success if `$1` is on crates.io at `$2`.
#
# --fail is load-bearing. Without it curl exits 0 on a 404, so every crate reads
# as published and the script skips the entire workspace while claiming to have
# published it.
is_published() {
	curl -sSf --max-time 20 "https://crates.io/api/v1/crates/$1/$2" \
		-H 'User-Agent: highland-publish-check' >/dev/null 2>&1
}

# Blocks until `$1` at `$2` is visible in the index.
#
# A fixed sleep is a guess, and guessing wrong is expensive: publishing
# highland-checks seconds after highland-core failed on "no matching package",
# which reads like a broken version requirement and is not one. The failure mode
# invites the wrong response, which is to bump a version and publish again.
#
# crates.io propagation is usually a few seconds, so a wait that normally
# returns immediately costs nothing and removes the whole class of error.
wait_for_index() {
	local name="$1" version="$2" deadline
	deadline=$((SECONDS + index_timeout))

	printf 'waiting for %s %s to reach the index\n' "$name" "$version"
	while ! is_published "$name"; do
		if ((SECONDS >= deadline)); then
			printf 'timed out after %ds waiting for %s %s\n' \
				"$index_timeout" "$name" "$version" >&2
			return 1
		fi
		sleep 3
	done
	printf '%s %s is in the index\n' "$name" "$version"
}

for crate in "${crates[@]}"; do
	# Skipping what is already published is what makes a partial run resumable.
	# A first attempt can stop at any point -- a missing dependency, a yanked
	# release, an interrupted terminal -- and the fix is to re-run, not to
	# re-plan.
	version="$(workspace_version "$crate")"
	if is_published "$crate" "$version"; then
		printf 'skipping %s %s, already published\n' "$crate" "$version"
		continue
	fi

	if $dry_run; then
		printf 'would publish %s %s\n' "$crate" "$version"
		continue
	fi

	printf 'publishing %s %s\n' "$crate" "$version"
	cargo publish -p "$crate"
	wait_for_index "$crate" "$version"
done

printf 'done\n'
