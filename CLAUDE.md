# CLAUDE.md

Code-level notes for `foundryvtt-streamdeck-plugin`. User-facing docs live in `README.md`; the code
itself is comment-free per workspace convention, so the non-obvious decisions are recorded here.

An OpenAction plugin (OpenDeck / Tacto) written in Rust with the `openaction` crate. Currently a
scaffold: one Counter action plus a minimal property inspector. No Foundry integration yet.

## Layout

| Path | Notes |
|---|---|
| `src/main.rs` | Wiring only: logger, config, relay + poller spawn, `set_global_event_handler`, `register_action`, `run`. |
| `src/config.rs` | `Config`, env fallback, redaction. Hand-written `Debug` — never derive it, it holds the API key. |
| `src/relay.rs` | Relay WebSocket client: auth, reconnect, request correlation. |
| `src/foundry.rs` | The `execute-js` script builders. Kept separate so they can be reviewed, and pasted into a Foundry console, without digging through handler logic. |
| `src/image.rs` | Art cache (TTL 10 min, LRU 32) and the SVG-wrapper fallback. |
| `src/counter.rs` | Scaffold action. |
| `src/sheet.rs` | Sheet-toggle action, instance mirror, poll loop. |
| `assets/` | **Staging directory, not an installed path.** See below. |
| `build.sh` | `cargo build` + assemble `dist/<uuid>.sdPlugin/` for hand-copying. |
| `dist/` | Build output, gitignored. |
| `.relay.local` | Gitignored test credentials, sourced into the environment by the harness. Never read by the plugin directly. |

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
`Icon`, `CategoryIcon`, the action `Icon`, and the state `Image` (`"actionDefaultImage"` is
special-cased to reuse the action icon).

No `<text>` in the SVG: OpenDeck rasterises into a 144x144 canvas where font availability isn't
guaranteed.

`Controllers` is `["Keypad"]` only. Advertising `"Encoder"` without implementing `dial_rotate` /
`touch_tap` would put a dead action on a Stream Deck+.

## Settings

`#[serde(default)]` is at **container** level, not per field. Per-field would give `step: 0` — a
counter that never counts. Container level routes missing fields through the manual `Default` impl.

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

All `Action` methods have no-op defaults, so the three implemented here are the entire surface.

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
| 5 | plugin -> host | `setSettings` `{step:1,value:1,...}` then `setTitle` `"1"` |
| 6 | host -> plugin | `didReceiveSettings`, `{step:5,value:1,label:"Initiative"}` |
| 7 | plugin -> host | exactly one frame: `setTitle` `"Initiative\n1"`, and **no `setSettings`** |
| 8 | host -> plugin | `keyUp` at `step:5` -> `value:6`, title `"Initiative\n6"` |
| 9 | host -> plugin | `didReceiveSettings` with `label:"   "` -> title `"6"`, one line |

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

## Where the Foundry relay will attach

Not built yet, but these shape decisions already made:

- `run()` blocks for the process lifetime, so the relay client is a `tokio::spawn` started
  **before** it. It will want tokio's `time` feature for reconnect backoff.
- Push-driven title updates use `visible_instances()` / `get_instance()` — a relay event arrives
  with no `Instance` in hand, so that is the lookup path.
- Relay URL and scoped API key belong in **global** settings (`get_global_settings` /
  `set_global_settings`, plus `HasSettingsInterface: true`), not per-action settings. They are
  plugin-wide, and duplicating them per key would be miserable to retrofit.
- Shared state rides on the action struct: `register_action(Counter)` takes ownership, so
  `Counter { relay: Arc<Relay> }` is the natural home, with `&self` handlers reading it.
- The sibling `foundryvtt-rest-api-relay` speaks `ws://host:3010/ws/api?clientId=...` with
  first-message-only auth (`{"type":"auth","token":...}` within 10s or close code 4002), and ships
  generated `openapi.json` / `asyncapi.json` a client can be derived from.


## The sheet action

### Prerequisites in Foundry, both mandatory

`allowExecuteJs` must be **on** — there is no sheet open/close action anywhere in the relay or the
REST module, so `execute-js` is the only route. `notifyOnExecuteJs` must be **off**, or every press
and every poll whispers the GM.

