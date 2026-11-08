# CLAUDE.md

Code-level notes for `foundryvtt-streamdeck-plugin`. User-facing docs live in `README.md`; the code
itself is comment-free per workspace convention, so the non-obvious decisions are recorded here.

An OpenAction plugin (OpenDeck / Tacto) written in Rust with the `openaction` crate: a standalone Counter
plus Actor Sheet, Condition, Initiative and Macro actions driven through the companion module.

## Layout

| Path | Notes |
|---|---|
| `src/main.rs` | Wiring only: logger, config, server + companion-event loop spawn, session start/end and listener-state tasks, `set_global_event_handler`, `register_action`, `run`. |
| `src/config.rs` | `Config`, env fallback, redaction. Hand-written `Debug` — never derive it, it holds the token. |
| `src/server.rs` | WebSocket server the companion module dials: token `hello`, one session at a time, ping deadline, request/result correlation. |
| `src/foundry.rs` | Typed wrappers for the companion API over `request` frames — the whole plugin→Foundry surface in one place. No JavaScript lives in the plugin. |
| `src/image.rs` | Generic `TtlCache` (TTL 10 min, LRU 32), token-art and condition-art entry types, and the SVG-wrapper fallback. |
| `src/counter.rs` | Counter action: short/long press, per-press action + step, reset-to-start. |
| `src/sheet.rs` | Sheet-toggle action and its instance mirror. |
| `src/condition.rs` | Condition-toggle action, its instance mirror, current token selection. |
| `src/initiative.rs` | Initiative action: combat state, start-combat press, current-combatant display via the sheet pipeline. |
| `src/macros.rs` | Macro action (`macro` is a reserved word, hence the module name). |
| `src/events.rs` | `Actions` (sheet + condition + initiative + macro), the companion event loop and the per-session resync. |
| `src/connection.rs` | Connection state pushed to, and `setConnection` from, every action's PI. Global config, so one implementation. |
| `assets/` | **Staging directory, not an installed path.** See below. |
| `build.sh` | `cargo build` + assemble `dist/<uuid>.sdPlugin/` for hand-copying. |
| `dist/` | Build output, gitignored. |

### `assets/` maps to the `.sdPlugin` root

OpenDeck's `read_manifest` does `base_path.join("manifest.json")` where `base_path` is the
`.sdPlugin` directory, and the reference plugins' build scripts copy `assets/*` up into that root.
So `assets/manifest.json` must land at `<uuid>.sdPlugin/manifest.json` — **not**
`<uuid>.sdPlugin/assets/manifest.json`. Get this wrong and OpenDeck silently fails to load the
plugin with no error. `build.sh` exists mostly to make that mistake impossible.

### Assets cannot be embedded in the binary

OpenDeck reads them itself; this process is never in the path.

- `plugins/webserver.rs` serves files from the plugin directory at request time, canonicalizing each
  path and rejecting anything escaping the plugin dir. The property inspector runs in an iframe
  loaded from that HTTP server (websocket port + 2), so OpenDeck opens `pi.html` directly.
- `manifest.json` is a chicken-and-egg: OpenDeck reads it to discover which binary to launch, so it
  cannot come from the binary.
- Icons are read from disk by `convert_icon` to populate the action list, independent of this
  process.

**Exception:** `Instance::set_image` accepts a base64 data URI, so button images pushed at runtime
*can* be `include_bytes!`'d in. That is distinct from the manifest `Icon`, which must be a file.

## UUIDs

Plugin `us.elbert.foundryvtt`, action `us.elbert.foundryvtt.counter`. Reverse-domain (`elbert.us`)
**plus project** — a bare `us.elbert` would claim the whole namespace and collide with any second
plugin, and the plugin id doubles as the installed directory name. Must be lowercase.

Renaming either orphans saved profile state on every key already configured, so treat them as
frozen.

## Icons

`convert_icon` probes `path + ".svg"`, then `path + "@2x.png"`, then `path + ".png"`. Hence
extension-less `"Icon": "icon"` in the manifest resolving to `icon.svg`. One file serves the plugin
`Icon`, `CategoryIcon` and most action `Icon`s; a state `Image` of `"actionDefaultImage"` is
special-cased to reuse the action icon, anything else is `plugin_dir.join(image)` through the same
`convert_icon`. The counter uses that split: `"Icon": "icon"` keeps the bright mark in the action
list while its state `Image` is the muted `counter.svg` (hex outline at 35 %, no fill) so the
title text stays readable on the key.

