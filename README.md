# launchpad

Small Windows tray app that replaces the normal startup list. It starts your apps after a delay, after another app, or once the CPU calms down. It also keeps their own startup entries switched off.

## Install

Download `launchpad.exe` from the [latest release](https://github.com/watdafawx/launchpad/releases/latest) and double-click it. It asks whether to install, then starts in the tray. No admin rights needed. (`launchpad.exe --install` does the same without the question.)

To build it yourself instead: `cargo build --release`, then run `target\release\launchpad.exe --install`.

`--install` installs for your user only:
- copies the exe to `%LOCALAPPDATA%\Programs\launchpad\`
- keeps config and logs in `%LOCALAPPDATA%\launchpad\`
- adds a `launchpad` value under `HKCU\Software\Microsoft\Windows\CurrentVersion\Run`, so it starts at login as you, not elevated

If an older version (Program Files + admin logon task) is installed, the installer offers to remove it. That needs admin once. Your old config is copied over.

To remove it, run `launchpad.exe --uninstall`. That removes the Run entry and turns the managed startup entries back on.

## Use

Right-click the tray icon and pick **Settings…**. In the **Windows startup items** list, click **Manage** next to an app. Then set its rules and Save. Each app has these rules:

| Field | Meaning |
|---|---|
| After | Wait until this other app has been started (or skipped) |
| Delay | Seconds to wait, counted from "After" (or from login) |
| CPU below | Wait until total CPU stays under this % for 3 s (gives up after 90 s) |
| Admin | Start elevated. Windows shows a UAC prompt for it at each login. Everything else starts as your normal user. |
| Process | Exe name used to skip the app when it's already running. Leave empty to auto-detect. |

The log of each run is in `%LOCALAPPDATA%\launchpad\last-run.log`. You can also open it from the tray menu or read it at the bottom of Settings.

## MCP server (for AI assistants)

`launchpad.exe --mcp` runs a Model Context Protocol server over stdio. Assistants can read the setup, startup items and last-run log. They can also file **suggestions**, which show up in Settings for you to Apply or Dismiss. The server never edits the config, so nothing changes until you approve it and Save.

Claude Code:

```
claude mcp add --scope user launchpad -- "%LOCALAPPDATA%\Programs\launchpad\launchpad.exe" --mcp
```

Tools: `get_setup`, `list_startup_items`, `get_last_run_log`, `list_suggestions`, `suggest_changes`, `withdraw_suggestion`. Suggestions are stored in `%LOCALAPPDATA%\launchpad\suggestions.json`.

## Updates

launchpad checks GitHub for a new release a minute after it starts, then every 12 hours. It downloads `launchpad.exe`, checks it against the release's `SHA256SUMS.txt`, swaps it in and restarts in the tray. The restart doesn't run your startup list again. To install updates yourself instead, turn off auto-update in Settings > Options and use the tray menu's **Check for updates**. Each check is logged to `%LOCALAPPDATA%\launchpad\update.log`.

To cut a release, bump `version` in `Cargo.toml`, commit, and push a matching tag (`v0.2.0`). The Release workflow builds the exe and publishes it.

## Notes

- The tray process uses about 2–3 MB of private memory. The Settings window is a separate process that only runs while it's open.
- Commands can be a full command line, a `.lnk` path or a `shell:AppsFolder\…` URI. `%VARS%` are expanded.
- launchpad never runs elevated itself. Apps marked Admin start through a normal UAC prompt, so nothing gets admin rights without you approving it.
- Turning off an all-users startup entry (HKLM or the common Startup folder) needs admin, so Settings asks once when you save.
