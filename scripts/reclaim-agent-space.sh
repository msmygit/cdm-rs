#!/usr/bin/env bash
#
# Reclaim disk space taken by coding-agent git worktrees under `.claude/worktrees/`.
#
# Agents (Claude Code and friends) run in throwaway git worktrees. Each one builds its own
# `target/` directory, which for this workspace is 2-12 GiB of compiled dependencies, debug
# symbols and incremental-compilation cache. A dozen finished agents will quietly consume most
# of a disk: this repository has seen 98 GiB accumulate against 1.0 GiB free, at which point
# builds fail with no obvious cause.
#
# Everything in `target/` is regenerable from source by `cargo build`, so removing it costs
# rebuild time and nothing else. This script therefore removes ONLY build directories. It never
# touches source, git history, branches, settings files, or anything under `~/.claude` (which on
# inspection holds session transcripts and totals well under 100 MB — not where the space goes).
#
# Usage:
#   scripts/reclaim-agent-space.sh                 # dry run: report only, change nothing
#   scripts/reclaim-agent-space.sh --apply         # remove build dirs that are safe to remove
#   scripts/reclaim-agent-space.sh --apply --all   # also include worktrees with open/unmerged work
#   scripts/reclaim-agent-space.sh --prune         # additionally remove worktrees fully merged and clean
#   scripts/reclaim-agent-space.sh --idle-minutes 30
#
# Exit status is 0 on success, non-zero on a usage error or if the repository root is not found.

set -euo pipefail

# --- configuration -----------------------------------------------------------------------------

# A worktree whose build directory was written to more recently than this is assumed to have an
# agent actively building in it. Deleting it mid-build does not corrupt anything, but it does
# throw away work in progress and can fail that agent's run, so the default is to leave it alone.
IDLE_MINUTES=20

APPLY=0     # 0 = dry run
ALL=0       # 1 = include worktrees whose branch is unmerged or which have uncommitted changes
PRUNE=0     # 1 = also `git worktree remove` worktrees that are fully merged and clean

# Build directories to reclaim. All are regenerable. Add to this list rather than generalising:
# an over-broad glob here is the one way this script could destroy something that matters.
BUILD_DIRS=(target)

# --- argument parsing --------------------------------------------------------------------------

usage() {
    sed -n '3,28p' "$0" | sed 's/^# \{0,1\}//'
    exit "${1:-0}"
}

while [ $# -gt 0 ]; do
    case "$1" in
        --apply)         APPLY=1 ;;
        --all)           ALL=1 ;;
        --prune)         PRUNE=1 ;;
        --idle-minutes)  shift; IDLE_MINUTES="${1:?--idle-minutes needs a value}" ;;
        -h|--help)       usage 0 ;;
        *)               printf 'unknown argument: %s\n\n' "$1" >&2; usage 2 ;;
    esac
    shift
done

# --- helpers ---------------------------------------------------------------------------------

repo_root() {
    git rev-parse --show-toplevel 2>/dev/null || {
        printf 'not inside a git repository\n' >&2
        exit 1
    }
}

# Human-readable size of a path, or "-" when it does not exist.
size_of() {
    [ -e "$1" ] || { printf -- '-'; return; }
    du -sh "$1" 2>/dev/null | cut -f1
}

# Size in kibibytes, for arithmetic. 0 when the path does not exist.
size_kb() {
    [ -e "$1" ] || { printf '0'; return; }
    du -sk "$1" 2>/dev/null | cut -f1
}

free_space() {
    df -h / | awk 'NR==2 {print $4}'
}

# True when anything inside the path was modified within IDLE_MINUTES — i.e. something is
# probably building in it right now.
#
# Depth matters here. `target/` itself is barely written to during a build; the churn is in
# `target/debug/**`. Testing only the top directory reports a live build as idle, which is how an
# earlier version of this script would have deleted the build directory of a running agent. Two
# levels is enough to see `debug/incremental`, `debug/deps` and friends, and is cheap: `find`
# stops as soon as the first match prints.
recently_touched() {
    [ -e "$1" ] || return 1
    [ -n "$(find "$1" -maxdepth 2 -mmin "-${IDLE_MINUTES}" -print -quit 2>/dev/null)" ]
}

# --- main --------------------------------------------------------------------------------------

ROOT="$(repo_root)"
WORKTREES="${ROOT}/.claude/worktrees"

if [ ! -d "$WORKTREES" ]; then
    printf 'no agent worktrees at %s — nothing to do\n' "$WORKTREES"
    exit 0
fi

