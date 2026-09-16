#!/usr/bin/env bash
# HOOK_VERSION=4
# Run a project's gate checks, skipping the ones whose inputs did not move.
#
# Kenny, 2026-09-16: "indien mogelijk zouden we van verschillende
# onderdelen bv de hash bijhouden, en enkel als die hash verandert dan
# testen we daarop." Measured before it was built: across sixteen projects
# 584 of the last 1418 commits (41%) touched only documentation or
# configuration and ran the full suite anyway. In kp-themes, finer: 200
# commits x 30 checks is 6000 runs, of which 4239 (70%) could not have
# found anything — 8.79 s of gate time per commit falling to 4.19 s.
#
# Sourced by a project's gates.sh:
#
#     . "$(git rev-parse --show-toplevel)/.githooks/gate-cache.sh"
#     gate fonts node gates/check-fonts.mjs
#     gate types npx tsc --noEmit -p jsconfig.json
#     gate_cache_done
#
# Three things are not negotiable and are not configurable:
#
#   * Only a GREEN run is ever remembered. A failing check leaves no
#     cache entry, so it runs again next time.
#   * The check's own source is part of its input set, so editing a
#     check always re-runs it.
#   * The cache lives inside .git and never travels. A fresh clone runs
#     everything, and no machine can inherit another machine's verdict.
#
# The safety net Kenny chose (cache-safety, 2026-09-16): the cache is
# ignored entirely on the first commit of each day and whenever
# GATE_FULL=1 is set, which every release script does. That bounds the
# damage of an incomplete input set to one working day instead of one
# release cycle.
#
# What can and cannot be cached:
#
#   `gate`      a node command, run under trace-inputs.cjs, which records
#               every path it opens. Anything else — a shell script, a
#               python script, a compiler — has no input set to record on
#               this machine (strace is not installed) and therefore
#               ALWAYS RUNS. An unknown input set means no skipping, never
#               a guess.
#   `gate_glob` a command whose inputs are named by git path globs rather
#               than recorded. For the case Kenny decided separately
#               (rust-suite, 2026-09-16): the Rust suite depends on the
#               Rust sources, which is one rule for one language rather
#               than a hand-kept list per check, and `git ls-files`
#               produces the file list so nothing goes stale.
#
# One limit is worth naming because it is invisible: a check whose verdict
# depends on something that is not a file — an environment variable, the
# clock, the network — cannot be cached honestly, because nothing about it
# moves between two commits. Such a check belongs in a `gate_glob` with no
# globs, or outside the runner entirely. Found while writing the drills
# for this file: a drill that made a check fail through the environment
# was skipped on the very next run, which is correct behaviour and a
# useless drill.
# The caller's shell options are left exactly as they were. `gate` and
# `gate_glob` return a status on purpose, and several of their own steps
# return non-zero in normal operation, so each one turns `errexit` off
# for the length of its body and puts it back. A project that had
# `set -e` therefore keeps it: a failing gate aborts the chain as a bare
# command, and one that uses `|| exit 1` works the same way. Without this
# guard every project would have had to change its `set` line, which is
# fourteen files of risk for no benefit.
gate_root="$(git rev-parse --show-toplevel)"
gate_gitdir="$(git rev-parse --absolute-git-dir)"
gate_cachedir="$gate_gitdir/gate-cache"
gate_tracer="$gate_root/.githooks/trace-inputs.cjs"
mkdir -p "$gate_cachedir"

gate_ran=0
gate_skipped=0
gate_untraceable=0
gate_full="${GATE_FULL:-0}"
gate_full_reason=""

# The first commit of a day ignores the cache. The stamp is written up
# front, so a run that fails halfway does not hand the next commit a
# second full round.
gate_today="$(date +%F)"
if [ "$gate_full" != 1 ]; then
  if [ "$(cat "$gate_cachedir/.day" 2>/dev/null || true)" != "$gate_today" ]; then
    gate_full=1
    gate_full_reason="eerste commit van vandaag"
  fi
fi
[ "$gate_full" = 1 ] && [ -z "$gate_full_reason" ] && gate_full_reason="GATE_FULL=1"
printf '%s' "$gate_today" > "$gate_cachedir/.day"

