# foundryvtt-streamdeck-plugin

An [OpenAction](https://openaction.amankhanna.me/) plugin for stream controller devices, built with
the `openaction` Rust crate. Runs under [OpenDeck](https://github.com/nekename/OpenDeck) or Tacto.

This is a scaffold. It ships a working Counter action and a minimal property inspector to establish
the build loop; **no Foundry VTT integration exists yet.**

## Requirements

- Rust (stable)
- OpenDeck

## Build and install

```sh
./build.sh
```

That produces `dist/us.elbert.foundryvtt.sdPlugin/`. Copy that directory into OpenDeck's plugins
folder and restart OpenDeck:

| Install | Path |
|---|---|
| Native | `~/.config/opendeck/plugins/` |
| Flatpak | `~/.var/app/me.amankhanna.opendeck/config/opendeck/plugins/` |

Copy the `.sdPlugin` directory itself, so that `manifest.json` ends up directly inside
`plugins/us.elbert.foundryvtt.sdPlugin/`. If the manifest is nested any deeper, OpenDeck will not
find the plugin and will not report an error.

Only `x86_64-unknown-linux-gnu` is built.

## The Counter action

Pressing the key increments a stored counter and writes the new value to the key's title.

The property inspector has two fields:

- **Label** — optional text shown above the count. With a label set, the title is two lines
  (`label` then the count); with it empty, just the count. Editing it updates the key immediately
  without resetting the count.
- **Step** — how much each press adds. Defaults to 1; may be negative.

Both are per-key settings, so the same action can appear on several keys with different values.

## Logs

OpenDeck captures the plugin's stdout and stderr to:

```
~/.local/share/opendeck/logs/plugins/us.elbert.foundryvtt.log
```