The plugin **fails closed** on the second one: a one-shot probe runs on each relay session, and if
notifications are on it logs an error and sets `poll_ms = 0` for that session. Buttons still track
every change the deck itself causes (the toggle's return value is authoritative); only
externally-caused changes drift. Refusing to spam beats spamming.

### The instance mirror is not optional

`Instance.settings_json` is `pub(crate)` in openaction, so a poller holding `Arc<Instance>` from
`visible_instances()` **cannot read that instance's settings**. `sheet.rs` therefore keeps its own
`InstanceId -> SheetSettings` map, maintained in `will_appear` / `did_receive_settings` /
`will_disappear`. Removing it silently breaks polling.

### `_sheet`, not `sheet`, when only reading state

`actor.sheet` *constructs* an Application on first access (`client-document.mjs:213-231`). Polling N
actors through it would instantiate N sheets every interval. The read-only probe is
`!!(a._sheet && a._sheet.rendered)`; `_sheet` is only nulled in `_onSheetChange`, never by `close()`.
The toggle script uses `a.sheet` deliberately, because there it *wants* the construction.

### The toggle script's load-bearing details

- `render({force: true})` — `force` is mandatory. A bare `render()` silently no-ops on a closed
  ApplicationV2 and still resolves (`application.mjs:520-521`).
- `close({animate: false})` skips a ~1 s transition.
- **`.rendered` is re-read after the await.** `DocumentSheetV2#_canRender` throws on missing view
  permission, `#render` catches it, warns, and resolves normally — so resolution is not proof of
  opening. The re-read value is what makes the response authoritative.
- `minimized` is ignored: a minimized sheet reports `rendered === true`, which is the correct
  reading of "is this on screen".
- Scripts are spliced into `eval("(async () => { ... })()")`, so they are in **statement position**
  and need an explicit `return`.
- Return **plain JSON only**. A Document or Application makes the module's `JSON.stringify` throw
  inside `send()`, which swallows it — you get no response at all and a silent 30 s timeout.
- `execute-js` is sent with **no `userId`**. Supplying one activates the `codeExecutionPermission`
  role check; omitting it skips that while `allowExecuteJs` still applies.

### Artwork is fetched and drawn inside Foundry

The `execute-js` art script fetches the token image, draws it letterboxed into a 144x144 canvas, and
returns three `image/webp` data URLs — closed (no border), open (border), offline (grey border).

- **The border is drawn in the browser, not composited in Rust and not overlaid with SVG.** That
  removes any dependency on webkit2gtk rendering a `data:` URI nested inside an SVG `data:` URI,
  which cannot be verified without hardware.
- **Rust never decodes an image.** OpenDeck's webview decodes whatever we send, so `.webp`, `.png`
  and `.svg` token art all work. Compositing in Rust would need `image` *and* resvg, because
  Foundry's `DEFAULT_TOKEN` is an SVG.
- **Drawing from a `blob:` URL makes canvas tainting impossible** — blob URLs are same-origin
  whatever the source. A cross-origin S3 asset fails at `fetch` with a catchable error instead of a
  `SecurityError` at `toDataURL`.
- Downscaling in Foundry caps the payload at ~15 KB per variant instead of a multi-MB base64, and
  fixes aspect ratio at the source so OpenDeck's non-aspect-preserving `drawImage` is a no-op.
- `prototypeToken.texture.src` is the default. **`getPreferredArtwork()` is opt-in, not the
  default**, because it returns `showTokenPortrait ? texture : this.img` — i.e. the *portrait*
  unless that dnd5e flag is set.
- Wildcard `randomImg` srcs resolve via `getTokenImages()` then `.sort()[0]` — deterministic on
  purpose; a random pick would change the button image on every refetch and look like a bug.
- Video srcs are legal on `texture.src` (not on `img`) and fall back to the portrait.

**Known limitation:** cross-origin S3-hosted art is unreachable by *any* route, because the relay's
HTTP `GET /download` is itself a `fetch` + `FileReader` in the same browser
(`fileSystem.ts:315-327`). Falls back to the portrait, then to the manifest default. The fix is CORS
headers on the bucket.

### Relay client

- Success on auth is `{"type":"connected"}` — **not** `auth-success`; the relay docs are stale.
- The connection task owns the whole `WebSocketStream` and drives reads and writes in one `select!`.
  **Do not `split()` it.** The relay pings every 20 s and tungstenite's auto-Pong needs the write
  half to flush; single ownership makes that guaranteed rather than something to reason about.
- Client timeout is 25 s, inside the relay's 30 s, so our error surfaces first.
- On disconnect, `pending` is drained so in-flight callers fail immediately instead of waiting out
  the timeout, and the art cache is cleared because a new session may be a different world.
- The relay's WS `download-file` and `sheet-screenshot` **return no bytes** — `format` is stripped,
  the callback is nil, and the payload is discarded. Do not try to move images over `/ws/api`.

### Settings

**`clientId` is optional.** A scoped relay key carries its own `scopedClientId`, and the relay
resolves it when the field is blank — so `is_complete()` requires only URL and key, and a blank
clientId is omitted from *both* the connect URL and the auth frame. The resolved value comes back in
the `connected` frame and is surfaced in the inspector.

**`set_global_settings` only sends.** Config is applied when the host echoes
`didReceiveGlobalSettings`, which is also where the PI gets refreshed. Pushing connection state
immediately after `set_global_settings` repaints the inspector from the *pre-write* config and makes
the user's typing appear to vanish. `sheet-settings.mjs` regression-tests exactly this.

**`sendToPlugin` is routed by `(action, context)` against openaction's instance map**, so it is
silently dropped if no `willAppear` has registered that instance. Harmless in real use (an inspector
only opens for a placed button) but it will bite any test harness.

Connection config is **global**; actor binding is **per-instance**. `plugin_ready()` issues
`get_global_settings()` and `did_receive_global_settings` is the single place config is mutated —
`setConnection` from the PI writes and does nothing else, letting the host's echo apply it.

`HasSettingsInterface` is **unusable from openaction**: OpenDeck sends
`{"event":"showSettingsInterface"}` and openaction 2.7 has no handler for it, so the button would
appear and do nothing. Connection fields live in the sheet action's property inspector instead.

**The sheet action never calls `set_settings`.** Only the PI writes settings, which makes the
`did_receive_settings` -> `setSettings` feedback loop structurally impossible.

**The API key is never echoed to the PI** (`hasApiKey: bool` instead), an empty incoming `apiKey`
means "leave unchanged", and `Config`'s `Debug` is hand-written to redact.

### One manifest state, not two

Two states would let OpenDeck advance the state index on press, so the button would flip visually
even when `render({force:true})` was swallowed by a permission failure — it would lie. Foundry's
`.rendered` is the only source of truth, so there is nothing for the host to advance.

### Property inspectors are per-action

`pi.html` (counter) and `pi-sheet.html` (sheet) share no fields, so each is self-contained with
inline styles. A shared `pi.css` was considered and rejected: it adds an unverifiable stylesheet load
path on a headless box for the sake of ~25 duplicated lines.

## Testing

Three zero-dependency Node harnesses in the scratchpad, none committed (they would be the only JS in
a Rust repo):

| Harness | Covers |
|---|---|
| `mock-host.mjs` | The counter transcript. Ignores `getGlobalSettings`/`setGlobalSettings` so plugin-level frames don't shift its sequential assertions. |
| `sheet-test.mjs` | Mock OpenDeck **and** mock relay: auth, art fetch, toggle both ways, cache, batched polling, PI round trips, ping/pong, poller parking. |
| `sheet-failures.mjs` | Fail-closed on `notifyOnExecuteJs`, `execute-js` disabled, disconnect and reconnect. |

Assertions compare **parsed, key-sorted** objects — `set_settings`/`set_image` serialize through
maps, so key order is alphabetical rather than declaration order.

Two harness lessons worth keeping: waits must **filter by frame kind**, because the background poll
loop interleaves `execute-js` frames into the relay inbox and a naive "next frame" consumes the wrong
one; and the mock relay must stay **self-consistent** — if a toggle flips its state, its poll answers
have to agree, or the next poll legitimately contradicts the toggle.
