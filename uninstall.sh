#!/usr/bin/env bash
# Restore plugin-owned configuration, then unlink herdr-agent-quota-win.
#
# Usage:
#   ./uninstall.sh                    # restore config, then unlink
#   ./uninstall.sh --agent grok       # remove only that agent, stay installed
#
# A full uninstall also drops the saved sidebar-layout, row-gap,
# quota-percent, fields, brand-colors, brand-glyphs, agent-order, and low-quota-alert prefs,
# and hands Herdr's agent panel back its own ordering.
#
# The restore action runs, and is waited for, before unlinking: Herdr owns the
# plugin state directory holding the Claude/Agy statusLine backups, and
# `herdr plugin action invoke` returns before the action has finished.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")" && pwd)"
# shellcheck source=scripts/herdr-action.sh
source "$ROOT/scripts/herdr-action.sh"
HERDR_ACTION_JSON_PARSER="$ROOT/target/release/herdr-agent-quota"
export HERDR_ACTION_JSON_PARSER

AGENTS=""
while (($# > 0)); do
  case "$1" in
    --agent)
      (($# >= 2)) || { printf 'error: missing value for %s\n' "$1" >&2; exit 1; }
      AGENTS="${AGENTS:+$AGENTS,}$2"
      shift 2
      ;;
    -h|--help)
      sed -n '2,10p' "$0"
      exit 0
      ;;
    *)
      printf 'error: unknown argument: %s\n' "$1" >&2
      exit 1
      ;;
  esac
done

IFS=',' read -r -a SELECTED_AGENTS <<<"$AGENTS"
for name in "${SELECTED_AGENTS[@]}"; do
  case "$name" in
    ""|all|claude|codex|grok|agy|opencode|pi|omp|hermes) ;;
    *) printf 'error: unknown agent: %s\n' "$name" >&2; exit 1 ;;
  esac
done
if [[ ",$AGENTS," == *,all,* && "$AGENTS" != all ]]; then
  printf "error: 'all' cannot be combined with another agent\n" >&2
  exit 1
fi

FULL_UNINSTALL=0
if [[ -z "$AGENTS" || ",$AGENTS," == *,all,* ]]; then
  FULL_UNINSTALL=1
fi

die() {
  printf 'error: %s\n' "$*" >&2
  exit 1
}

command -v herdr >/dev/null 2>&1 || die "Herdr is not installed or not on PATH"

# Herdr runs the uninstall action with a fixed command line, in the server's own
# environment, so `env AGENTS=... herdr plugin action invoke` is silently
# ignored — and an ignored selection here means removing everything instead of
# one agent. A partial selection therefore travels through a one-shot file in
# the plugin config directory; full uninstalls remove any stale one first.
UNINSTALL_AGENTS_PREF=""

clear_uninstall_agents_pref() {
  [[ -z "$UNINSTALL_AGENTS_PREF" ]] && return 0
  rm -f "$UNINSTALL_AGENTS_PREF"
}

RESTORE_DISABLED=0
cleanup_uninstall() {
  clear_uninstall_agents_pref
  if ((RESTORE_DISABLED)); then
    herdr plugin disable herdr-agent-quota-win >/dev/null 2>&1 \
      || printf 'warning: could not restore the plugin disabled state\n' >&2
  fi
}

select_agents() {
  local directory
  directory="$(herdr plugin config-dir herdr-agent-quota-win)" \
    || die "cannot resolve plugin config directory"
  mkdir -p "$directory"
  UNINSTALL_AGENTS_PREF="$directory/uninstall-agents"
  trap cleanup_uninstall EXIT
  rm -f "$UNINSTALL_AGENTS_PREF"
  if ((!FULL_UNINSTALL)); then
    printf '%s\n' "$AGENTS" > "$UNINSTALL_AGENTS_PREF"
  fi
}

PLUGIN_LIST="$(herdr plugin list --plugin herdr-agent-quota-win 2>/dev/null)" \
  || die "cannot inspect Herdr plugin state"
if grep -Fq -- '- herdr-agent-quota-win (' <<<"$PLUGIN_LIST"; then
  # An earlier interrupted uninstall may have disabled the plugin. Enable it
  # long enough for Herdr to provide the state directory to the restore action.
  if ! grep -Fq ') enabled [' <<<"$PLUGIN_LIST"; then
    RESTORE_DISABLED=1
    herdr plugin enable herdr-agent-quota-win >/dev/null 2>&1 \
      || die "cannot temporarily enable the plugin for restoration"
  fi
  select_agents
  printf '%s\n' '→ restoring plugin-owned configuration'
  # Waiting matters twice here: the selection file below must stay in place
  # until the action has read it, and unlinking before the restore finishes
  # can strand a statusLine entry pointing at a plugin that is gone.
  invoke_action_and_wait uninstall || die "restore action failed; nothing was unlinked"

  # Removing one agent is not uninstalling the plugin; the rest still need it.
  if ((!FULL_UNINSTALL)); then
    printf '%s\n' "Removed $AGENTS. The plugin stays linked for the other agents."
    exit 0
  fi

  RESTORE_DISABLED=0
  printf '%s\n' '→ disabling and unlinking the Herdr plugin'
  herdr plugin disable herdr-agent-quota-win >/dev/null || true
  unlink_output="$(herdr plugin unlink herdr-agent-quota-win 2>&1)" || die "herdr plugin unlink failed: $unlink_output"
  printf '%s\n' 'Uninstalled and restored.'
else
  printf '%s\n' 'herdr-agent-quota-win is not linked; no configuration was changed.'
fi
