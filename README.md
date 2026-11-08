# foundryvtt-streamdeck-plugin

An [OpenAction](https://openaction.amankhanna.me/) plugin for stream controller devices, built with
the `openaction` Rust crate. Runs under [OpenDeck](https://github.com/nekename/OpenDeck) or Tacto.

Five actions: a **Counter** scaffold, an **Actor Sheet** button that toggles a character sheet
open and closed in Foundry and shows that actor's token artwork, a **Condition** button that
shows and toggles a condition on the selected tokens, an **Initiative** button that starts
combat and then follows whose turn it is, and a **Macro** button that runs a Foundry macro.

## Requirements

- Rust (stable)
- OpenDeck
- For the Actor Sheet, Condition, Initiative and Macro actions: a Foundry world (dnd5e, for conditions)
  with the [Stream Deck Companion module](https://github.com/DanElbert/foundryvtt-streamdeck-module)
  (1.0 or later) enabled, and the GM's browser running on the same machine as OpenDeck, or on one
  that can reach it. No relay, no account, no other module.

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

A per-key counter shown as the key's title. A short press and a long press (hold for about
0.6 s) each do one of:

- **Step** — add that press's own step amount (may be negative).
- **Reset** — set the count back to the start value.
- **Nothing**.

Defaults: short press steps by 1, long press resets. A long press fires while the key is still
held and flashes a checkmark; releasing afterwards does nothing extra.

The property inspector has:

- **Label** — optional text shown above the count. With a label set, the title is two lines
  (`label` then the count); with it empty, just the count. Editing it updates the key immediately
  without resetting the count.
- **Start value** — what Reset returns to. Defaults to 0.
- **Short press** / **Long press** — the action dropdown, plus a step amount when the action is
  Step.

All of these are per-key settings, so the same action can appear on several keys with different
values. The number is drawn a little larger than OpenDeck's default title size; keys placed before
this version keep their old size until re-placed or adjusted in OpenDeck's title settings.

## The Actor Sheet action

Press to toggle the actor's character sheet open or closed in Foundry. The key shows the actor's
token art, with a coloured border while the sheet is open and a grey border when Foundry can't be
reached.

### How the plugin talks to Foundry

The plugin opens a small WebSocket server on your machine (`127.0.0.1:28415` by default). The
companion module, running in the GM's browser, connects to it and authenticates with a shared token.
Nothing runs anywhere else, and the plugin never runs arbitrary JavaScript in Foundry: it calls a
fixed set of companion functions.

### Plugin setup

Open any Foundry button's property inspector and expand **Plugin connection** (shared by every
button). Press **Generate** to create a token, **Copy** it, then **Save connection**. The status line
should read `listening on 127.0.0.1:28415 — waiting for companion`. Change the port if that one is
taken. Leave the listen address at `127.0.0.1` unless the GM's browser runs on another machine; then
use `0.0.0.0` and point Foundry at this machine's hostname. The token is the only thing guarding the
socket in that case, so only do it on a trusted network.

### Foundry setup

Install and enable the **Stream Deck Companion module** (`foundryvtt-streamdeck`) in the world. Its
manifest URL is:

```
https://raw.githubusercontent.com/DanElbert/foundryvtt-streamdeck-module/main/module.json
```

In the browser that runs beside OpenDeck (normally the GM's), open **Configure Settings → Module
Settings** and fill in **Stream Deck: Plugin URL** (`ws://localhost:28415` unless the port was
changed) and **Stream Deck: Token** (paste the token). These are per-browser settings; other
players' browsers never connect. The connection is made as soon as the settings are saved, and the
property inspector's status line changes to `companion 1.0.0 connected`.

A rejected token shows a permanent error notification in Foundry; fix the setting and it reconnects.
Browsers allow a page served over https to open a plain `ws://localhost` socket, so a remote Foundry
server works too. Recent Chrome versions may ask once for permission to reach the local network.

Then pick an actor from the dropdown (grouped by the world's actor folders), or paste an
`Actor.xxxxxxxx` UUID directly if the list is unavailable.

**Artwork** chooses between the token image, the portrait, or whatever the game system prefers.
Token is the default.

### State sync

There is no polling. The companion module pushes an event whenever an actor sheet opens or closes in
the GM's browser, and the matching buttons update immediately. Whenever the browser connects, which
includes every reload, it sends the full set of open sheets so the deck catches up. While no browser
is connected, every Foundry button shows its grey offline border and presses flash the alert.

## The Condition action

Each key is bound to one D&D 5e condition: blinded, charmed, deafened, frightened, grappled,
incapacitated, invisible, paralyzed, petrified, poisoned, prone, restrained, stunned, surprised or
unconscious.
Exhaustion isn't offered. Pick the condition in the property inspector; the list comes from
Foundry, so the companion must be connected.

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

## The Initiative action

One key that does two jobs, depending on whether a combat exists (the one the GM's combat tracker
is showing).

**No combat:** select tokens in Foundry and press. The plugin creates a combat, adds the selected
tokens, rolls initiative for all of them (no dialogs; hidden tokens' rolls go to the GM) and starts
round 1. The key shows crossed swords, dim when nothing is selected and bright with **Start (N)**
when N tokens are selected.

**During a combat:** the key shows the current combatant's token art, their name and the round
(`R3`), and follows turn changes made in the tracker. Pressing opens their character sheet, and
pressing again closes it; the key gets the same open border as an Actor Sheet button. A combat that
has been set up but not started shows the first combatant in the tracker, without a round.

The key never ends or advances a combat; do that in the tracker. The *Show combatant name* option
hides the name but keeps the round.

## The Macro action

Pick a macro from the dropdown (grouped by the world's macro folders; only macros the GM's browser
may run are listed), or paste a UUID — including a `Compendium.…` one — into the manual field. The
key shows the macro's icon and name; press it to run the macro in the GM's browser. The key
flashes OK once the macro has *started*, and the alert if it couldn't be found or run. Script
macros see the tokens selected there as `token` / `actor`, exactly as when run from the hotbar.

A macro that throws shows its error as a notification in Foundry, not on the deck. Changes to a
macro's name or icon show up after a reconnect, or at once with **Refresh art**.

## Logs

OpenDeck captures the plugin's stdout and stderr to:

```
~/.local/share/opendeck/logs/plugins/us.elbert.foundryvtt.log
```
