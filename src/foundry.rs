use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ArtSource {
	Token,
	Portrait,
	Preferred,
}

impl ArtSource {
	fn as_str(self) -> &'static str {
		match self {
			Self::Token => "token",
			Self::Portrait => "portrait",
			Self::Preferred => "preferred",
		}
	}
}

fn js(value: &str) -> String {
	serde_json::to_string(value).unwrap_or_else(|_| "\"\"".to_string())
}

pub fn toggle_script(uuid: &str) -> String {
	format!(
		r#"
try {{
  const a = await fromUuid({uuid});
  if (!a) return {{ error: "actor not found" }};
  const s = a.sheet;
  if (!s) return {{ error: "no sheet class for this actor" }};
  if (s.rendered) {{
    await s.close({{ animate: false }});
    return {{ open: !!s.rendered, name: a.name }};
  }}
  await s.render({{ force: true }});
  return {{ open: !!s.rendered, name: a.name }};
}} catch (e) {{
  return {{ error: String((e && e.message) || e) }};
}}
"#,
		uuid = js(uuid)
	)
}

pub const COMPANION_ID: &str = "foundryvtt-streamdeck";
pub const EVENT_HOOK: &str = "foundryvtt-streamdeck.event";

pub fn sync_script() -> String {
	format!(
		r#"
let notify = null;
try {{
  notify = !!game.settings.get("foundry-rest-api", "notifyOnExecuteJs");
}} catch (e) {{}}
const m = game.modules.get({id});
if (!m || !m.active || !m.api) return {{ module: null, notify, open: null, selection: null }};
const snap = m.api.snapshot();
return {{ module: snap.version, notify, open: snap.open, selection: snap.selection }};
"#,
		id = js(COMPANION_ID)
	)
}

fn companion_call(call: &str) -> String {
	format!(
		r#"
const m = game.modules.get({id});
if (!m || !m.active || !m.api) return {{ error: "companion module missing" }};
try {{
  return await m.api.{call};
}} catch (e) {{
  return {{ error: String((e && e.message) || e) }};
}}
"#,
		id = js(COMPANION_ID)
	)
}

pub fn conditions_script() -> String {
	companion_call("conditions()")
}

pub fn toggle_condition_script(id: &str) -> String {
	companion_call(&format!("toggleCondition({})", js(id)))
}

pub fn condition_art_script(id: &str, accent: &str, offline: &str) -> String {
	companion_call(&format!(
		"conditionArt({}, {{ accent: {}, offline: {} }})",
		js(id),
		js(accent),
		js(offline)
	))
}

pub fn art_script(uuid: &str, source: ArtSource, open_color: &str, offline_color: &str) -> String {
	format!(
		r##"
try {{
  const A = await fromUuid({uuid});
  if (!A) return {{ error: "actor not found" }};
  const MODE = {mode};
  const OPEN_COLOR = {open_color};
  const OFFLINE_COLOR = {offline_color};
  const VID = /\.(mp4|m4v|webm|ogv)(\?|$)/i;

  let src = null;
  if (MODE === "portrait") {{
    src = A.img;
  }} else {{
    if (MODE === "preferred" && A.getPreferredArtwork) {{
      try {{ const p = await A.getPreferredArtwork(); if (p && p.src) src = p.src; }} catch (e) {{}}
    }}
    if (!src) {{
      const tok = A.isToken ? A.token : A.prototypeToken;
      let s = (tok && tok.texture) ? tok.texture.src : null;
      if (s && s.includes("*")) {{
        try {{
          const imgs = await A.getTokenImages();
          s = (imgs && imgs.length) ? imgs.slice().sort()[0] : null;
        }} catch (e) {{ s = null; }}
      }}
      if (s && VID.test(s)) s = null;
      src = s;
    }}
    if (!src) src = A.img;
  }}
  if (!src) return {{ error: "no artwork" }};

  const url = /^(https?:)?\/\//.test(src) ? src : foundry.utils.getRoute(src);
  let res;
  try {{
    res = await fetch(url);
  }} catch (e) {{
    return {{ error: "fetch blocked (CORS?): " + String((e && e.message) || e), src }};
  }}
  if (!res.ok) return {{ error: "fetch " + res.status, src }};

  const obj = URL.createObjectURL(await res.blob());
  const img = new Image();
  img.src = obj;
  try {{
    await img.decode();
  }} catch (e) {{
    URL.revokeObjectURL(obj);
    return {{ error: "decode failed", src }};
  }}

  const draw = (color) => {{
    const c = document.createElement("canvas");
    c.width = 144; c.height = 144;
    const x = c.getContext("2d");
    x.fillStyle = "#1b1d24";
    x.fillRect(0, 0, 144, 144);
    const iw = img.naturalWidth || 144;
    const ih = img.naturalHeight || 144;
    const k = Math.min(144 / iw, 144 / ih);
    const w = iw * k, h = ih * k;
    x.drawImage(img, (144 - w) / 2, (144 - h) / 2, w, h);
    if (color) {{
      x.lineWidth = 10;
      x.strokeStyle = color;
      x.beginPath();
      if (x.roundRect) x.roundRect(5, 5, 134, 134, 12); else x.rect(5, 5, 134, 134);
      x.stroke();
    }}
    return c.toDataURL("image/webp", 0.9);
  }};

  const closed = draw(null);
  const open = draw(OPEN_COLOR);
  const offline = draw(OFFLINE_COLOR);
  URL.revokeObjectURL(obj);

  return {{ src, name: A.name, closed, open, offline, isOpen: !!(A._sheet && A._sheet.rendered) }};
}} catch (e) {{
  return {{ error: String((e && e.message) || e) }};
}}
"##,
		uuid = js(uuid),
		mode = js(source.as_str()),
		open_color = js(open_color),
		offline_color = js(offline_color)
	)
}