**Title font size is a manifest state field, not a protocol call.** OpenDeck's frontend composites
the title onto the image on a canvas in the webview and ships a JPEG to the device; `setTitle` only
carries text. `ActionState` deserialises the Elgato state keys (`FontSize`, `TitleAlignment`,
`TitleColor`, `FontFamily`, `ShowTitle`) via serde aliases, default size 16. The counter sets
`"FontSize": 20`. Placing a key copies the manifest states into the profile, so keys placed before
a `FontSize` change keep the old size until re-placed or edited in OpenDeck's title panel.

No `<text>` in the SVG: OpenDeck rasterises into a 144x144 canvas where font availability isn't
guaranteed.

`Controllers` is `["Keypad"]` only. Advertising `"Encoder"` without implementing `dial_rotate` /
`touch_tap` would put a dead action on a Stream Deck+.

## Settings

`#[serde(default)]` is at **container** level, not per field. Per-field would give `step: 0` — a
counter that never counts. Container level routes missing fields through the manual `Default` impl.
That is also the whole migration story for keys saved by older versions: `{step, value, label}`
hydrates to short-press Step(`step`), long-press Reset, `start: 0`. The short-press step is still
named `step` for that reason — renaming it would silently reset every configured key to 1.

`setSettings` **replaces the entire settings object** for an instance. The property inspector
therefore holds one `settings` object as its source of truth, mutates single fields, and re-sends
the whole thing: a merge on the PI side, forced replace on the wire. This is why editing Label does
not clobber `value`. The reference counter PI instead tracks `value` in a loose variable and
re-sends it, which is the "wholesale-replacing config objects" anti-pattern from the workspace
`CLAUDE.md` and does not scale past two fields.

The cleaner-in-principle alternative is `sendToPlugin` with only the changed field and a
plugin-side merge. Not chosen: an extra round trip, more plumbing, and divergence from every
reference PI worth comparing against.

## Action handlers

**`did_receive_settings` must never call `set_settings`.** That ping-pongs: host →
`didReceiveSettings` → plugin `setSettings` → host → `didReceiveSettings` → ... Only `key_up`
writes settings. There is a regression check for this (7b in the mock host below).

Three title-refresh points, all load-bearing:

| Hook | Why |
|---|---|
| `will_appear` | initial paint when the key becomes visible |
| `did_receive_settings` | makes property-inspector edits update the key live |
| `key_up` | after incrementing |

`will_disappear` only aborts a pending long-press timer.

### Long press

openaction awaits every inbound event inline in one loop (`inbound/mod.rs`), so a `sleep` inside
`key_down` would stall the matching `keyUp`. `key_down` therefore `tokio::spawn`s the 600 ms timer
and parks a `Hold { claimed: Arc<AtomicBool>, task: JoinHandle }` in `Counter.holds`, keyed by
instance id. The timer task snapshots the settings delivered with `keyDown` (an `Instance` can't
read its own settings — `settings_json` is `pub(crate)`) and re-resolves the instance with
`get_instance` after the sleep.

Exactly one of timer / release wins, decided by `claimed.swap(true)` on both sides:

- timer wakes, swap returns `false` → runs the long action, `show_ok`.
- `key_up`, swap returns `false` → aborts the timer (still asleep, nothing to tear down) and runs
  the short action.
- `key_up`, swap returns `true` → the long action already fired; do nothing and **do not abort**,
  the task may be mid-`set_settings`.

A bare `keyUp` with no prior `keyDown` (the mock host does this) is a short press, so the original
transcript still holds. `PressAction::Nothing` returns before `set_settings`, so it produces no
frames at all.

`register_action` must come **before** `run`; `run` blocks for the process lifetime. Logging goes
to stdout with `ColorChoice::Never` because OpenDeck redirects plugin stdout/stderr into a log file,
where ANSI codes are noise.

## Property inspector

The connect function **must be a plain window global**. OpenDeck's webserver appends a shim to every
PI HTML file that listens for a `postMessage` of `{event: "connect", payload: [...]}` and then calls
`connectOpenActionSocket(...payload)`, falling back to `connectElgatoStreamDeckSocket`. A
`<script type="module">` would silently never run. Both names are defined.

