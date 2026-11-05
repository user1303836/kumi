// Built from hermes-plugin/ui/src/ by `npm run build` in hermes-plugin/ui/. Edit ui/src/, not this file.
"use strict";
(() => {
  // src/dashboard/theme.ts
  var T = {
    text: "var(--color-foreground)",
    secondary: "var(--color-card-foreground)",
    muted: "var(--color-muted-foreground)",
    faint: "var(--color-muted-foreground)",
    border: "var(--color-border)",
    card: "var(--color-card)",
    chip: "var(--color-muted)",
    accent: "var(--color-primary)",
    good: "var(--color-success)",
    warn: "var(--color-warning)",
    bad: "var(--color-destructive)"
  };
  var CANVAS = {
    text: "--color-card-foreground",
    faint: "--color-muted-foreground",
    border: "--color-border",
    accent: "--color-primary",
    strong: "--color-foreground"
  };

  // src/dashboard/react.ts
  var sdk = window.__HERMES_PLUGIN_SDK__;
  if (!sdk) throw new Error("Kumi: the dashboard plugin SDK is missing");
  var SDK = sdk;
  var React = sdk.React;
  var react_default = React;
  var {
    Fragment,
    createElement,
    useCallback,
    useEffect,
    useMemo,
    useRef,
    useState,
    useSyncExternalStore
  } = React;

  // src/shared/format.ts
  var DAWS = { ableton: "Live", live: "Live", reaper: "REAPER", renoise: "Renoise", soundcraft: "SoundCraft" };
  function dawName(daw) {
    return typeof daw === "string" && DAWS[daw.toLowerCase()] || "your DAW";
  }
  function nameOf(value) {
    if (typeof value === "string") return value.trim() || void 0;
    if (value && typeof value === "object" && typeof value.name === "string") return value.name.trim() || void 0;
    return void 0;
  }
  function focusPath(focus) {
    const parts = focus?.focus;
    const inner = nameOf(parts?.device) ?? nameOf(parts?.item?.name) ?? nameOf(parts?.clip);
    return [nameOf(parts?.track), inner].filter((part) => Boolean(part)).map((part) => clean(part, 40));
  }
  function focusPlace(focus) {
    const parts = focus?.focus;
    const view = nameOf(parts?.mainView?.label) ?? nameOf(parts?.view);
    const section = nameOf(parts?.section?.name) ?? nameOf(parts?.scene);
    return [view && clean(view, 20), section && clean(section, 24)].filter((part) => Boolean(part));
  }
  function focusParameter(focus) {
    const parameter = focus?.focus?.parameter;
    const name = nameOf(parameter?.name);
    if (!name) return void 0;
    const value = nameOf(parameter?.value ?? void 0);
    return clean(value ? `${name} ${value}` : name, 60);
  }
  function catchUpLines(focus, most = 4) {
    const entries = Array.isArray(focus?.catch_up) ? focus.catch_up : [];
    const all = entries.flatMap((entry) => entry && Array.isArray(entry.lines) ? entry.lines.filter((line) => typeof line === "string") : []);
    const unlisted = entries.reduce((sum, entry) => sum + (entry && typeof entry.more === "number" && entry.more > 0 ? Math.floor(entry.more) : 0), 0);
    return { lines: all.slice(0, most).map((line) => clean(line, 120)), more: Math.max(0, all.length - most) + unlisted };
  }
  function colorOf(value) {
    const color = value && typeof value === "object" ? value.color : void 0;
    return typeof color === "string" && /^#[0-9a-f]{6}$/i.test(color) ? color : void 0;
  }
  function clean(text, max = 80) {
    if (typeof text !== "string") return "";
    const one = text.replace(/[\u0000-\u001f\u007f]+/g, " ").replace(/\s+/g, " ").replace(/"/g, "”").trim();
    return one.length > max ? `${one.slice(0, max - 1)}…` : one;
  }
  var MINUS = "−";
  function num(value, digits = 1) {
    const fixed = Math.abs(value).toLocaleString("en-US", { maximumFractionDigits: digits, minimumFractionDigits: 0 });
    return value < 0 && fixed !== "0" ? `${MINUS}${fixed}` : fixed;
  }
  function bpm(tempo) {
    return typeof tempo === "number" && tempo > 0 ? `${num(tempo, 1)} BPM` : void 0;
  }
  function ago(at, now = Date.now()) {
    if (typeof at !== "number") return "";
    const seconds2 = Math.max(0, (now - at) / 1e3);
    if (seconds2 < 45) return "now";
    if (seconds2 < 3600) return `${Math.round(seconds2 / 60)} min ago`;
    if (seconds2 < 86400) return `${Math.round(seconds2 / 3600)} h ago`;
    return `${Math.round(seconds2 / 86400)} d ago`;
  }
  function seconds(value) {
    if (typeof value !== "number" || !(value >= 0)) return "";
    return value < 10 ? `${num(value, 2)} s` : value < 60 ? `${num(value, 1)} s` : `${Math.floor(value / 60)}:${String(Math.round(value % 60)).padStart(2, "0")}`;
  }
  var ID = /^[A-Za-z0-9][A-Za-z0-9_.:-]{0,63}$/;
  function plainId(value) {
    return typeof value === "string" && ID.test(value) ? value : null;
  }
  function canUndo(status, item) {
    return Boolean(item && status?.connected && !status.stale && !status.unsure && item.undo_available === true && plainId(item.id));
  }
  function undoRequest(item) {
    return `Undo Kumi change ${plainId(item.id) ?? ""}`.trim();
  }
  function soundRequest(soundId, name, revision) {
    const id = plainId(soundId);
    if (!id) return null;
    const from = revision ? plainId(revision) : null;
    const said = clean(name ?? "", 80);
    return `Use sound ${id}${from ? ` from map ${from}` : ""}${said ? ` (“${said}”)` : ""} on the selected track`;
  }
  var LOOK_AGAIN_REQUEST = "Kumi, look at my project again before changing anything else";
  function cleanupWords(cleanup) {
    if (cleanup === "restored") return "Kumi put back what it changed meanwhile.";
    if (cleanup === "unsure") return "Kumi isn’t sure it put everything back.";
    return "";
  }
  function chipWords(status, tempo) {
    if (!status) return { tone: "muted", text: "Kumi" };
    const daw = dawName(status.daw);
    const project = clean(status.project?.name ?? "", 40);
    if (status.connected) {
      const parts = [daw, project, bpm(tempo)].filter(Boolean);
      return { tone: "good", text: parts.join(" · ") };
    }
    if (status.stale && status.running === false) return { tone: "muted", text: project ? `${daw} · ${project} · Kumi stopped` : `${daw} · Kumi stopped` };
    if (status.stale && status.owner) return { tone: "warn", text: project ? `${daw} · ${project} · not answering` : `${daw} · not answering` };
    return { tone: "muted", text: `Kumi · ${daw} not connected` };
  }

  // src/shared/hooks.ts
  function useKumi(store) {
    return useSyncExternalStore(store.subscribe, store.get, store.get);
  }
  function useWant(store, part) {
    useEffect(() => store.want(part), [store, part]);
  }
  var AUDITION_CACHE_CHARS = 48 * 1024 * 1024;
  var Audition = class {
    constructor(load, limit = AUDITION_CACHE_CHARS) {
      this.load = load;
      this.limit = limit;
    }
    load;
    limit;
    audio = null;
    cache = /* @__PURE__ */ new Map();
    cached = 0;
    loading = /* @__PURE__ */ new Map();
    generation = 0;
    /** Moves on with every play and stop: a play whose turn has passed never starts its sound. */
    turn = 0;
    listeners = /* @__PURE__ */ new Set();
    playing = null;
    subscribe = (fn) => {
      this.listeners.add(fn);
      return () => this.listeners.delete(fn);
    };
    changed(playing) {
      this.playing = playing;
      for (const fn of [...this.listeners]) fn();
    }
    /** The data URL for `id`: from the cache, the load already under way, or the backend. */
    url(id) {
      const cached = this.cache.get(id);
      if (cached) {
        this.cache.delete(id);
        this.cache.set(id, cached);
        return Promise.resolve(cached);
      }
      const loading = this.loading.get(id);
      if (loading) return loading;
      const generation = this.generation;
      const promise = this.load(id).then((reply) => {
        if (typeof reply?.data_url !== "string" || !reply.data_url.startsWith("data:")) throw new Error("Kumi sent no audio for that sound.");
        if (generation === this.generation) this.keep(id, reply.data_url);
        return reply.data_url;
      }).finally(() => {
        if (this.loading.get(id) === promise) this.loading.delete(id);
      });
      this.loading.set(id, promise);
      return promise;
    }
    keep(id, url) {
      if (url.length > this.limit) return;
      this.cache.set(id, url);
      this.cached += url.length;
      for (const [oldest, kept] of this.cache) {
        if (this.cached <= this.limit) break;
        this.cache.delete(oldest);
        this.cached -= kept.length;
      }
    }
    /** Another profile or connection: stop, and forget every sound of the last one. */
    reset() {
      this.stop();
      this.generation += 1;
      this.cache.clear();
      this.cached = 0;
      this.loading.clear();
    }
    /** Play `id` (stopping whatever plays). A later `play` or `stop` takes over even while this one still loads, so
     *  there's never more than one sound playing, and never one that can't be stopped. */
    async play(id) {
      this.stop();
      const turn = this.turn;
      this.changed(id);
      try {
        const url = await this.url(id);
        if (turn !== this.turn) return;
        const audio = new Audio(url);
        this.audio = audio;
        audio.onended = () => {
          if (this.audio === audio) this.stop();
        };
        await audio.play();
      } catch (error) {
        if (turn === this.turn) this.stop();
        throw error;
      }
    }
    stop() {
      this.turn += 1;
      if (this.audio) {
        this.audio.pause();
        this.audio.onended = null;
        this.audio = null;
      }
      if (this.playing !== null) this.changed(null);
    }
    dispose() {
      this.reset();
      this.listeners.clear();
    }
  };
  function usePlaying(audition) {
    return useSyncExternalStore(audition.subscribe, () => audition.playing, () => audition.playing);
  }

  // src/dashboard/jsx-runtime.ts
  var Fragment2 = react_default.Fragment;
  function jsx(type, props, key) {
    return react_default.createElement(type, key === void 0 ? props : { ...props, key });
  }
  var jsxs = jsx;

  // src/shared/ui.tsx
  var S = {
    section: { display: "flex", flexDirection: "column", gap: 6, padding: "10px 12px", borderBottom: `1px solid ${T.border}` },
    heading: { display: "flex", alignItems: "center", gap: 6, fontSize: 11, letterSpacing: "0.06em", textTransform: "uppercase", color: T.muted },
    row: { display: "flex", alignItems: "center", gap: 6, minWidth: 0 },
    // A message can hold a URL or a path with no place to break: it wraps anywhere rather than widen the pane.
    text: { fontSize: 12.5, color: T.text, lineHeight: 1.45, overflowWrap: "anywhere" },
    muted: { fontSize: 12, color: T.muted, lineHeight: 1.45, overflowWrap: "anywhere" },
    faint: { fontSize: 11.5, color: T.faint },
    ellipsis: { overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap", minWidth: 0 }
  };
  function Chip({ children, title, tone }) {
    const color = tone === "accent" ? T.accent : tone === "good" ? T.good : tone === "warn" ? T.warn : tone === "bad" ? T.bad : T.secondary;
    return /* @__PURE__ */ jsx("span", { title, style: { fontSize: 11, lineHeight: "16px", padding: "0 6px", borderRadius: 3, background: T.chip, color, whiteSpace: "nowrap" }, children });
  }
  function Dot({ color, size = 7, title }) {
    return /* @__PURE__ */ jsx("span", { "aria-hidden": !title, title, style: { display: "inline-block", width: size, height: size, borderRadius: "50%", background: color, flex: "none" } });
  }
  function TextButton({
    children,
    onClick,
    title,
    disabled,
    strong
  }) {
    return /* @__PURE__ */ jsx(
      "button",
      {
        disabled,
        onClick,
        title,
        type: "button",
        style: {
          font: "inherit",
          fontSize: 11.5,
          lineHeight: "18px",
          padding: "0 7px",
          borderRadius: 3,
          border: `1px solid ${strong ? T.accent : T.border}`,
          background: "transparent",
          color: disabled ? T.faint : strong ? T.accent : T.secondary,
          cursor: disabled ? "default" : "pointer",
          whiteSpace: "nowrap",
          flex: "none"
        },
        children
      }
    );
  }
  function Section({ title, right, children }) {
    return /* @__PURE__ */ jsxs("section", { style: S.section, children: [
      /* @__PURE__ */ jsxs("div", { style: { ...S.heading, justifyContent: "space-between" }, children: [
        /* @__PURE__ */ jsx("span", { style: { ...S.row, gap: 6 }, children: title }),
        right
      ] }),
      children
    ] });
  }

  // src/shared/HistoryView.tsx
  var STATE = {
    applied: { mark: "✓", tone: "good", word: "applied" },
    undone: { mark: "↺", tone: "muted", word: "undone" },
    kept: { mark: "✓", tone: "good", word: "kept" },
    partial: { mark: "◐", tone: "warn", word: "partly applied" },
    unsure: { mark: "?", tone: "warn", word: "unsure" },
    refused: { mark: "✕", tone: "muted", word: "refused" },
    failed: { mark: "✕", tone: "bad", word: "failed" }
  };
  function stateOf(item) {
    return STATE[item.state ?? ""] ?? { mark: "·", tone: "muted", word: clean(item.state ?? "recorded", 20) };
  }
  var TONE = { good: T.good, warn: T.warn, bad: T.bad, muted: T.faint };
  function FocusView({ focus, now }) {
    const catchUp = catchUpLines(focus);
    if (!focus || !focus.focus && !focus.project && !catchUp.lines.length) {
      return /* @__PURE__ */ jsx("span", { style: S.muted, children: "Nothing in focus yet. Kumi shows what you select in your DAW once it's connected." });
    }
    const transport = focus.transport;
    const path = focusPath(focus);
    const parameter = focusParameter(focus);
    const busy = nameOf(focus.focus?.busy ?? void 0);
    const meter = nameOf(transport?.meter ?? void 0);
    return /* @__PURE__ */ jsxs("div", { style: { display: "flex", flexDirection: "column", gap: 6, opacity: focus.stale ? 0.6 : 1 }, children: [
      /* @__PURE__ */ jsxs("div", { style: { ...S.row, gap: 7 }, children: [
        /* @__PURE__ */ jsx(Dot, { color: colorOf(focus.focus?.track) ?? T.faint, size: 9 }),
        /* @__PURE__ */ jsx("span", { style: { ...S.text, ...S.ellipsis, fontWeight: 600 }, children: path.length ? path.join(" › ") : "Nothing selected" })
      ] }),
      parameter && /* @__PURE__ */ jsx("span", { style: { ...S.muted, ...S.ellipsis }, children: parameter }),
      /* @__PURE__ */ jsxs("div", { style: { ...S.row, gap: 4, flexWrap: "wrap" }, children: [
        focus.project?.name && /* @__PURE__ */ jsx(Chip, { title: "The open project", children: clean(focus.project.name, 40) }),
        focusPlace(focus).map((place, index) => /* @__PURE__ */ jsx(Chip, { children: place }, index)),
        typeof transport?.playing === "boolean" && /* @__PURE__ */ jsx(Chip, { tone: transport.playing ? "good" : void 0, children: transport.playing ? "▶ playing" : "■ stopped" }),
        bpm(transport?.tempo) && /* @__PURE__ */ jsx(Chip, { children: bpm(transport?.tempo) }),
        meter && /* @__PURE__ */ jsx(Chip, { children: clean(meter, 12) }),
        busy && /* @__PURE__ */ jsx(Chip, { tone: "warn", title: "The DAW answers late while this lasts, so what's shown may be behind", children: clean(busy, 40) }),
        focus.stale && (focus.running === false ? /* @__PURE__ */ jsx(Chip, { children: "Kumi stopped" }) : /* @__PURE__ */ jsx(Chip, { tone: "warn", children: "not answering" }))
      ] }),
      catchUp.lines.length > 0 && /* @__PURE__ */ jsxs("div", { style: { display: "flex", flexDirection: "column", gap: 2 }, children: [
        /* @__PURE__ */ jsx("span", { style: S.faint, children: "Since Kumi last looked" }),
        catchUp.lines.map((line, index) => /* @__PURE__ */ jsx("span", { style: { ...S.muted, ...S.ellipsis }, children: line }, index)),
        catchUp.more > 0 && /* @__PURE__ */ jsx("span", { style: S.faint, children: `and ${catchUp.more} more` })
      ] }),
      focus.stale && typeof focus.heard_at === "number" && /* @__PURE__ */ jsx("span", { style: S.faint, children: `Last heard ${ago(focus.heard_at, now)}.` })
    ] });
  }
  function HistoryView({ history, now, action }) {
    const items = history?.items ?? [];
    if (!items.length) {
      return /* @__PURE__ */ jsx("span", { style: S.muted, children: "No changes yet. Each change Kumi makes in your DAW shows here, with its undo." });
    }
    return /* @__PURE__ */ jsx("ol", { style: { listStyle: "none", margin: 0, padding: 0, display: "flex", flexDirection: "column", gap: 2 }, children: items.map((item) => {
      const state = stateOf(item);
      const track = nameOf(item.track);
      return /* @__PURE__ */ jsxs("li", { style: { ...S.row, gap: 7, padding: "4px 0", borderTop: `1px solid ${T.border}`, opacity: item.state === "undone" ? 0.7 : 1 }, children: [
        /* @__PURE__ */ jsx("span", { "aria-label": state.word, title: state.word, style: { width: 14, textAlign: "center", color: TONE[state.tone], flex: "none" }, children: state.mark }),
        /* @__PURE__ */ jsxs("span", { style: { display: "flex", flexDirection: "column", minWidth: 0, flex: 1 }, children: [
          /* @__PURE__ */ jsx("span", { style: { ...S.text, ...S.ellipsis, textDecoration: item.state === "undone" ? "line-through" : void 0 }, title: clean(item.title ?? "", 300), children: clean(item.title ?? item.label ?? item.id, 120) }),
          /* @__PURE__ */ jsxs("span", { style: { ...S.row, gap: 5 }, children: [
            track && /* @__PURE__ */ jsx(Dot, { color: colorOf(item.track) ?? T.faint, size: 6 }),
            /* @__PURE__ */ jsx("span", { style: { ...S.faint, ...S.ellipsis }, children: [track && clean(track, 24), item.family && clean(item.family, 16), item.label && clean(item.label, 12), ago(item.at, now)].filter(Boolean).join(" · ") })
          ] }),
          (item.state === "unsure" || item.state === "partial") && /* @__PURE__ */ jsx("span", { style: { ...S.muted, color: T.warn, whiteSpace: "normal" }, children: [clean(item.reason ?? "Kumi couldn’t confirm what happened.", 200), cleanupWords(item.cleanup), "Ask Kumi to look again before another change."].filter(Boolean).join(" ") })
        ] }),
        action?.(item)
      ] }, item.id);
    }) });
  }

  // src/shared/mapModel.ts
  var GRID = 64;
  var FALLBACK = ["#e85d4a", "#f5a623", "#50c8b4", "#7ed321", "#4a90e2", "#9b59b6", "#ec70c8", "#e5c440"];
  function groupColor(color, key) {
    if (Array.isArray(color) && color.length === 3 && color.every((c) => Number.isInteger(c) && c >= 0 && c <= 255)) {
      return `rgb(${color[0]}, ${color[1]}, ${color[2]})`;
    }
    if (typeof color === "string" && /^#[0-9a-f]{6}$/i.test(color)) return color;
    return FALLBACK[Math.abs(key) % FALLBACK.length];
  }
  var cellOf = (x, y) => Math.min(GRID - 1, Math.floor(y * GRID)) * GRID + Math.min(GRID - 1, Math.floor(x * GRID));
  function buildModel(map) {
    const points = [];
    const cells = /* @__PURE__ */ new Map();
    for (const raw of Array.isArray(map?.points) ? map.points : []) {
      if (!Array.isArray(raw) || raw.length < 4) continue;
      const [id, x, y, group] = raw;
      if (typeof id !== "string" || !Number.isFinite(x) || !Number.isFinite(y)) continue;
      const point = { id, x: Math.min(1, Math.max(0, x)), y: Math.min(1, Math.max(0, y)), group: Number.isInteger(group) ? group : -1 };
      const cell = cellOf(point.x, point.y);
      const list = cells.get(cell);
      if (list) list.push(points.length);
      else cells.set(cell, [points.length]);
      points.push(point);
    }
    const groups = Object.entries(map?.groups ?? {}).map(([key, group]) => ({
      key: Number(key),
      name: clean(group?.name ?? `group ${key}`, 32),
      color: groupColor(group?.color, Number(key)),
      count: typeof group?.count === "number" ? group.count : points.filter((point) => point.group === Number(key)).length
    })).filter((group) => Number.isInteger(group.key)).sort((a, b) => a.key - b.key);
    return { revision: map?.revision ?? null, points, groups, sounds: map?.sounds ?? {}, cells };
  }
  function fitView(width, height, margin = 16) {
    const size = Math.max(1, Math.min(width, height) - 2 * margin);
    return { scale: 1, dx: (width - size) / 2, dy: (height - size) / 2 };
  }
  function toScreen(view, size, x, y) {
    return [x * size * view.scale + view.dx, y * size * view.scale + view.dy];
  }
  function zoomAt(view, factor, px, py) {
    const scale = Math.min(40, Math.max(1, view.scale * factor));
    const applied = scale / view.scale;
    return { scale, dx: px - (px - view.dx) * applied, dy: py - (py - view.dy) * applied };
  }
  function pick(model, view, size, px, py, radius = 8, visible) {
    const unit = size * view.scale;
    if (unit <= 0) return -1;
    const mx = (px - view.dx) / unit;
    const my = (py - view.dy) / unit;
    const reach = radius / unit;
    const from = { x: Math.max(0, Math.floor((mx - reach) * GRID)), y: Math.max(0, Math.floor((my - reach) * GRID)) };
    const to = { x: Math.min(GRID - 1, Math.floor((mx + reach) * GRID)), y: Math.min(GRID - 1, Math.floor((my + reach) * GRID)) };
    let best = -1;
    let bestDistance = reach * reach;
    for (let cy = from.y; cy <= to.y; cy++) {
      for (let cx = from.x; cx <= to.x; cx++) {
        for (const index of model.cells.get(cy * GRID + cx) ?? []) {
          if (visible && !visible(index)) continue;
          const point = model.points[index];
          const distance = (point.x - mx) ** 2 + (point.y - my) ** 2;
          if (distance <= bestDistance) {
            bestDistance = distance;
            best = index;
          }
        }
      }
    }
    return best;
  }
  function searchSounds(model, query) {
    const words = query.toLowerCase().split(/\s+/).filter(Boolean);
    if (!words.length) return null;
    const found = /* @__PURE__ */ new Set();
    for (const point of model.points) {
      const sound = model.sounds[point.id];
      const text = [sound?.name, sound?.class, sound?.kind, sound?.folder].filter(Boolean).join(" ").toLowerCase();
      if (words.every((word) => text.includes(word))) found.add(point.id);
    }
    return found;
  }
  function frame(model, ids, width, height, margin = 48) {
    const chosen = model.points.filter((point) => ids.has(point.id));
    if (!chosen.length) return null;
    const xs = chosen.map((point) => point.x);
    const ys = chosen.map((point) => point.y);
    const [x0, x1, y0, y1] = [Math.min(...xs), Math.max(...xs), Math.min(...ys), Math.max(...ys)];
    const size = Math.max(1, Math.min(width, height) - 32);
    const span = Math.max(x1 - x0, y1 - y0, 0.04);
    const scale = Math.min(40, Math.max(1, (Math.min(width, height) - 2 * margin) / (span * size)));
    const cx = (x0 + x1) / 2;
    const cy = (y0 + y1) / 2;
    return { scale, dx: width / 2 - cx * size * scale, dy: height / 2 - cy * size * scale };
  }

  // src/shared/MapCanvas.tsx
  function resolve(canvas) {
    const style = getComputedStyle(canvas);
    const read = (name, fallback) => style.getPropertyValue(name).trim() || fallback;
    return {
      text: read(CANVAS.text, "#999"),
      faint: read(CANVAS.faint, "#666"),
      border: read(CANVAS.border, "#444"),
      accent: read(CANVAS.accent, "#4a90e2"),
      strong: read(CANVAS.strong, "#ddd")
    };
  }
  function MapCanvas({ map, found, playing, selected, onPick, actions, empty }) {
    const model = useMemo(() => buildModel(map), [map]);
    const canvasRef = useRef(null);
    const boxRef = useRef(null);
    const [size, setSize] = useState({ width: 0, height: 0 });
    const [view, setView] = useState(null);
    const [query, setQuery] = useState("");
    const [hidden, setHidden] = useState(() => /* @__PURE__ */ new Set());
    const [hover, setHover] = useState(null);
    const drag = useRef(null);
    const mapSize = Math.max(1, Math.min(size.width, size.height) - 32);
    const hasPoints = model.points.length > 0;
    const searched = useMemo(() => searchSounds(model, query), [model, query]);
    const indexOf = useMemo(() => new Map(model.points.map((point, index) => [point.id, index])), [model]);
    const foundIds = useMemo(
      () => found && (found.revision === null || found.revision === model.revision) ? new Set(found.ids) : null,
      [found, model.revision]
    );
    const visible = useMemo(() => {
      const groupHidden = hidden.size > 0;
      return (index) => {
        const point = model.points[index];
        if (groupHidden && hidden.has(point.group)) return false;
        return !searched || searched.has(point.id);
      };
    }, [model, hidden, searched]);
    useEffect(() => {
      const box = boxRef.current;
      if (!box) return;
      const observer = new ResizeObserver((entries) => {
        const rect = entries[0]?.contentRect;
        if (rect) setSize({ width: Math.floor(rect.width), height: Math.floor(rect.height) });
      });
      observer.observe(box);
      return () => observer.disconnect();
    }, [hasPoints]);
    useEffect(() => {
      if (size.width > 0 && size.height > 0) setView((current) => current ?? fitView(size.width, size.height));
    }, [size.width, size.height]);
    useEffect(() => setView(size.width > 0 ? fitView(size.width, size.height) : null), [model.revision]);
    useEffect(() => {
      if (!foundIds || size.width === 0) return;
      const framed = frame(model, foundIds, size.width, size.height);
      if (framed) setView(framed);
    }, [foundIds, model, size.width, size.height]);
    useEffect(() => {
      const canvas = canvasRef.current;
      if (!canvas) return;
      const wheel = (event) => {
        event.preventDefault();
        const rect = canvas.getBoundingClientRect();
        const factor = Math.exp(-event.deltaY * (event.ctrlKey ? 0.01 : 25e-4));
        setView((current) => current ? zoomAt(current, factor, event.clientX - rect.left, event.clientY - rect.top) : current);
      };
      canvas.addEventListener("wheel", wheel, { passive: false });
      return () => canvas.removeEventListener("wheel", wheel);
    }, [hasPoints]);
    useEffect(() => {
      const canvas = canvasRef.current;
      if (!canvas || !view || size.width === 0) return;
      const ratio = window.devicePixelRatio || 1;
      canvas.width = Math.round(size.width * ratio);
      canvas.height = Math.round(size.height * ratio);
      const context = canvas.getContext("2d");
      if (!context) return;
      const colors = resolve(canvas);
      context.setTransform(ratio, 0, 0, ratio, 0, 0);
      context.clearRect(0, 0, size.width, size.height);
      const radius = Math.min(5, 2.2 + Math.log2(view.scale) * 0.8);
      const groupColor2 = new Map(model.groups.map((group) => [group.key, group.color]));
      for (const pass of [false, true]) {
        context.globalAlpha = pass ? 0.92 : 0.16;
        model.points.forEach((point, index) => {
          if (visible(index) !== pass) return;
          const [x, y] = toScreen(view, mapSize, point.x, point.y);
          if (x < -8 || y < -8 || x > size.width + 8 || y > size.height + 8) return;
          context.fillStyle = groupColor2.get(point.group) ?? colors.faint;
          context.beginPath();
          context.arc(x, y, radius, 0, Math.PI * 2);
          context.fill();
        });
      }
      context.globalAlpha = 1;
      const ring = (id, color, extra, width) => {
        const index = indexOf.get(id);
        if (index === void 0) return;
        const [x, y] = toScreen(view, mapSize, model.points[index].x, model.points[index].y);
        context.strokeStyle = color;
        context.lineWidth = width;
        context.beginPath();
        context.arc(x, y, radius + extra, 0, Math.PI * 2);
        context.stroke();
      };
      foundIds?.forEach((id) => ring(id, colors.accent, 3, 1.5));
      if (playing) ring(playing, colors.accent, 6, 2);
      if (selected) ring(selected, colors.strong, 4, 2);
    }, [model, indexOf, view, size, visible, foundIds, selected, playing, mapSize]);
    const at = (event) => {
      const rect = canvasRef.current?.getBoundingClientRect();
      return rect ? [event.clientX - rect.left, event.clientY - rect.top] : [0, 0];
    };
    if (!model.points.length) {
      return /* @__PURE__ */ jsx("div", { style: { ...S.muted, padding: 16 }, children: empty });
    }
    const chosen = selected ? model.sounds[selected] : void 0;
    const hovered = hover ? model.points[hover.index] : void 0;
    const hoveredSound = hovered ? model.sounds[hovered.id] : void 0;
    const shown = model.points.reduce((count, _, index) => count + (visible(index) ? 1 : 0), 0);
    return /* @__PURE__ */ jsxs("div", { style: { display: "flex", flexDirection: "column", height: "100%", minHeight: 240, color: T.text }, children: [
      /* @__PURE__ */ jsxs("div", { style: { ...S.row, flexWrap: "wrap", padding: "8px 10px", gap: 6, borderBottom: `1px solid ${T.border}` }, children: [
        /* @__PURE__ */ jsx(
          "input",
          {
            "aria-label": "Search sounds",
            onChange: (event) => setQuery(event.target.value),
            placeholder: "Search the map: name, class or folder",
            style: { flex: "1 1 180px", minWidth: 140, font: "inherit", fontSize: 12, padding: "3px 8px", borderRadius: 3, border: `1px solid ${T.border}`, background: "transparent", color: T.text },
            value: query
          }
        ),
        /* @__PURE__ */ jsxs("span", { style: S.faint, children: [
          shown === model.points.length ? `${model.points.length} sounds` : `${shown} of ${model.points.length} sounds`,
          model.revision ? ` · map ${model.revision}` : ""
        ] })
      ] }),
      /* @__PURE__ */ jsx("div", { style: { ...S.row, flexWrap: "wrap", gap: 4, padding: "6px 10px" }, children: model.groups.map((group) => {
        const off = hidden.has(group.key);
        return /* @__PURE__ */ jsxs(
          "button",
          {
            onClick: () => setHidden((current) => {
              const next = new Set(current);
              if (next.has(group.key)) next.delete(group.key);
              else next.add(group.key);
              return next;
            }),
            title: off ? `Show ${group.name}` : `Hide ${group.name}`,
            type: "button",
            style: { ...S.row, gap: 4, font: "inherit", fontSize: 11, padding: "1px 6px", borderRadius: 10, border: `1px solid ${T.border}`, background: "transparent", color: off ? T.faint : T.secondary, cursor: "pointer", opacity: off ? 0.6 : 1 },
            children: [
              /* @__PURE__ */ jsx("span", { style: { width: 8, height: 8, borderRadius: "50%", background: group.color, opacity: off ? 0.3 : 1 } }),
              group.name,
              " ",
              /* @__PURE__ */ jsx("span", { style: { color: T.faint }, children: group.count })
            ]
          },
          group.key
        );
      }) }),
      /* @__PURE__ */ jsxs("div", { ref: boxRef, style: { position: "relative", flex: 1, minHeight: 160, overflow: "hidden" }, children: [
        /* @__PURE__ */ jsx(
          "canvas",
          {
            ref: canvasRef,
            "aria-label": "Sound map",
            onPointerDown: (event) => {
              if (!view) return;
              event.currentTarget.setPointerCapture?.(event.pointerId);
              const [x, y] = at(event);
              drag.current = { x, y, moved: false, view };
            },
            onPointerMove: (event) => {
              const [x, y] = at(event);
              const held = drag.current;
              if (held) {
                if (Math.abs(x - held.x) + Math.abs(y - held.y) > 3) held.moved = true;
                if (held.moved) setView({ ...held.view, dx: held.view.dx + x - held.x, dy: held.view.dy + y - held.y });
                return;
              }
              if (!view) return;
              const index = pick(model, view, mapSize, x, y, 8, visible);
              setHover(index >= 0 ? { index, x, y } : null);
            },
            onPointerUp: (event) => {
              const held = drag.current;
              drag.current = null;
              if (!held || held.moved || !view) return;
              const [x, y] = at(event);
              const index = pick(model, view, mapSize, x, y, 10, visible);
              if (index >= 0) onPick(model.points[index].id);
            },
            onPointerLeave: () => setHover(null),
            onDoubleClick: () => setView(fitView(size.width, size.height)),
            style: { position: "absolute", inset: 0, width: "100%", height: "100%", cursor: hover ? "pointer" : drag.current?.moved ? "grabbing" : "grab", touchAction: "none" }
          }
        ),
        hovered && hoveredSound && hover && /* @__PURE__ */ jsxs(
          "div",
          {
            style: { position: "absolute", left: Math.min(hover.x + 12, Math.max(0, size.width - 220)), top: Math.max(0, hover.y - 34), maxWidth: 220, pointerEvents: "none", padding: "3px 7px", borderRadius: 3, background: T.card, border: `1px solid ${T.border}`, fontSize: 11.5, color: T.text, ...S.ellipsis },
            children: [
              clean(hoveredSound.name, 48),
              " ",
              /* @__PURE__ */ jsxs("span", { style: { color: T.muted }, children: [
                "· ",
                clean(hoveredSound.class ?? "", 16)
              ] })
            ]
          }
        )
      ] }),
      selected && chosen && /* @__PURE__ */ jsxs("div", { style: { ...S.row, justifyContent: "space-between", gap: 8, padding: "8px 10px", borderTop: `1px solid ${T.border}` }, children: [
        /* @__PURE__ */ jsxs("div", { style: { display: "flex", flexDirection: "column", minWidth: 0 }, children: [
          /* @__PURE__ */ jsx("span", { style: { ...S.text, ...S.ellipsis, fontWeight: 600 }, children: clean(chosen.name, 80) }),
          /* @__PURE__ */ jsxs("span", { style: { ...S.row, gap: 4, marginTop: 2 }, children: [
            chosen.class && /* @__PURE__ */ jsx(Chip, { children: clean(chosen.class, 16) }),
            chosen.kind && /* @__PURE__ */ jsx(Chip, { children: clean(chosen.kind, 16) }),
            typeof chosen.duration === "number" && /* @__PURE__ */ jsx(Chip, { children: seconds(chosen.duration) }),
            chosen.folder && /* @__PURE__ */ jsx("span", { style: { ...S.faint, ...S.ellipsis }, children: clean(chosen.folder, 60) })
          ] })
        ] }),
        /* @__PURE__ */ jsx("div", { style: { ...S.row, gap: 6 }, children: actions?.(selected, chosen) })
      ] })
    ] });
  }

  // src/shared/store.ts
  var JOB_KEPT_MS = 6e4;
  var MOST_JOBS = 20;
  var EMPTY = { available: null, error: null, status: null, focus: null, history: null, map: null, setup: null, found: null, jobs: {} };
  function finished(job) {
    return typeof job.state === "string" && job.state !== "running";
  }
  function statusOf(error) {
    const text = error instanceof Error ? error.message : String(error ?? "");
    const match = /^(\d{3})\b/.exec(text.trim()) ?? /\b(\d{3})\b/.exec(text);
    return match ? Number(match[1]) : 0;
  }
  function notLive(state) {
    return {
      status: state.status && { ...state.status, connected: false, stale: true },
      focus: state.focus && { ...state.focus, connected: false, stale: true },
      history: state.history && {
        ...state.history,
        connected: false,
        stale: true,
        items: state.history.items.map((item) => ({ ...item, undo_available: false }))
      }
    };
  }
  function heard(reply) {
    if (!reply || typeof reply !== "object" || !("age_ms" in reply)) return reply;
    const age = reply.age_ms;
    return { ...reply, heard_at: typeof age === "number" && Number.isFinite(age) ? Date.now() - age : null };
  }
  var KumiStore = class {
    constructor(fetch, historyLimit = 50) {
      this.fetch = fetch;
      this.historyLimit = historyLimit;
    }
    fetch;
    historyLimit;
    state = EMPTY;
    listeners = /* @__PURE__ */ new Set();
    reads = /* @__PURE__ */ new Map();
    wanted = /* @__PURE__ */ new Map();
    generation = 0;
    get = () => this.state;
    subscribe = (fn) => {
      this.listeners.add(fn);
      return () => this.listeners.delete(fn);
    };
    set(patch) {
      this.state = { ...this.state, ...patch };
      for (const fn of [...this.listeners]) fn();
    }
    /** A view that shows `part` asks for it while mounted: the map isn't read until something shows it. */
    want(part) {
      this.wanted.set(part, (this.wanted.get(part) ?? 0) + 1);
      if (!this.state[part]) void this.refresh(part);
      return () => this.wanted.set(part, Math.max(0, (this.wanted.get(part) ?? 1) - 1));
    }
    /** Forget everything (another profile or connection): what's shown must be that profile's. */
    reset() {
      this.generation += 1;
      this.reads.clear();
      this.state = EMPTY;
      for (const fn of [...this.listeners]) fn();
    }
    /** Run read `key`, or, while one is running, run it once more after it: a read never answers with what an
     *  older one, begun before the change that asked for it, saw. */
    once(key, run) {
      const running = this.reads.get(key);
      if (running) {
        running.again = true;
        return running.promise;
      }
      const generation = this.generation;
      const entry = { again: false, promise: Promise.resolve() };
      entry.promise = (async () => {
        do {
          entry.again = false;
          await run().catch(() => void 0);
        } while (entry.again && generation === this.generation);
      })().finally(() => {
        if (this.reads.get(key) === entry) this.reads.delete(key);
      });
      this.reads.set(key, entry);
      return entry.promise;
    }
    /** Read /status, then whatever its revisions say moved. */
    refreshStatus() {
      return this.once("status", async () => {
        const generation = this.generation;
        let status;
        try {
          status = await this.fetch("/status");
        } catch (error) {
          if (generation !== this.generation) return;
          if (statusOf(error) === 404) this.set({ available: false, error: null });
          else this.set({ ...notLive(this.state), error: error instanceof Error ? error.message : String(error) });
          return;
        }
        if (generation !== this.generation) return;
        const before = this.state.status;
        const after = status.revisions ?? {};
        const first = this.state.available !== true;
        const liveness = !before || before.connected !== status.connected || before.stale !== status.stale || before.running !== status.running;
        this.set({ available: true, error: null, status: heard(status) });
        const held = { focus: this.state.focus?.seq ?? null, history: this.state.history?.seq ?? null, map: this.state.map?.revision ?? null };
        const moved = (part) => this.state[part] === null || (after[part] ?? null) !== held[part];
        const reads = [];
        if (liveness || moved("focus")) reads.push(this.refresh("focus"));
        if (liveness || moved("history")) reads.push(this.refresh("history"));
        if ((this.wanted.get("map") ?? 0) > 0 && moved("map")) reads.push(this.refresh("map"));
        if (first || (this.wanted.get("setup") ?? 0) > 0) reads.push(this.refresh("setup"));
        await Promise.all(reads);
      });
    }
    refresh(part) {
      return this.once(part, async () => {
        const generation = this.generation;
        const path = part === "history" ? `/history?limit=${this.historyLimit}` : `/${part}`;
        try {
          const value = await this.fetch(path);
          if (generation === this.generation) this.set({ [part]: heard(value) });
        } catch (error) {
          if (generation !== this.generation) return;
          if (statusOf(error) === 404 && part !== "setup") this.set({ available: false });
        }
      });
    }
    /** The sounds a `kumi_sounds` call found, to light up on the map of the same revision. */
    found(revision, ids) {
      this.set({ found: { revision, ids: ids.slice(0, 200), at: Date.now() } });
    }
    /** Follow job `id` (from a tool's result or a `job.progress` event): read it now, then while it runs. */
    trackJob(id) {
      const jobId = plainId(id);
      if (!jobId || this.state.jobs[jobId] || Object.keys(this.state.jobs).length >= MOST_JOBS) return;
      void this.refreshJob(jobId);
    }
    /** Read every job still running and forget the finished ones kept long enough. While the runtime is quiet nothing
     *  moves, so nothing's read; once it has stopped, each job is read a last time, and one still "running" then ended
     *  with it. */
    refreshJobs(now = Date.now()) {
      const kept = Object.entries(this.state.jobs).filter(([, job]) => job.ended_at === null || now - job.ended_at < JOB_KEPT_MS);
      if (kept.length !== Object.keys(this.state.jobs).length) this.set({ jobs: Object.fromEntries(kept) });
      const stopped = this.state.status?.stale === true && this.state.status.running === false;
      if (this.state.status?.stale && !stopped) return Promise.resolve();
      return Promise.all(kept.filter(([, job]) => job.ended_at === null).map(([id]) => this.refreshJob(id, stopped))).then(() => void 0);
    }
    refreshJob(id, stopped = false) {
      return this.once(`job:${id}`, async () => {
        const generation = this.generation;
        try {
          const read = await this.fetch(`/job?id=${encodeURIComponent(id)}`);
          if (generation !== this.generation) return;
          const before = this.state.jobs[id];
          if (!before && finished(read)) return;
          const job = stopped && !finished(read) ? { ...read, state: "interrupted" } : read;
          const ended_at = finished(job) ? before?.ended_at ?? Date.now() : null;
          this.set({ jobs: { ...this.state.jobs, [id]: { ...job, job_id: id, ended_at } } });
        } catch (error) {
          if (generation !== this.generation || statusOf(error) !== 404 || !this.state.jobs[id]) return;
          const { [id]: _gone, ...rest } = this.state.jobs;
          this.set({ jobs: rest });
        }
      });
    }
  };

  // src/dashboard/index.tsx
  var BASE = "/api/plugins/kumi";
  var POLL_MS = 5e3;
  function useNow(every = 3e4) {
    const [now, setNow] = useState(() => Date.now());
    useEffect(() => {
      const timer = setInterval(() => setNow(Date.now()), every);
      return () => clearInterval(timer);
    }, [every]);
    return now;
  }
  function CopyButton({ text, label }) {
    const [done, setDone] = useState(false);
    return /* @__PURE__ */ jsx(
      TextButton,
      {
        onClick: () => {
          void navigator.clipboard?.writeText(text).then(() => {
            setDone(true);
            setTimeout(() => setDone(false), 1500);
          });
        },
        title: `Copy “${text}” to paste into a chat`,
        children: done ? "Copied" : label
      }
    );
  }
  function KumiDashboard() {
    const store = useMemo(() => new KumiStore((path) => SDK.fetchJSON(`${BASE}${path}`)), []);
    const audition = useMemo(() => new Audition((id) => SDK.fetchJSON(`${BASE}/audio?id=${encodeURIComponent(id)}`)), []);
    useWant(store, "map");
    const state = useKumi(store);
    const playing = usePlaying(audition);
    const now = useNow();
    const [selected, setSelected] = useState(null);
    useEffect(() => {
      void store.refreshStatus();
      const timer = setInterval(() => {
        if (document.visibilityState !== "hidden") void store.refreshStatus();
      }, POLL_MS);
      return () => {
        clearInterval(timer);
        audition.dispose();
      };
    }, [store, audition]);
    const words = chipWords(state.status, state.focus?.transport?.tempo);
    const tone = { good: T.good, warn: T.warn, bad: T.bad, muted: T.faint }[words.tone];
    const connected = Boolean(state.status?.connected);
    if (state.available === false) {
      return /* @__PURE__ */ jsxs("div", { style: { padding: 24, color: T.text }, children: [
        /* @__PURE__ */ jsx("p", { style: S.text, children: "Kumi's agent plugin is off in this profile." }),
        /* @__PURE__ */ jsx("p", { style: S.muted, children: "Turn it on (Hermes's plugins settings, or `hermes plugins enable kumi`) and restart the dashboard to see Kumi here." })
      ] });
    }
    return /* @__PURE__ */ jsxs("div", { style: { display: "flex", flexDirection: "column", gap: 12, color: T.text, minHeight: "calc(100vh - 160px)" }, children: [
      /* @__PURE__ */ jsxs("div", { style: { ...S.row, gap: 10, padding: "4px 2px" }, children: [
        /* @__PURE__ */ jsx(Dot, { color: tone, size: 9 }),
        /* @__PURE__ */ jsx("span", { style: { fontSize: 15, fontWeight: 600 }, children: words.text }),
        state.status?.runtime?.version && /* @__PURE__ */ jsxs("span", { style: S.faint, children: [
          "Kumi ",
          state.status.runtime.version
        ] }),
        state.setup?.message && state.setup.state && state.setup.state !== "ready" && state.setup.state !== "unknown" && /* @__PURE__ */ jsx("span", { style: S.muted, children: state.setup.message })
      ] }),
      state.status?.unsure && /* @__PURE__ */ jsxs("div", { style: { ...S.row, gap: 10, padding: "0 2px", alignItems: "flex-start" }, children: [
        /* @__PURE__ */ jsxs("span", { style: { ...S.muted, color: T.warn, flex: 1 }, children: [
          "Kumi isn't sure your project is as it was after ",
          clean(state.status.unsure.op ?? "its last action", 40),
          state.status.unsure.reason ? `: ${clean(state.status.unsure.reason, 160)}` : "",
          ". ",
          cleanupWords(state.status.unsure.cleanup),
          " Ask Kumi in your chat to look again before another change."
        ] }),
        /* @__PURE__ */ jsx(CopyButton, { label: "Copy request", text: LOOK_AGAIN_REQUEST })
      ] }),
      /* @__PURE__ */ jsxs("div", { style: { display: "grid", gridTemplateColumns: "minmax(260px, 1fr) minmax(320px, 2fr)", gap: 12, alignItems: "stretch" }, children: [
        /* @__PURE__ */ jsxs("div", { style: { border: `1px solid ${T.border}`, borderRadius: 6, background: T.card, overflow: "hidden", alignSelf: "start" }, children: [
          /* @__PURE__ */ jsx(Section, { title: "Focus", children: /* @__PURE__ */ jsx(FocusView, { focus: state.focus, now }) }),
          /* @__PURE__ */ jsx(Section, { title: "History", children: /* @__PURE__ */ jsx(
            HistoryView,
            {
              action: (item) => canUndo(state.status, item) ? /* @__PURE__ */ jsx(CopyButton, { label: "Copy undo", text: undoRequest(item) }) : null,
              history: state.history,
              now
            }
          ) })
        ] }),
        /* @__PURE__ */ jsx("div", { style: { border: `1px solid ${T.border}`, borderRadius: 6, background: T.card, overflow: "hidden", minHeight: 420, display: "flex", flexDirection: "column" }, children: /* @__PURE__ */ jsx(
          MapCanvas,
          {
            actions: (id, sound) => {
              const request = soundRequest(id, sound.name, state.map?.revision);
              return /* @__PURE__ */ jsxs(Fragment2, { children: [
                /* @__PURE__ */ jsx(TextButton, { onClick: () => playing === id ? audition.stop() : void audition.play(id).catch(() => void 0), children: playing === id ? "■ Stop" : "▶ Play" }),
                request && /* @__PURE__ */ jsx(CopyButton, { label: "Copy request", text: request })
              ] });
            },
            empty: state.map === null ? "Reading the sound map…" : "No sound map yet: add a sample folder in Kumi's settings and ask Kumi to scan it.",
            found: state.found,
            map: state.map,
            onPick: (id) => {
              setSelected(id);
              void audition.play(id).catch(() => void 0);
            },
            playing,
            selected
          }
        ) })
      ] })
    ] });
  }
  window.__HERMES_PLUGINS__?.register("kumi", KumiDashboard);
})();
