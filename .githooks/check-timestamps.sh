#!/usr/bin/env bash
# HOOK_VERSION=4
# Timestamp gate (standing rule 52): a time shown in a user interface is
# written for a person, so an ISO-8601 string never reaches a rendered page.
#
# Kenny, 2026-09-10, at a live dashboard showing `2026-09-10T02:16:47Z`:
# "niet bepaald iets dat een mens wilt lezen". Decided as a rule with a gate
# on 2026-09-26.
#
# What this can see, and what it cannot. It reads only the lines a commit
# ADDS to user-interface files, and refuses the shapes that are ISO for
# certain: a literal ISO date-time, and an ISO producer (`toISOString()`,
# `to_rfc3339`, `isoformat()`, a `|iso` filter, a `%Y-%m-%dT` format) inside
# something that renders. A field merely NAMED `published_at` is not
# refused: kyu renders such fields after turning them into "3 min ago" in
# Rust, which a grep cannot tell apart from a raw value. That half is each
# project's render test, the way chassis-rs asserts two locales.
#
# The machine-readable value belongs in a `datetime="…"` attribute, which is
# exempt. A line that must carry an ISO value for a machine (a JSON example
# in a template, a data attribute read by script) says so with the marker
# `timestamp-ok` anywhere on the line.
#
# Depends on bash, git and grep only, like the rest of .githooks.
set -u

ui_files='\.(html|htm|jinja|jinja2|j2|tera|hbs|askama|tsx|jsx|vue|svelte|axaml|xaml)$'

iso_literal='[0-9]{4}-[0-9]{2}-[0-9]{2}T[0-9]{2}:[0-9]{2}'
iso_producer='toISOString\(\)|to_rfc3339|isoformat\(\)|\|[[:space:]]*iso\b|%Y-%m-%dT'

mapfile -t files < <(git diff --cached --name-only --diff-filter=AM 2>/dev/null | grep -E "$ui_files" || true)
[ "${#files[@]}" -eq 0 ] && exit 0

found=""
for f in "${files[@]}"; do
  hits="$(git diff --cached -U0 -- "$f" 2>/dev/null \
    | grep -E '^\+' | grep -vE '^\+\+\+' | sed 's/^+//' \
    | grep -v 'timestamp-ok' \
    | sed -E 's/datetime="[^"]*"//g' \
    | grep -E "$iso_literal|$iso_producer" || true)"
  [ -n "$hits" ] && found="$found$(printf '  %s:\n' "$f")$(printf '%s\n' "$hits" | sed 's/^/      /')"$'\n'
done

if [ -n "$found" ]; then
  {
    echo "COMMIT BLOCKED — an ISO-8601 time would reach a rendered page (standing rule 52)."
    printf '%s' "$found"
    echo "Render it for a person (the viewer's locale, date and time as readable parts),"
    echo "and keep the ISO value only in a datetime=\"…\" attribute."
    echo "A line that carries it for a machine on purpose can say so with: timestamp-ok"
  } >&2
  exit 1
fi
exit 0