# Hash an input set. Files go through one sha256sum call rather than one
# per file — the type check's set is 1290 entries and a subprocess each
# would cost more than the check it is trying to avoid. A directory is
# hashed by its listing, so a file APPEARING in a directory a check walks
# moves the hash even though no file the check read has changed. A path
# that has since disappeared is recorded as absent, which is also a
# change.
gate_hash() {
  local inputs="$1"
  {
    xargs -d '\n' -a "$inputs" -r sha256sum -- 2>/dev/null
    while IFS= read -r p; do
      if [ -d "$gate_root/$p" ]; then
        printf 'D %s:' "$p"
        ls -A -- "$gate_root/$p" 2>/dev/null | sort | tr '\n' ','
        printf '\n'
      elif [ ! -f "$gate_root/$p" ]; then
        printf 'X %s\n' "$p"
      fi
    done < "$inputs"
  } | sha256sum | cut -d' ' -f1
}

# Whether this command is one whose reads can be recorded.
# Put `errexit` back the way the caller had it, then hand on the status.
_gate_return() { [ "$1" = 1 ] && set -e; return "$2"; }

gate_traceable() {
  case "$1" in
    node|npx) [ -f "$gate_tracer" ] ;;
    *) return 1 ;;
  esac
}

gate() {
  local name="$1"; shift
  local had_e=0; case $- in *e*) had_e=1; set +e;; esac
  local inputs="$gate_cachedir/$name.inputs"
  local stamp="$gate_cachedir/$name.hash"
  local before=""

  if ! gate_traceable "$1"; then
    gate_untraceable=$((gate_untraceable + 1))
    gate_ran=$((gate_ran + 1))
    ( cd "$gate_root" && "$@" )
    _gate_return "$had_e" $?
    return
  fi

  if [ "$gate_full" != 1 ] && [ -s "$inputs" ] && [ -s "$stamp" ]; then
    before="$(gate_hash "$inputs")"
    if [ "$before" = "$(cat "$stamp")" ]; then
      gate_skipped=$((gate_skipped + 1))
      _gate_return "$had_e" 0
      return
    fi
  fi

  local trace
  trace="$(mktemp "$gate_cachedir/.trace.XXXXXX")"
  gate_ran=$((gate_ran + 1))
  if ( cd "$gate_root" \
       && GATE_TRACE_OUT="$trace" GATE_TRACE_ROOT="$gate_root" \
          NODE_OPTIONS="${NODE_OPTIONS:-} --require $gate_tracer" "$@" ); then
    # Only a green run is remembered, and only if it actually recorded
    # something. An empty trace means the tracer did not load, and caching
    # on an empty input set would skip this check forever after.
    if [ -s "$trace" ]; then
      mv "$trace" "$inputs"
      gate_hash "$inputs" > "$stamp"
    else
      rm -f "$trace" "$inputs" "$stamp"
    fi
    _gate_return "$had_e" 0
    return
  fi
  # A red run leaves no verdict behind, so the next commit runs it again
  # even if nothing changed in between.
  rm -f "$trace" "$stamp"
  _gate_return "$had_e" 1
}

# A command whose inputs are named rather than recorded. The set is
# whatever `git ls-files` returns for the globs given before `--`, so it
# tracks files being added and removed without anyone editing a list.
gate_glob() {
  local name="$1"; shift
  local had_e=0; case $- in *e*) had_e=1; set +e;; esac
  local globs=()
  while [ "$#" -gt 0 ] && [ "$1" != "--" ]; do globs+=("$1"); shift; done
  [ "${1:-}" = "--" ] && shift
  local inputs="$gate_cachedir/$name.inputs"
  local stamp="$gate_cachedir/$name.hash"

  git -C "$gate_root" ls-files -- "${globs[@]}" > "$inputs" 2>/dev/null

  if [ "$gate_full" != 1 ] && [ -s "$inputs" ] && [ -s "$stamp" ]      && [ "$(gate_hash "$inputs")" = "$(cat "$stamp")" ]; then
    gate_skipped=$((gate_skipped + 1))
    _gate_return "$had_e" 0
    return
  fi

  gate_ran=$((gate_ran + 1))
  if ( cd "$gate_root" && "$@" ); then
    gate_hash "$inputs" > "$stamp"
    _gate_return "$had_e" 0
    return
  fi
  rm -f "$stamp"
  _gate_return "$had_e" 1
}

gate_cache_done() {
  local total=$((gate_ran + gate_skipped))
  if [ "$gate_full" = 1 ]; then
    printf 'gate-cache: volledige ronde (%s) — %d checks gedraaid\n' \
      "$gate_full_reason" "$gate_ran"
  else
    printf 'gate-cache: %d van %d checks gedraaid, %d overgeslagen omdat hun invoer niet bewoog' \
      "$gate_ran" "$total" "$gate_skipped"
    [ "$gate_untraceable" -gt 0 ] && printf ' (%d daarvan draaien altijd: hun invoer is hier niet op te nemen)' "$gate_untraceable"
    printf '\n'
  fi
}
