# CLI

OpenUsage installs a Rust-based `openusage` command when the desktop app starts.

The command is installed at:

```sh
~/.local/bin/openusage
```

Make sure `~/.local/bin` is in your shell `PATH` if the command is not found.

## Commands

### `openusage usage`

Refreshes all enabled providers, saves successful results to the same cache used by the desktop app, then prints the current cached usage as JSON.

Failed provider refreshes do not overwrite the last successful cached snapshot.

### `openusage refresh`

Alias for `openusage usage`.

### `openusage usage --no-refresh`

Prints the current cached JSON without refreshing providers first.

## Output

The output is the same JSON array as `GET /v1/usage` from the local HTTP API.

Only enabled providers are included, ordered by the app's provider settings.