Other protocol details worth knowing:

- `inActionInfo` arrives as a **JSON string**, not an object.
- The `context` for `setSettings` comes from `inActionInfo.context`, not the PI's own UUID.
- `paint()` skips the focused element so an inbound `didReceiveSettings` — including the echo of our
  own write — cannot yank the caret mid-typing.
- 250 ms debounce on `input` plus an immediate `commit` on `change`, so the title updates live while
  typing and a blur/Enter always flushes.

## `run()` CLI contract

`run` matches flags **case-insensitively** and **panics** if any is absent: `-port`, `-pluginuuid`,
`-registerevent`, `-info`. It then dials `ws://localhost:<port>`. `Info.devices` has no serde
default, so `{"devices":[]}` is the minimal valid `-info`.

Running the binary by hand with no arguments panicking with `missing CLI flag: -port` is the
expected smoke-test result, not a bug.

## Distribution

`CodePaths` maps Rust target triples to `<triple>/bin/<name>`, which is what `cargo install --root`
produces and what a CI matrix would upload — so adding targets later is purely additive and needs no
manifest restructuring. Only the host triple is listed today because only it can be built.

No CI yet. When it lands: `assets/` becomes the zip root,
`cargo install --path . --target <triple> --root <root>/<triple>`, one zip named
`us.elbert.foundryvtt.sdPlugin`.

OpenDeck also merges `manifest.{linux,macos,windows}.json` overrides via json-patch. Those names
come from `std::env::consts::OS`, so it is `manifest.macos.json` — *not* `mac`, which differs from
the `OS[].Platform` spelling `"mac"`. Unused here.

## Testing

OpenDeck is not installed on the dev box, and it needs a real device anyway: with none attached it
shows `NoDevicesDetected`, so there is no slot to place an action in, `willAppear` never fires, and
there is nothing to press. Verification is therefore a mock OpenDeck host — a zero-dependency Node
script doing the RFC 6455 handshake and minimal text framing, spawning the binary with:

```
-port 57116 -pluginUUID us.elbert.foundryvtt -registerEvent registerPlugin -info '{"devices":[]}'
```

It is kept out of the repo (it would be the only JS in a Rust project); the transcript it asserts:

| # | Direction | Frame |
|---|---|---|
| 1 | plugin -> host | `registerPlugin`, `uuid` |
| 2 | host -> plugin | `willAppear`, `settings: {}` |
| 3 | plugin -> host | `setTitle` `{"title":"0","state":null}` — empty settings hydrating to the struct default |
| 4 | host -> plugin | `keyUp`, `settings: {step:1,value:0,label:""}` |
| 5 | plugin -> host | `setSettings` `{step:1,value:1,label:"",start:0,short_action:"step",long_action:"reset",long_step:1}` then `setTitle` `"1"` |
| 6 | host -> plugin | `didReceiveSettings`, `{step:5,value:1,label:"Initiative"}` |
| 7 | plugin -> host | exactly one frame: `setTitle` `"Initiative\n1"`, and **no `setSettings`** |
| 8 | host -> plugin | `keyUp` at `step:5` -> `value:6`, title `"Initiative\n6"` |
| 9 | host -> plugin | `didReceiveSettings` with `label:"   "` -> title `"6"`, one line |
| 10 | host -> plugin | `keyDown` then `keyUp` 100 ms later at `{step:2,value:6}` -> `setSettings` `value:8`, one `setTitle` |
| 11 | host -> plugin | `keyDown` at `{value:8,start:3}`, nothing else -> ~600 ms later `setSettings` `value:3`, `setTitle` `"3"`, `showOk`; a following `keyUp` sends nothing |
| 12 | host -> plugin | `keyDown` then `willDisappear` -> nothing, ever. openaction drops the instance on `willDisappear`, so the harness must send `willAppear` again before any further instance events |

Inbound payloads are camelCase with `settings`, `coordinates: {row,column}`, `controller`, `state`,
`isInMultiAction`; the inbound enum is `#[serde(tag = "event")]`, and `isInMultiAction` has no serde
default so it must be present. Outbound `state: null` is correct — `SetTitlePayload` has no
`skip_serializing_if`.

Compare **parsed, key-sorted** objects rather than raw JSON strings: `set_settings` serializes
through a map, so key order is alphabetical rather than struct declaration order, and it is not
semantic either way.

