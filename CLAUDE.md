# CLAUDE.md

Code-level notes for `foundryvtt-streamdeck-plugin`. User-facing docs live in `README.md`; the code
itself is comment-free per workspace convention, so the non-obvious decisions are recorded here.

An OpenAction plugin (OpenDeck / Tacto) written in Rust with the `openaction` crate. Currently a
scaffold: one Counter action plus a minimal property inspector. No Foundry integration yet.

## Layout

| Path | Notes |
|---|---|
| `src/main.rs` | Whole plugin. One action; split into modules when a second lands. |
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
