# foundryvtt-streamdeck-plugin

An [OpenAction](https://openaction.amankhanna.me/) plugin for stream controller devices, built with
the `openaction` Rust crate. Runs under [OpenDeck](https://github.com/nekename/OpenDeck) or Tacto.

Three actions: a **Counter** scaffold, an **Actor Sheet** button that toggles a character sheet
open and closed in Foundry and shows that actor's token artwork, and a **Condition** button that
shows and toggles a condition on the selected tokens.

## Requirements

- Rust (stable)
- OpenDeck
- For the Actor Sheet and Condition actions: a Foundry world (dnd5e, for conditions) running the
  [REST API module](https://github.com/ThreeHats/foundryvtt-rest-api), paired with a relay, a
  relay API key, and the
  [Stream Deck Companion module](https://github.com/DanElbert/foundryvtt-streamdeck-module)

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

### Foundry setup

Install and enable the **Stream Deck Companion** module (`foundryvtt-streamdeck`) in the world. Its
manifest URL is:

```
https://raw.githubusercontent.com/DanElbert/foundryvtt-streamdeck-module/main/module.json
```

Then in **Configure Settings → Module Settings → REST API**:

| Setting | Must be | Why |
|---|---|---|
| Allow Execute JavaScript | **on** | There is no sheet open/close endpoint; this is the only route. |
| Notify on Execute JS | **off** | Otherwise every press and artwork fetch whispers the GM in chat. |

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

There is no polling. The companion module pushes an event over the relay whenever an actor sheet
opens or closes in the GM's browser, and the matching buttons update immediately. When the plugin
connects, and whenever the GM's browser reloads, the full set of open sheets is sent so the deck
catches up.

Without the companion module the button still toggles sheets, but it won't notice sheets opened or
closed directly in Foundry. The plugin logs a warning and the property inspector shows
"companion module missing".

## The Condition action

Each key is bound to one D&D 5e condition: blinded, charmed, deafened, frightened, grappled,
incapacitated, invisible, paralyzed, petrified, poisoned, prone, restrained, stunned or unconscious.
Exhaustion isn't offered. Pick the condition in the property inspector; the list comes from
Foundry, so the relay must be connected and the companion module enabled.

The key follows whichever tokens are selected in the GM's browser:

| Key shows | When |
|---|---|
| Dimmed icon | The condition is on none of the selected tokens |
| Bright icon, solid border | It is on every selected token |
| Bright icon, dashed border | It is on some of them |
| Very faint icon | Nothing is selected (pressing just flashes the alert) |
| Grey border | Foundry can't be reached |

Pressing adds the condition to every selected token that lacks it, or, if they all have it,
removes it from all of them. The key updates once Foundry has applied the change.

A condition implied by another one, such as *incapacitated* while a token is unconscious,
paralyzed, petrified or stunned, shows as on, but pressing can't remove it. Remove the condition
that causes it instead.

## Logs

OpenDeck captures the plugin's stdout and stderr to:

```
~/.local/share/opendeck/logs/plugins/us.elbert.foundryvtt.log
```
