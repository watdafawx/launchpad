# launchpad

Small Windows tray app that replaces the normal startup list. It starts your apps after a delay, after another app, or once the CPU calms down. It also keeps their own startup entries switched off.

## Install

Download `launchpad.exe` from the [latest release](https://github.com/watdafawx/launchpad/releases/latest) and run:

```
launchpad.exe --install
```

To build it yourself instead: `cargo build --release`, then run `target\release\launchpad.exe --install`.

`--install` asks for admin once, then:
- copies the exe to `C:\Program Files\launchpad\`
- creates `C:\ProgramData\launchpad\` (admins can write, users can only read)
- registers a logon task with highest privileges, so you get no UAC prompt at login

To remove it, run `launchpad.exe --uninstall`. That deletes the task and turns the managed startup entries back on.

## Use

Right-click the tray icon and pick **Settings…**. In the **Windows startup items** list, click **Manage** next to an app. Then set its rules and Save. Each app has these rules:

| Field | Meaning |
|---|---|
| After | Wait until this other app has been started (or skipped) |
| Delay | Seconds to wait, counted from "After" (or from login) |
| CPU below | Wait until total CPU stays under this % for 3 s (gives up after 90 s) |
| Admin | Start elevated. Everything else starts as your normal user, not elevated. |
| Process | Exe name used to skip the app when it's already running. Leave empty to auto-detect. |

The log of each run is in `C:\ProgramData\launchpad\last-run.log`. You can also open it from the tray menu or read it at the bottom of Settings.

## MCP server (for AI assistants)

`launchpad.exe --mcp` runs a Model Context Protocol server over stdio. Assistants can read the setup, startup items and last-run log. They can also file **suggestions**, which show up in Settings for you to Apply or Dismiss. The server runs without admin rights and can't edit the config, so nothing changes until you approve it and Save.

Claude Code:

```
claude mcp add --scope user launchpad -- "C:\Program Files\launchpad\launchpad.exe" --mcp
```

Tools: `get_setup`, `list_startup_items`, `get_last_run_log`, `list_suggestions`, `suggest_changes`, `withdraw_suggestion`. Suggestions are stored in `%LOCALAPPDATA%\launchpad\suggestions.json`.

## Updates

launchpad checks GitHub for a new release a minute after it starts, then every 12 hours. It downloads `launchpad.exe`, checks it against the release's `SHA256SUMS.txt`, swaps it in and restarts in the tray. The restart doesn't run your startup list again. To install updates yourself instead, turn off auto-update in Settings > Options and use the tray menu's **Check for updates**. Each check is logged to `C:\ProgramData\launchpad\update.log`.

To cut a release, bump `version` in `Cargo.toml`, commit, and push a matching tag (`v0.2.0`). The Release workflow builds the exe and publishes it.

## Notes

- The tray process uses about 2–3 MB of private memory. The Settings window is a separate process that only runs while it's open.
- Commands can be a full command line, a `.lnk` path or a `shell:AppsFolder\…` URI. `%VARS%` are expanded.
- The config can only be edited by admins. Otherwise any program could add a command for launchpad to run with admin rights.