For real headless end-to-end testing, `openaction`'s `device_plugin` module (`register_device`,
`key_down`, `key_up`, `rerender_images`, ...) lets a plugin *provide* a virtual device — the
mechanism behind Tacto. A small companion plugin registering a fake 3x2 device would make
integration tests CI-able.

## The sheet action

### Requests go to the companion over our own socket

The plugin **hosts** a WebSocket server (`server.rs`); the companion module in the GM's browser dials
it. There is no relay and no REST module any more (that stack was retired in 1.0; the
`foundryvtt-rest-api-relay/` fork and `foundryvtt-rest-api/` checkouts in the workspace are history).
Every plugin→Foundry call is `{"type":"request","requestId","action","args":[…]}` (`Server::call`,
wrapped per action in `foundry.rs`); the companion dispatches `action` to its API and answers
`{"type":"result","requestId","result":…}` or `{"type":"result","requestId","error":…}`. See the
module's `CLAUDE.md` for the handler and for the toggle/art logic.

- **Two error layers:** a top-level `error` is a `ServerError::Remote` (dispatch: unknown action, a
  throw, an unserialisable reply); an `error` *inside* `result` is a domain answer like
  `{error:"no selection"}`, checked at each call site.
- **Connected means the companion is there.** Only the companion speaks this protocol, and its
  `hello` carries its version, so `server.companion()` is set at auth and cleared in `teardown`.
  The old "companion missing" state, its 5 s probe and `companion_ready` are gone; presses check
  `is_connected()` and alert locally when it is false.
- `REQUEST_TIMEOUT` is 25 s; a GM browser that stops answering is dropped by the ping deadline
  (below) well before that matters.

The companion module `foundryvtt-streamdeck` (`../foundryvtt-streamdeck-module`) 1.0+ is
**required**: it answers every request and pushes every state change. There is no fallback, and
plugin and module versions must move together (the wire protocol changed in 1.0 on both sides).

### Sheet state is pushed, not polled

The companion sends `{"type":"event","event":"sheet"|"snapshot"|"selection"|"combat", …}` frames;
`Server::dispatch` puts every `event` frame, as is, on the `events` broadcast channel and
`events::event_loop` keys on `event`. Here:

- Payloads are `{event:"sheet", uuid, open}` and `{event:"snapshot", open:[uuid...]}`. A snapshot is
  the complete set: `apply_snapshot` **replaces** `open`, so anything absent is closed.
- `resync` is the one `snapshot` request per session: open sheets, selection and combat. It also
  runs if the broadcast receiver lags. The companion pushes the same snapshot (plus forced
  selection/combat) right after `connected`, so the two usually agree; the request is kept because
  it is the testable, deterministic path.
