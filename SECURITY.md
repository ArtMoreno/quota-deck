# Security policy

## Reporting a vulnerability

Report privately through
[GitHub Security Advisories](https://github.com/ArtMoreno/quota-deck/security/advisories/new).
Please do not open a public issue for a vulnerability. Expect a first response
within 7 days.

## What this plugin touches

Useful context when judging impact:

- **Reads** `~/.claude/.credentials.json`, `~/.grok/auth.json`, and
  `~/.hermes/auth.json` (login keys only), an OpenRouter key supplied by the
  user, Claude Code and Agy statusLine JSON on stdin, and the local
  `codex app-server` JSON-RPC socket.
- **Writes** sanitized percentages and domain-separated hashes of account and
  session identifiers to Herdr's plugin state directory,
  `~/.config/herdr/config.toml`, `~/.claude/settings.json`, and
  `~/.gemini/antigravity-cli/settings.json`. Active-turn coordination locks, a
  configurable poll interval, and a temporary stop marker are also kept in
  that plugin state directory. Older plugin-owned Grok hook files may be
  removed during migration; user-owned hook content is never replaced.
- **Optionally writes** only an OpenRouter key explicitly passed to
  `install.ps1 -OpenRouterKey`, in Herdr's per-user plugin config directory.
  Full uninstall removes that installer-owned file. Environment and Hermes
  `.env` credentials are never copied.
- **Sends** authenticated usage requests to Anthropic's Claude OAuth usage
  endpoint, the Grok CLI billing endpoint, Hermes' Nous Portal endpoint, and
  OpenRouter. No usage data is uploaded anywhere.
- **Never** refreshes or rotates a provider credential, and never reads browser
  cookies or system keychains.

Credentials are held in memory for the duration of a single request and are
never written to the cache or logged. Statusline observations are reduced to the
typed fields QuotaDeck displays; prompt text and full provider payloads are not
written to QuotaDeck's cache. Topic is opt-in; when enabled, a truncated visible
prompt is sent to local Herdr metadata with a 24-hour TTL. Legacy cache files are
sanitized during migration.

For byte-exact statusLine restoration, QuotaDeck keeps one full backup beside
the source settings file, under the same per-user directory protection. Plugin
state keeps only the required statusLine fragment and a SHA-256 digest of the
installed settings, not a second complete copy. Unix private state files use
mode `0600`.

## Supported versions

The latest release on `main`.

This is a fork. Report anything in the upstream code to
[levi-qiao/herdr-agent-quota](https://github.com/levi-qiao/herdr-agent-quota);
report anything in the Windows port, the Hermes or OpenRouter collectors,
or the brand-mark layer here, since upstream does not carry that code.