printf 'Agent worktrees: %s\n' "$WORKTREES"
printf 'Free space now:  %s\n' "$(free_space)"
if [ "$APPLY" -eq 0 ]; then
    printf '\nDRY RUN — nothing will be deleted. Re-run with --apply to act.\n'
fi
printf '\n'

# Make sure "merged?" is answered against the real remote state rather than a stale ref.
git -C "$ROOT" fetch --quiet origin 2>/dev/null || \
    printf 'warning: could not fetch origin; merge status may be stale\n\n' >&2

total_kb=0
reclaimable_kb=0
skipped=0

for worktree in "$WORKTREES"/*/; do
    [ -d "$worktree" ] || continue
    name="$(basename "$worktree")"

    # A worktree whose gitdir has gone is a leftover directory, not a worktree. Report it and
    # leave it: deciding what it is belongs to a human, not to this script.
    if ! head="$(git -C "$worktree" rev-parse --short HEAD 2>/dev/null)"; then
        printf '  %-34s  NOT A GIT WORKTREE — skipping, inspect by hand\n' "$name"
        skipped=$((skipped + 1))
        continue
    fi

    branch="$(git -C "$worktree" rev-parse --abbrev-ref HEAD 2>/dev/null || printf '?')"
    dirty="$(git -C "$worktree" status --porcelain 2>/dev/null | wc -l | tr -d ' ')"

    if git -C "$ROOT" merge-base --is-ancestor "$head" origin/main 2>/dev/null; then
        merged="merged"
    else
        merged="unmerged"
    fi

    # Sum this worktree's build directories.
    wt_kb=0
    dirs_present=()
    for d in "${BUILD_DIRS[@]}"; do
        if [ -d "${worktree}${d}" ]; then
            wt_kb=$((wt_kb + $(size_kb "${worktree}${d}")))
            dirs_present+=("${worktree}${d}")
        fi
    done
    total_kb=$((total_kb + wt_kb))

    if [ "${#dirs_present[@]}" -eq 0 ]; then
        printf '  %-34s  %-8s  already clean\n' "$name" "$merged"
        continue
    fi

    human="$(size_of "${worktree}target")"

    # Reasons to leave a build directory alone.
    reason=""
    for d in "${dirs_present[@]}"; do
        if recently_touched "$d"; then
            reason="active: written to in the last ${IDLE_MINUTES}m"
            break
        fi
    done
    if [ -z "$reason" ] && [ "$ALL" -eq 0 ]; then
        if [ "$merged" = "unmerged" ]; then
            reason="unmerged branch (${branch}) — use --all to include"
        elif [ "$dirty" -ne 0 ]; then
            reason="${dirty} uncommitted change(s) — use --all to include"
        fi
    fi

    if [ -n "$reason" ]; then
        printf '  %-34s  %-8s  %6s  SKIP: %s\n' "$name" "$merged" "$human" "$reason"
        skipped=$((skipped + 1))
        continue
    fi

    reclaimable_kb=$((reclaimable_kb + wt_kb))

    if [ "$APPLY" -eq 1 ]; then
        for d in "${dirs_present[@]}"; do
            rm -rf "$d"
        done
        printf '  %-34s  %-8s  %6s  REMOVED\n' "$name" "$merged" "$human"

        # Only ever prune a worktree that is both merged and clean. A worktree is cheap (~5 MB of
        # source); the branch and its commits survive removal regardless, but an unmerged or dirty
        # one may hold the only copy of something, so it is never a candidate.
        if [ "$PRUNE" -eq 1 ] && [ "$merged" = "merged" ] && [ "$dirty" -eq 0 ]; then
            if git -C "$ROOT" worktree remove --force "$worktree" 2>/dev/null; then
                printf '  %-34s  %-8s  %6s  worktree pruned\n' "$name" "" ""
            fi
        fi
    else
        printf '  %-34s  %-8s  %6s  would remove\n' "$name" "$merged" "$human"
    fi
done

printf '\n'
printf 'Build directories found:       %s GiB\n' "$(awk -v k="$total_kb" 'BEGIN{printf "%.1f", k/1048576}')"
printf 'Reclaimable by this run:       %s GiB\n' "$(awk -v k="$reclaimable_kb" 'BEGIN{printf "%.1f", k/1048576}')"
printf 'Worktrees skipped:             %s\n' "$skipped"

if [ "$APPLY" -eq 1 ]; then
    git -C "$ROOT" worktree prune 2>/dev/null || true
    printf 'Free space after:              %s\n' "$(free_space)"
else
    printf '\nRe-run with --apply to reclaim.\n'
fi