- Every GM browser reload is now a full `session_ended` → `session_started` cycle (the socket is the
  browser's). The session-start task in `main.rs` therefore clears the token/macro art caches only
  when the `world` from `hello` differs from the previous session's, so a reload does not refetch
  every button's art.

### The instance mirror is not optional

`Instance.settings_json` is `pub(crate)` in openaction, so the event loop holding `Arc<Instance>` from
`visible_instances()` **cannot read that instance's settings**. `sheet.rs` therefore keeps its own
`InstanceId -> SheetSettings` map, maintained in `will_appear` / `did_receive_settings` /
`will_disappear`. Removing it silently breaks `repaint_visible`, so pushed events repaint nothing.

### Sheet toggle and artwork live in the companion

`toggleSheet` and `actorArt` used to be JavaScript strings built here; they are now companion API
functions, and their load-bearing details (`render({force:true})`, re-reading `.rendered`, `_sheet`
vs `sheet`, blob-URL drawing, wildcard/video handling, the S3 CORS limitation) are documented in the
module's `CLAUDE.md`. What still matters on this side:

- **Rust never decodes an image.** `actorArt` returns three 144² `image/webp` data URLs — closed,
  open (border), offline (grey border) — and OpenDeck's webview decodes whatever we pass to
  `set_image`. Compositing here would need `image` *and* resvg, because Foundry's `DEFAULT_TOKEN` is
  an SVG.
- The border is drawn in the browser, not overlaid with SVG here, which avoids depending on
  webkit2gtk rendering a `data:` URI nested inside an SVG `data:` URI (unverifiable without hardware).
  The `Indicator::Svg` option still does exactly that, as an opt-in.

### Server

`Server::run_forever` binds `bind:port` (`TcpListener`, `tokio_tungstenite::accept_async`), retrying
with backoff and surfacing a failed bind as `listen_error` in the PI, and rebinds on
`config_changed`. Inside, `serve` runs one `select!` over accept, authenticated connections, the
current session and `config_changed`:

- **Auth is a spawned per-connection `handshake`**, outside the session owner so a slow or hostile
  client can't starve pings: WebSocket accept, then the first text frame within 5 s must be
  `{"type":"hello","token","version","world"}`. Token compare is a byte fold, not `==`. Failure
  closes with `4001` (no/bad hello) or `4002` (bad token). Success sends
  `{"type":"connected","plugin":<version>}` and hands the stream to `serve` through a channel.
- **One session at a time.** A newer authenticated connection makes `serve` signal the running
  session to close its socket with `4003` ("replaced"), await it, `teardown`, then install the new
  one. The companion treats `4003` as "do not reconnect", so two GM browsers don't fight.
- **The session owns the whole `WebSocketStream`** and drives reads, writes, pings and the end
  signal in one `select!`. **Do not `split()` it.** It sends a protocol Ping every 20 s
  (`FOUNDRY_SD_PING_SECS` overrides, for tests) and drops the companion when nothing — including the
  browser's automatic Pongs — has arrived for `2 × interval + 5 s`. Browsers never ping, so this is
  the only dead-peer detection.
- `teardown` (connected→disconnected transition) fires `session_ended`, clears `tx` and `companion`,
  and drains `pending` so in-flight callers fail immediately instead of waiting out the timeout.
- `listen_changed` is a separate Notify so the PI status line follows bind/unbind without a repaint.
- `system_did_wake_up` still notifies `config_changed`: that closes and rebinds, and the browser
  reconnects within its backoff.

### Settings

**Config is `bind`, `port`, `token`** (plus the appearance fields). `is_complete()` is "token set";
without one the plugin does not listen and the PI says so. Global settings saved by pre-1.0 versions
(`relayUrl`/`apiKey`/`clientId`) still deserialize — unknown keys are ignored, container-level
`#[serde(default)]` fills the rest — so an upgrade just shows "no token: generate one and save".

**OpenDeck does not echo a plugin's own `setGlobalSettings` back to the plugin.** It writes the
file and sends `didReceiveGlobalSettings` to the *property inspectors* only
(`to_property_inspector = !from_property_inspector` in `events/inbound/settings.rs`). A plugin
`getGlobalSettings`, by contrast, replies to the plugin. So `apply_set_connection` sends `set` then
`get`: OpenDeck processes a socket's messages sequentially and syncs the file before returning, so
the `get` reply carries the new config. Without the `get`, Save wrote the file but nothing applied
until the plugin restarted — the mock harness hid this by echoing to the plugin. Config is still
applied only in `did_receive_global_settings`, which also refreshes the PI. Pushing connection state
immediately after the write repaints the inspector from the *pre-write* config and makes the
user's typing appear to vanish. `sheet-settings.mjs` regression-tests exactly this.

**`sendToPlugin` is routed by `(action, context)` against openaction's instance map**, so it is
silently dropped if no `willAppear` has registered that instance. Harmless in real use (an inspector
only opens for a placed button) but it will bite any test harness.

Connection config is **global**; actor binding is **per-instance**. `plugin_ready()` issues
`get_global_settings()` and `did_receive_global_settings` is the single place config is mutated —
`setConnection` from the PI writes and then re-reads, letting the reply apply it.

`HasSettingsInterface` is **unusable from openaction**: OpenDeck sends
`{"event":"showSettingsInterface"}` and openaction 2.7 has no handler for it, so the button would
appear and do nothing. Connection fields live in the sheet action's property inspector instead.

**The sheet action never calls `set_settings`.** Only the PI writes settings, which makes the
`did_receive_settings` -> `setSettings` feedback loop structurally impossible.

**The token *is* echoed to the PI**, unlike the old relay API key: the user has to copy it into
Foundry, it only guards a loopback socket, and the PI itself is served from OpenDeck's localhost
webserver. The PI generates it with `crypto.getRandomValues` (no Rust dependency), shows it in a
password field with Show/Copy, and an empty incoming `token` means "leave unchanged". `Config`'s
`Debug` is still hand-written to redact it from logs.

### Offline repaint

`Server::teardown` fires `session_ended` only on a connected→disconnected transition, and `main.rs`
repaints both actions and refreshes PIs on it. Without that, the `offline` variants were only shown
on the next unrelated repaint.

### Repaint refetches missing art

`repaint_visible` calls `refetch_if_missing` for every instance, which spawns `ensure_art` when the
companion is connected and the cache has no entry. `ensure_art` is otherwise only reached from `will_appear` /
`did_receive_settings` / `refreshArt`, so before this a cleared cache (session start, border colour
change) or a TTL-expired entry left the button on its manifest default until it was re-placed. Only
the repaint path spawns — `ensure_art`'s own trailing `paint` never does — so a failing fetch can't
loop; it just retries on the next repaint.

### One manifest state, not two

Two states would let OpenDeck advance the state index on press, so the button would flip visually
even when `render({force:true})` was swallowed by a permission failure — it would lie. Foundry's
`.rendered` is the only source of truth, so there is nothing for the host to advance.

### Property inspectors are per-action

`pi.html` (counter), `pi-sheet.html` (sheet), `pi-condition.html` (condition), `pi-initiative.html` (initiative) and `pi-macro.html` (macro) share no per-button
fields, so each is self-contained with
inline styles. A shared `pi.css` was considered and rejected: it adds an unverifiable stylesheet load
path on a headless box for the sake of ~25 duplicated lines.

## Testing

Zero-dependency Node harnesses in the scratchpad, none committed (they would be the only JS in
a Rust repo). Node 24's built-in `WebSocket` client plays the companion; the mock OpenDeck host is
still hand-rolled RFC 6455 server framing (Node has no WS server).

| Harness | Covers |
|---|---|
| `harness.mjs` | RFC 6455 server/frame helpers, a raw-TCP WS client (for the "never pongs" case), `waitFor`/`expectNone`. |
| `e2e.mjs` | Spawns the release binary under a mock host with `FOUNDRY_SD_PING_SECS=1`: listens on the configured port; PI `connection` state (listening/url/token echoed); bad token → `4002`, non-hello / no hello → `4001`; good hello → `connected` with the plugin version, PI shows the companion version before the snapshot reply, exactly one `snapshot` request, `actorArt` fetch and open-variant paint; pushed `sheet` close repaints, duplicate doesn't; press → `toggleSheet` with `requestId` echoed, top-level `error` alerts; second companion → first gets `4003`, same world keeps art; close → offline paint; new world refetches art; `setConnection` port change → `1001`, old port closed, new one accepts; silent raw companion dropped after ~8 s; token change rejects the old token and accepts the new. |
| `module-unit.mjs` | `main.mjs` under stubbed `Hooks`/`game.settings`/`foundry.utils.debounce`/`ui.notifications`/`canvas` and a fake `WebSocket` class: three client settings; no socket without a token; hello shape; no events before `connected`; snapshot + forced selection/combat after it; sheet close pushed once; `request` → `result` (result / unknown action / throw / domain error); `1006` reconnects after backoff and resyncs; `4003` and `4002` don't reconnect, `4002` notifies once permanently; URL/token changes reconnect; disabling closes. |

**PI state is eventually consistent, not ordered.** A `connection` push generated before the listener
bound (e.g. from `did_receive_global_settings`) can land after a later `getConnection` reply; the
`listen_changed` push corrects it. The harness therefore polls `getConnection` until the expected
state appears rather than asserting on the first frame.

Assertions compare **parsed, key-sorted** objects — `set_settings`/`set_image` serialize through
maps, so key order is alphabetical rather than declaration order.

Two harness lessons worth keeping: waits must **filter by frame kind**, because art fetches and
toggles interleave `request` frames into the companion inbox and a naive "next frame" consumes the
wrong one; and the mock companion must stay **self-consistent** — its snapshot, art `isOpen` and
toggle answers must agree, or a later answer legitimately contradicts an earlier one.

## The condition action

Each button is bound to one status id (`statusId`, plus `statusName` cached for painting before art
arrives). The list, toggling and artwork all live in the companion module's API; see its `CLAUDE.md`
for the Foundry side (tri-state rule, implied statuses, why exhaustion is excluded and surprised included).

- **Selection is pushed, never queried.** The companion's `{event:"selection", count, statuses}`
  replaces `ConditionState.selection` wholesale; `resync` seeds it from `snapshot().selection` and
  resets it to empty when a snapshot carries no selection. Unchanged selections don't repaint.
- **`key_up` never repaints.** It sends `toggleCondition` and only acts on an error (alert). The
  effect hooks in Foundry produce a `selection` event, and that is what repaints — so the button can
  never show a state Foundry didn't actually reach. It also alerts locally, without a round trip,
  when the last known selection is empty.
- **Variants**, chosen in `Variant::pick`:

  | Condition | Variant |
  |---|---|
  | companion disconnected | `offline` |
  | `count == 0` | `none` |
  | `statuses[id]` absent/0 | `off` |
  | `statuses[id] >= count` | `on` |
  | otherwise | `mixed` |

  All five are rendered in Foundry by `api.conditionArt` and cached per `(id, border colour)`, so a
  colour change refetches (`appearance_differs` clears both caches).
- The instance mirror exists for the same `settings_json` reason as the sheet action's.
- The condition PI omits the sheet-only *Open indicator* field. `setConnection` only overwrites
  fields present in the payload, so saving from either PI never clobbers the other's.

## The initiative action

State is the companion's `{event:"combat", combat}` push (see the module's `CLAUDE.md` for which combat and
whose turn); `resync` seeds it from `snapshot().combat`. The action owns no art or toggle code of its
own — it holds a `Sheet` clone and borrows:

- `Sheet::cached_art` / `fetch_art` with synthetic `SheetSettings { actor_uuid, art_source: Token }`,
  so combatant art shares the sheet cache (an Actor Sheet button for the same actor reuses it).
- `Sheet::styled`, so the configured *Open indicator* applies here too.
- `Sheet::toggle` for the press, and `Sheet::is_open` for the border. Sheet events therefore repaint
  initiative as well, and `selection` events repaint it for the `Start (N)` count (read through
  `ConditionState::selection_count`).

| State | Image | Title |
|---|---|---|
| no combat, 0 selected (or offline) | dim swords | none |
| no combat, N selected | bright swords | `Start (N)` |
| combat, no combatant / no actor | swords | `Empty` |
| combatant | sheet art variant (open/closed/offline) | `Name\nR{round}`; no round when unstarted |

The swords are an SVG string in `initiative.rs` sent as a base64 data URI (`set_image` takes those;
the manifest icon still has to be the file `assets/initiative.svg`).

**The art fetch can't call `repaint_visible` directly.** `paint` spawns it, and a spawned future
that reaches `paint` again is a recursive opaque type the compiler can't prove `Send`. The fetch
signals `InitiativeState::art_ready` instead and `Initiative::art_loop` (spawned in `main.rs`) does the
repaint. It only signals when art actually landed in the cache, so a failing fetch can't loop.

