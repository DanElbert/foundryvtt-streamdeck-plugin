# foundryvtt-streamdeck-plugin

An [OpenAction](https://openaction.amankhanna.me/) plugin for stream controller devices, built with
the `openaction` Rust crate. Runs under [OpenDeck](https://github.com/nekename/OpenDeck) or Tacto.

Two actions: a **Counter** scaffold, and an **Actor Sheet** button that toggles a character sheet
open and closed in Foundry and shows that actor's token artwork.

## Requirements

- Rust (stable)
- OpenDeck
- For the Actor Sheet action: a Foundry world running the
  [REST API module](https://github.com/ThreeHats/foundryvtt-rest-api), paired with a relay, and a
  relay API key

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

## The Actor Sheet action

Press to toggle the actor's character sheet open or closed in Foundry. The key shows the actor's
token art, with a coloured border while the sheet is open and a grey border when Foundry can't be
reached.

### Foundry setup, both required

In **Configure Settings → Module Settings → REST API**:

| Setting | Must be | Why |
|---|---|---|
| Allow Execute JavaScript | **on** | There is no sheet open/close endpoint; this is the only route. |
| Notify on Execute JS | **off** | Otherwise every press and every poll whispers the GM in chat. |

While *Notify on Execute JS* is left on, the plugin **disables its own state polling** rather than
flood your chat log, and says so in its log. The button still works — it just won't notice sheets you
open or close by hand in Foundry until you fix the setting.

Note that enabling *Allow Execute JavaScript* lets anyone holding a valid relay key for that world
run arbitrary JavaScript in your GM browser session. Use a dedicated key; revoking it is the kill
switch.

### Plugin setup

Open the button's property inspector and expand **Relay connection** (shared by every button):
relay URL, client ID and API key. Then pick an actor from the dropdown, or paste an
`Actor.xxxxxxxx` UUID directly if the list is unavailable.

**Artwork** chooses between the token image, the portrait, or whatever the game system prefers.
Token is the default.

### State sync

The button knows a sheet's state immediately after *it* toggles one. For sheets you open or close
directly in Foundry, it polls every 5 seconds (configurable; `0` turns polling off). One batched
request covers every visible button, so the cost doesn't grow with the number of buttons.

Push updates aren't possible today — the REST module's forwarded-hook list doesn't include sheet
render/close events. A small companion module could fix that later.

## Logs

OpenDeck captures the plugin's stdout and stderr to:

```
~/.local/share/opendeck/logs/plugins/us.elbert.foundryvtt.log
```