**The start press never repaints.** Like conditions, the combat that `startCombat()` creates reaches
the deck as a pushed `combat` event.

`events::Actions` bundles the three event-driven actions so `event_loop`, `resync`, the session
start/end tasks and the global-settings handler take one argument:

| Event | Applied to | Repaints |
|---|---|---|
| `sheet` / `snapshot` | sheet | sheet + initiative |
| `selection` | condition | condition + initiative |
| `combat` | initiative | initiative |

## The macro action

Stateless apart from art: nothing is pushed, so it takes no part in `event_loop` or `resync`.

- `key_up` gates on `is_connected()` like the sheet action, sends `executeMacro`, and answers with
  `show_ok` / `show_alert`. The companion replies once the macro has *started* (see its
  `CLAUDE.md`), so OK does not mean the script succeeded.
- Art is `TtlCache<String, MacroArt>` keyed by trimmed UUID, two variants (`ready`, `offline`), no
  accent. It is cleared on session start alongside token art, since macro UUIDs are per-world.
- The PI has no border-colour field; `setConnection` only overwrites fields present, so that's safe.

## Dropdown lists

The sheet and macro PIs fill their `<select>` from the companion's `actors` / `macros`, whose
entries carry a `folder` path; `fillGrouped` (duplicated in both PIs, per the no-shared-PI-code rule)
makes one `<optgroup>` per path. `foundry::list_reply` builds the `{event, status, <key>: list}`
reply for these and for `getConditions`.
