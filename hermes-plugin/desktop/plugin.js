// Built from hermes-plugin/ui/src/ by `npm run build` in hermes-plugin/ui/. Edit ui/src/, not this file.

// src/desktop/plugin.tsx
import * as sdk from "@hermes/plugin-sdk";
import { host as host5, PALETTE_AREA, PANES_AREA, STATUSBAR_AREAS } from "@hermes/plugin-sdk";

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
  return [nameOf(parts?.track), nameOf(parts?.device) ?? nameOf(parts?.clip)].filter((part) => Boolean(part)).map((part) => clean(part, 40));
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
function signed(value, digits = 1) {
  const text = num(value, digits);
  return value > 0 && text !== "0" ? `+${text}` : text;
}
function hz(value) {
  return value >= 1e4 ? `${num(value / 1e3, 1)} kHz` : `${num(value, 0)} Hz`;
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
function soundRequest(soundId, name) {
  const id = plainId(soundId);
  if (!id) return null;
  const said = clean(name ?? "", 80);
  return said ? `Use sound ${id} (“${said}”) on the selected track` : `Use sound ${id} on the selected track`;
}
function setupRequest(daw) {
  return `Set up Kumi for ${dawName(daw)}`;
}
function connectRequest(daw) {
  return `Connect Kumi to ${dawName(daw)}`;
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
  if (status.stale && status.owner) return { tone: "warn", text: project ? `${daw} · ${project} · not answering` : `${daw} · not answering` };
  return { tone: "muted", text: `Kumi · ${daw} not connected` };
}

// src/shared/hooks.ts
import { useEffect, useSyncExternalStore } from "react";
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

// src/shared/store.ts
var EMPTY = { available: null, error: null, status: null, focus: null, history: null, map: null, setup: null, found: null };
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
      const liveness = !before || before.connected !== status.connected || before.stale !== status.stale;
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
};
function foundSounds(result) {
  let value = result;
  if (typeof value === "string") {
    try {
      value = JSON.parse(value);
    } catch {
      return null;
    }
  }
  if (!value || typeof value !== "object" || !("items" in value) || !Array.isArray(value.items)) return null;
  const revision = "map_revision" in value && typeof value.map_revision === "string" ? value.map_revision : null;
  const ids = value.items.flatMap(
    (item) => item && typeof item === "object" && "sound_id" in item && typeof item.sound_id === "string" ? [item.sound_id] : []
  );
  return { revision, ids };
}

// src/desktop/actions.ts
import { host } from "@hermes/plugin-sdk";
var OPEN_WAIT_MS = 1e3;
var OPEN_POLL_MS = 100;
async function ask(ctx, text, options) {
  const composer = host.composer;
  const session = options.session ?? null;
  if (!composer) {
    const copied = await ctx.os?.writeClipboard?.(text) ?? false;
    host.notify({
      kind: copied ? "info" : "warning",
      message: copied ? "Copied the request: paste it into the chat to send it." : `Ask in the chat: ${text}`
    });
    return { outcome: copied ? "copied" : "none" };
  }
  if (!session) {
    if (await composer.insertText(null, text)) return { outcome: "drafted" };
    host.notify({ kind: "warning", message: "Open a chat first, then try again." });
    return { outcome: "none" };
  }
  const deliver = async () => options.send ? composer.submit(session, text) : composer.insertText(session, text);
  if (await deliver()) return { outcome: options.send ? "sent" : "drafted" };
  if (typeof host.openSession === "function") {
    try {
      await host.openSession(session);
      for (let waited = 0; waited <= OPEN_WAIT_MS; waited += OPEN_POLL_MS) {
        if (await deliver()) return { outcome: options.send ? "sent" : "drafted" };
        const tick = Promise.withResolvers();
        ctx.setTimeout(tick.resolve, OPEN_POLL_MS);
        await tick.promise;
      }
    } catch {
    }
  }
  if (await composer.insertText(null, text)) {
    host.notify({
      kind: "warning",
      message: "Kumi's chat isn't open here, so the request is in your input, unsent. Send it from the chat Kumi is connected to."
    });
    return { outcome: "drafted" };
  }
  host.notify({ kind: "warning", message: "Open a chat first, then try again." });
  return { outcome: "none" };
}

// src/desktop/theme.ts
var T = {
  text: "var(--ui-text-primary)",
  secondary: "var(--ui-text-secondary)",
  muted: "var(--ui-text-tertiary)",
  faint: "var(--ui-text-quaternary)",
  border: "var(--ui-stroke-secondary)",
  card: "var(--ui-bg-card)",
  chip: "var(--ui-bg-tertiary)",
  accent: "var(--ui-accent)",
  good: "var(--ui-green)",
  warn: "var(--ui-yellow)",
  bad: "var(--ui-red)"
};
var CANVAS = {
  text: "--ui-text-secondary",
  faint: "--ui-text-quaternary",
  border: "--ui-stroke-secondary",
  accent: "--ui-accent",
  strong: "--ui-text-primary"
};

// src/desktop/card.tsx
import { useEffect as useEffect2, useState } from "react";

// src/shared/listen.ts
var NOT_MEASURED = { preview: true, refused: true, cancelled: true, unsure: true };
function parseListen(result) {
  let value = result;
  if (typeof value === "string") {
    try {
      value = JSON.parse(value);
    } catch {
      return null;
    }
  }
  if (!value || typeof value !== "object" || !("metrics" in value) || !value.metrics || typeof value.metrics !== "object") return null;
  const state = "state" in value ? value.state : void 0;
  if (typeof state === "string" && NOT_MEASURED[state]) return null;
  return value;
}
var CAPTURED_BY = { ears: "Kumi Ears", offline: "an offline render" };
function captureWords(capture) {
  if (!capture || typeof capture !== "object") return void 0;
  const parts = [];
  const from = capture.from_beat;
  const beats = capture.beats;
  if (finite(from) && finite(beats)) parts.push(`Beats ${num(from, 2)}–${num(from + beats, 2)}`);
  if (finite(capture.seconds)) parts.push(`${num(capture.seconds, 1)} s measured`);
  const by = typeof capture.method === "string" ? CAPTURED_BY[capture.method] ?? clean(capture.method, 20) : "";
  const said = [parts.join(" · "), by && `through ${by}`].filter(Boolean).join(", ");
  if (!said) return void 0;
  return capture.cleanup === "restored" ? `${said} · your project put back as it was` : said;
}
var finite = (value) => typeof value === "number" && Number.isFinite(value);
function fileName(analysis, fallback) {
  const file = typeof analysis?.file === "string" ? analysis.file.split(/[\\/]/).pop() : void 0;
  return clean(file || fallback, 60);
}
function row(label, mine, reference, show, delta, deltaShow) {
  if (!finite(mine) && !finite(reference)) return null;
  const out = { label, mine: finite(mine) ? show(mine) : "—", direction: 0 };
  if (reference !== void 0) {
    out.reference = finite(reference) ? show(reference) : "—";
    const difference = finite(delta) ? delta : finite(mine) && finite(reference) ? mine - reference : void 0;
    if (difference !== void 0) {
      out.delta = (deltaShow ?? show)(difference);
      out.direction = difference < 0 ? -1 : difference > 0 ? 1 : 0;
    }
  }
  return out;
}
function listenView(result) {
  const mine = result.metrics ?? {};
  const compare = result.compare;
  const reference = compare?.metrics;
  const differences = compare?.differences;
  const theirs = (value) => compare ? value ?? null : void 0;
  const rows = [
    row("Loudness", mine.loudness?.integratedLufs, theirs(reference?.loudness?.integratedLufs), (v) => `${num(v)} LUFS`, differences?.loudness?.differenceLu, (v) => `${signed(v)} LU`),
    row("True peak", mine.loudness?.truePeakDbtp, theirs(reference?.loudness?.truePeakDbtp), (v) => `${num(v)} dBTP`, void 0, (v) => `${signed(v)} dB`),
    row("Brightness (centroid)", mine.balance?.centroidHz, theirs(reference?.balance?.centroidHz), hz, void 0, (v) => `${signed(v, 0)} Hz`),
    row("Loudness range", mine.loudness?.rangeLu, theirs(reference?.loudness?.rangeLu), (v) => `${num(v)} LU`, void 0, (v) => `${signed(v)} LU`),
    row("Crest", mine.dynamics?.crestDb, theirs(reference?.dynamics?.crestDb), (v) => `${num(v)} dB`, void 0, (v) => `${signed(v)} dB`),
    row("Tempo", mine.tempo?.bpm, theirs(reference?.tempo?.bpm), (v) => `${num(v)} BPM`, void 0, (v) => `${signed(v)} BPM`)
  ].filter((value) => value !== null);
  const key = mine.key?.name;
  if (typeof key === "string" && key) {
    const otherKey = reference?.key?.name;
    rows.push({ label: "Key", mine: clean(key, 20), ...compare ? { reference: typeof otherKey === "string" && otherKey ? clean(otherKey, 20) : "—" } : {}, direction: 0 });
  }
  const differenceBands = (Array.isArray(differences?.balance) ? differences.balance : []).flatMap(
    (band) => band && finite(band.difference) ? [{ name: clean(band.band ?? band.hz ?? "", 16), value: band.difference }] : []
  );
  const levelBands = (Array.isArray(mine.balance?.bands) ? mine.balance.bands : []).flatMap(
    (band) => band && finite(band.db) ? [{ name: clean(band.name ?? band.hz ?? "", 16), value: band.db }] : []
  );
  const view = {
    title: fileName(mine, "Measured audio"),
    mineId: typeof result.artifact_id === "string" ? result.artifact_id : void 0,
    rows,
    bands: differenceBands.length ? differenceBands : levelBands,
    bandsAre: differenceBands.length ? "difference" : "level",
    headlines: Array.isArray(differences?.headlines) ? differences.headlines.filter((line) => typeof line === "string").map((line) => clean(line, 140)) : [],
    verdict: typeof result.verdict_text === "string" && result.verdict_text ? clean(result.verdict_text, 400) : void 0,
    capture: captureWords(result.capture)
  };
  if (compare) {
    view.referenceTitle = fileName(reference, "Reference");
    view.referenceId = typeof compare.artifact_id === "string" ? compare.artifact_id : void 0;
  }
  const interpretation = result.interpretation;
  if (interpretation && typeof interpretation.text === "string" && interpretation.text.trim()) {
    const model = typeof interpretation.model === "string" && interpretation.model ? ` · ${clean(interpretation.model, 40)}` : "";
    view.interpretation = { label: `Interpretation${model}`, text: interpretation.text.trim().slice(0, 4e3) };
  } else if (typeof result.interpretation_error === "string" && result.interpretation_error) {
    view.interpretationError = clean(result.interpretation_error, 200);
  }
  return view;
}

// src/shared/ui.tsx
import { jsx, jsxs } from "react/jsx-runtime";
var S = {
  section: { display: "flex", flexDirection: "column", gap: 6, padding: "10px 12px", borderBottom: `1px solid ${T.border}` },
  heading: { display: "flex", alignItems: "center", gap: 6, fontSize: 11, letterSpacing: "0.06em", textTransform: "uppercase", color: T.muted },
  row: { display: "flex", alignItems: "center", gap: 6, minWidth: 0 },
  text: { fontSize: 12.5, color: T.text, lineHeight: 1.45 },
  muted: { fontSize: 12, color: T.muted, lineHeight: 1.45 },
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

// src/desktop/card.tsx
import { jsx as jsx2, jsxs as jsxs2 } from "react/jsx-runtime";
async function peaksOf(dataUrl, buckets = 96) {
  const comma = dataUrl.indexOf(",");
  const binary = atob(dataUrl.slice(comma + 1));
  const bytes = new Uint8Array(binary.length);
  for (let i = 0; i < binary.length; i++) bytes[i] = binary.charCodeAt(i);
  const decoded = await new OfflineAudioContext(1, 1, 44100).decodeAudioData(bytes.buffer);
  const samples = decoded.getChannelData(0);
  const step = Math.max(1, Math.floor(samples.length / buckets));
  const peaks = [];
  for (let start = 0; start < samples.length && peaks.length < buckets; start += step) {
    let peak = 0;
    for (let i = start; i < Math.min(samples.length, start + step); i++) peak = Math.max(peak, Math.abs(samples[i]));
    peaks.push(peak);
  }
  const top = Math.max(...peaks, 1e-6);
  return peaks.map((peak) => peak / top);
}
function Wave({ audition, id, label }) {
  const [peaks, setPeaks] = useState(null);
  const [failed, setFailed] = useState(false);
  const playing = usePlaying(audition) === id;
  useEffect2(() => {
    let live = true;
    audition.url(id).then(peaksOf).then((found) => live && setPeaks(found)).catch(() => live && setFailed(true));
    return () => {
      live = false;
    };
  }, [audition, id]);
  return /* @__PURE__ */ jsxs2("div", { style: { display: "flex", flexDirection: "column", gap: 3, flex: "1 1 160px", minWidth: 140 }, children: [
    /* @__PURE__ */ jsxs2("div", { style: { ...S.row, justifyContent: "space-between" }, children: [
      /* @__PURE__ */ jsx2("span", { style: { ...S.faint, ...S.ellipsis }, children: label }),
      /* @__PURE__ */ jsx2(TextButton, { onClick: () => playing ? audition.stop() : void audition.play(id).catch(() => setFailed(true)), children: playing ? "■" : "▶" })
    ] }),
    /* @__PURE__ */ jsxs2("svg", { "aria-label": `Waveform of ${label}`, height: 36, preserveAspectRatio: "none", role: "img", viewBox: "0 0 96 36", width: "100%", children: [
      (peaks ?? []).map((peak, index) => /* @__PURE__ */ jsx2("rect", { fill: playing ? T.accent : T.secondary, height: Math.max(1, peak * 34), opacity: 0.85, width: 0.7, x: index + 0.15, y: 18 - Math.max(0.5, peak * 17) }, index)),
      !peaks && /* @__PURE__ */ jsx2("line", { stroke: T.border, x1: 0, x2: 96, y1: 18, y2: 18 })
    ] }),
    failed && /* @__PURE__ */ jsx2("span", { style: S.faint, children: "The audio isn't available any more." })
  ] });
}
function Rows({ view }) {
  const compared = view.referenceTitle !== void 0;
  const cell = { padding: "2px 6px", textAlign: "right", fontVariantNumeric: "tabular-nums", whiteSpace: "nowrap" };
  return /* @__PURE__ */ jsxs2("table", { style: { borderCollapse: "collapse", fontSize: 12, color: T.text, width: "100%" }, children: [
    compared && /* @__PURE__ */ jsx2("thead", { children: /* @__PURE__ */ jsxs2("tr", { style: { color: T.muted, fontSize: 11 }, children: [
      /* @__PURE__ */ jsx2("th", { style: { ...cell, textAlign: "left", fontWeight: 400 } }),
      /* @__PURE__ */ jsx2("th", { style: { ...cell, fontWeight: 400 }, children: "This" }),
      /* @__PURE__ */ jsx2("th", { style: { ...cell, fontWeight: 400 }, children: "Reference" }),
      /* @__PURE__ */ jsx2("th", { style: { ...cell, fontWeight: 400 }, children: "Difference" })
    ] }) }),
    /* @__PURE__ */ jsx2("tbody", { children: view.rows.map((row2) => /* @__PURE__ */ jsxs2("tr", { style: { borderTop: `1px solid ${T.border}` }, children: [
      /* @__PURE__ */ jsx2("td", { style: { ...cell, textAlign: "left", color: T.muted }, children: row2.label }),
      /* @__PURE__ */ jsx2("td", { style: cell, children: row2.mine }),
      compared && /* @__PURE__ */ jsx2("td", { style: { ...cell, color: T.muted }, children: row2.reference ?? "—" }),
      compared && /* @__PURE__ */ jsx2("td", { style: { ...cell, color: row2.direction === 0 ? T.muted : T.text, fontWeight: 600 }, children: row2.delta ?? "—" })
    ] }, row2.label)) })
  ] });
}
function Bands({ view }) {
  if (!view.bands.length) return null;
  const top = Math.max(...view.bands.map((band) => Math.abs(band.value)), view.bandsAre === "difference" ? 3 : 1);
  return /* @__PURE__ */ jsxs2("div", { style: { display: "flex", flexDirection: "column", gap: 3 }, children: [
    /* @__PURE__ */ jsx2("span", { style: S.faint, children: view.bandsAre === "difference" ? "Band balance, this minus reference (level-matched)" : "Band levels" }),
    /* @__PURE__ */ jsx2("div", { style: { display: "flex", alignItems: "stretch", gap: 3, height: 54 }, children: view.bands.map((band) => {
      const share = Math.min(1, Math.abs(band.value) / top);
      const label = view.bandsAre === "difference" ? `${band.name}: ${signed(band.value)} dB` : `${band.name}: ${num(band.value)} dB`;
      return /* @__PURE__ */ jsxs2("div", { style: { flex: 1, display: "flex", flexDirection: "column", alignItems: "center", gap: 2, minWidth: 0 }, title: label, children: [
        /* @__PURE__ */ jsxs2("div", { style: { position: "relative", flex: 1, width: "100%" }, children: [
          view.bandsAre === "difference" ? /* @__PURE__ */ jsx2(
            "div",
            {
              style: {
                position: "absolute",
                left: "15%",
                right: "15%",
                background: band.value < 0 ? T.secondary : T.accent,
                opacity: 0.8,
                ...band.value < 0 ? { top: "50%", height: `${share * 50}%` } : { bottom: "50%", height: `${share * 50}%` }
              }
            }
          ) : /* @__PURE__ */ jsx2("div", { style: { position: "absolute", left: "15%", right: "15%", bottom: 0, height: `${Math.max(4, (1 - share) * 100)}%`, background: T.secondary, opacity: 0.8 } }),
          view.bandsAre === "difference" && /* @__PURE__ */ jsx2("div", { style: { position: "absolute", left: 0, right: 0, top: "50%", borderTop: `1px solid ${T.border}` } })
        ] }),
        /* @__PURE__ */ jsx2("span", { style: { fontSize: 9, color: T.faint, ...S.ellipsis, maxWidth: "100%" }, children: band.name })
      ] }, band.name);
    }) })
  ] });
}
function Interpretation({ view }) {
  const [open, setOpen] = useState(false);
  if (view.interpretationError) return /* @__PURE__ */ jsxs2("span", { style: S.faint, children: [
    "No model interpretation: ",
    view.interpretationError
  ] });
  if (!view.interpretation) return null;
  const text = view.interpretation.text;
  const long = text.length > 320;
  return /* @__PURE__ */ jsxs2("div", { style: { display: "flex", flexDirection: "column", gap: 3, borderLeft: `2px solid ${T.border}`, paddingLeft: 8 }, children: [
    /* @__PURE__ */ jsx2("span", { style: S.faint, children: view.interpretation.label }),
    /* @__PURE__ */ jsx2("span", { style: { ...S.muted, whiteSpace: "pre-wrap" }, children: long && !open ? `${text.slice(0, 320)}…` : text }),
    long && /* @__PURE__ */ jsx2("span", { children: /* @__PURE__ */ jsx2(TextButton, { onClick: () => setOpen(!open), children: open ? "Less" : "More" }) })
  ] });
}
function ListenCard({ media, audition }) {
  const parsed = parseListen(media.result);
  if (!parsed || media.file?.kind !== "audio") return media.fallback;
  const view = listenView(parsed);
  return /* @__PURE__ */ jsxs2(
    "div",
    {
      "data-kumi-card": "listen",
      style: { display: "flex", flexDirection: "column", gap: 9, maxWidth: 560, padding: "10px 12px", borderRadius: 6, border: `1px solid ${T.border}`, background: T.card, color: T.text },
      children: [
        /* @__PURE__ */ jsxs2("div", { style: { ...S.row, justifyContent: "space-between", alignItems: "flex-start", gap: 8 }, children: [
          /* @__PURE__ */ jsxs2("span", { style: { ...S.text, fontWeight: 600, minWidth: 0 }, children: [
            /* @__PURE__ */ jsx2("span", { style: { color: T.accent }, children: "♪" }),
            " ",
            view.title,
            view.referenceTitle && /* @__PURE__ */ jsxs2("span", { style: { color: T.muted, fontWeight: 400 }, children: [
              " vs ",
              view.referenceTitle
            ] })
          ] }),
          /* @__PURE__ */ jsx2(Chip, { title: "Measured on this computer by Kumi, from the audio file itself", children: "measured" })
        ] }),
        view.capture && /* @__PURE__ */ jsx2("span", { style: S.faint, title: "Kumi played this window and measured it; it isn't a change, so there's nothing to undo", children: view.capture }),
        view.verdict && /* @__PURE__ */ jsx2("span", { style: S.text, children: view.verdict }),
        view.mineId ? /* @__PURE__ */ jsxs2("div", { style: { ...S.row, gap: 12, flexWrap: "wrap", alignItems: "flex-start" }, children: [
          /* @__PURE__ */ jsx2(Wave, { audition, id: view.mineId, label: view.title }),
          view.referenceId && /* @__PURE__ */ jsx2(Wave, { audition, id: view.referenceId, label: view.referenceTitle ?? "Reference" })
        ] }) : media.fallback,
        /* @__PURE__ */ jsx2(Rows, { view }),
        /* @__PURE__ */ jsx2(Bands, { view }),
        view.headlines.length > 0 && /* @__PURE__ */ jsx2("ul", { style: { margin: 0, paddingLeft: 16, display: "flex", flexDirection: "column", gap: 1 }, children: view.headlines.map((line, index) => /* @__PURE__ */ jsx2("li", { style: S.muted, children: line }, index)) }),
        /* @__PURE__ */ jsx2(Interpretation, { view })
      ]
    }
  );
}

// src/desktop/composer.tsx
import { host as host2 } from "@hermes/plugin-sdk";
import { useCallback, useSyncExternalStore as useSyncExternalStore2 } from "react";

// src/desktop/pane.tsx
import { useEffect as useEffect3, useState as useState2 } from "react";

// src/shared/HistoryView.tsx
import { jsx as jsx3, jsxs as jsxs3 } from "react/jsx-runtime";
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
  if (!focus || !focus.focus && !focus.project) {
    return /* @__PURE__ */ jsx3("span", { style: S.muted, children: "Nothing in focus yet. Kumi shows what you select in your DAW once it's connected." });
  }
  const view = typeof focus.focus?.view === "string" ? focus.focus.view : void 0;
  const transport = focus.transport;
  const path = focusPath(focus);
  const catchUp = Array.isArray(focus.catch_up) ? focus.catch_up.filter((line) => typeof line === "string") : [];
  return /* @__PURE__ */ jsxs3("div", { style: { display: "flex", flexDirection: "column", gap: 6, opacity: focus.stale ? 0.6 : 1 }, children: [
    /* @__PURE__ */ jsxs3("div", { style: { ...S.row, gap: 7 }, children: [
      /* @__PURE__ */ jsx3(Dot, { color: colorOf(focus.focus?.track) ?? T.faint, size: 9 }),
      /* @__PURE__ */ jsx3("span", { style: { ...S.text, ...S.ellipsis, fontWeight: 600 }, children: path.length ? path.join(" › ") : "Nothing selected" })
    ] }),
    /* @__PURE__ */ jsxs3("div", { style: { ...S.row, gap: 4, flexWrap: "wrap" }, children: [
      focus.project?.name && /* @__PURE__ */ jsx3(Chip, { title: "The open project", children: clean(focus.project.name, 40) }),
      view && /* @__PURE__ */ jsx3(Chip, { children: clean(view, 20) }),
      transport?.playing !== void 0 && /* @__PURE__ */ jsx3(Chip, { tone: transport.playing ? "good" : void 0, children: transport.playing ? "▶ playing" : "■ stopped" }),
      bpm(transport?.tempo) && /* @__PURE__ */ jsx3(Chip, { children: bpm(transport?.tempo) }),
      typeof transport?.position === "string" && /* @__PURE__ */ jsx3(Chip, { children: clean(transport.position, 16) }),
      focus.stale && /* @__PURE__ */ jsx3(Chip, { tone: "warn", children: "not answering" })
    ] }),
    catchUp.length > 0 && /* @__PURE__ */ jsxs3("div", { style: { display: "flex", flexDirection: "column", gap: 2 }, children: [
      /* @__PURE__ */ jsx3("span", { style: S.faint, children: "Since Kumi last looked" }),
      catchUp.slice(0, 4).map((line, index) => /* @__PURE__ */ jsx3("span", { style: { ...S.muted, ...S.ellipsis }, children: clean(line, 120) }, index))
    ] }),
    focus.stale && typeof focus.heard_at === "number" && /* @__PURE__ */ jsx3("span", { style: S.faint, children: `Last heard ${ago(focus.heard_at, now)}.` })
  ] });
}
function HistoryView({ history, now, action }) {
  const items = history?.items ?? [];
  if (!items.length) {
    return /* @__PURE__ */ jsx3("span", { style: S.muted, children: "No changes yet. Each change Kumi makes in your DAW shows here, with its undo." });
  }
  return /* @__PURE__ */ jsx3("ol", { style: { listStyle: "none", margin: 0, padding: 0, display: "flex", flexDirection: "column", gap: 2 }, children: items.map((item) => {
    const state = stateOf(item);
    const track = nameOf(item.track);
    return /* @__PURE__ */ jsxs3("li", { style: { ...S.row, gap: 7, padding: "4px 0", borderTop: `1px solid ${T.border}`, opacity: item.state === "undone" ? 0.7 : 1 }, children: [
      /* @__PURE__ */ jsx3("span", { "aria-label": state.word, title: state.word, style: { width: 14, textAlign: "center", color: TONE[state.tone], flex: "none" }, children: state.mark }),
      /* @__PURE__ */ jsxs3("span", { style: { display: "flex", flexDirection: "column", minWidth: 0, flex: 1 }, children: [
        /* @__PURE__ */ jsx3("span", { style: { ...S.text, ...S.ellipsis, textDecoration: item.state === "undone" ? "line-through" : void 0 }, title: clean(item.title ?? "", 300), children: clean(item.title ?? item.label ?? item.id, 120) }),
        /* @__PURE__ */ jsxs3("span", { style: { ...S.row, gap: 5 }, children: [
          track && /* @__PURE__ */ jsx3(Dot, { color: colorOf(item.track) ?? T.faint, size: 6 }),
          /* @__PURE__ */ jsx3("span", { style: { ...S.faint, ...S.ellipsis }, children: [track && clean(track, 24), item.family && clean(item.family, 16), item.label && clean(item.label, 12), ago(item.at, now)].filter(Boolean).join(" · ") })
        ] }),
        (item.state === "unsure" || item.state === "partial") && /* @__PURE__ */ jsx3("span", { style: { ...S.muted, color: T.warn, whiteSpace: "normal" }, children: [clean(item.reason ?? "Kumi couldn’t confirm what happened.", 200), cleanupWords(item.cleanup), "Ask Kumi to look again before another change."].filter(Boolean).join(" ") })
      ] }),
      action?.(item)
    ] }, item.id);
  }) });
}

// src/desktop/pane.tsx
import { Fragment, jsx as jsx4, jsxs as jsxs4 } from "react/jsx-runtime";
function useNow(every = 3e4) {
  const [now, setNow] = useState2(() => Date.now());
  useEffect3(() => {
    const timer = setInterval(() => setNow(Date.now()), every);
    return () => clearInterval(timer);
  }, [every]);
  return now;
}
function ownerSession(state) {
  const session = state.status?.owner?.session_id;
  return typeof session === "string" && session ? session : null;
}
var NEEDS_SETUP = { needs_runtime: true, outdated_runtime: true, damaged_runtime: true, needs_daw_component: true };
function nextStep(state) {
  const setup = state.setup;
  const daw = state.status?.daw ?? setup?.daw;
  if (setup && (NEEDS_SETUP[setup.state ?? ""] || setup.next_step === "install_runtime" || setup.next_step === "install_daw_component")) {
    return { label: `Set up Kumi for ${dawName(daw)}`, request: setupRequest(daw) };
  }
  if (setup?.state === "ready" && !state.status?.connected) return { label: `Connect to ${dawName(daw)}`, request: connectRequest(daw) };
  return null;
}
var TONE2 = { good: T.good, warn: T.warn, bad: T.bad, muted: T.faint };
function KumiPane({ ctx, store, onOpenMap }) {
  const state = useKumi(store);
  const now = useNow();
  if (state.available === false) {
    return /* @__PURE__ */ jsxs4("div", { style: { padding: 14, display: "flex", flexDirection: "column", gap: 8 }, children: [
      /* @__PURE__ */ jsx4("span", { style: S.text, children: "Kumi's agent half is off in this profile." }),
      /* @__PURE__ */ jsx4("span", { style: S.muted, children: "Turn on the kumi plugin's Agent switch in Capabilities → Plugins, then restart the gateway. This pane shows your DAW's FOCUS and Kumi's HISTORY once it's on." })
    ] });
  }
  if (state.available === null) {
    return /* @__PURE__ */ jsx4("div", { style: { ...S.muted, padding: 14 }, children: "Reading Kumi's state…" });
  }
  const words = chipWords(state.status, state.focus?.transport?.tempo);
  const step = nextStep(state);
  const session = ownerSession(state);
  const connected = Boolean(state.status?.connected);
  const setupState = state.setup?.state;
  const setupMessage = typeof state.setup?.message === "string" && setupState && setupState !== "ready" && setupState !== "unknown" ? state.setup.message : null;
  const undoable = (state.history?.items ?? []).filter((item) => canUndo(state.status, item)).length;
  return /* @__PURE__ */ jsxs4("div", { style: { height: "100%", overflowY: "auto", color: T.text }, children: [
    /* @__PURE__ */ jsxs4("div", { style: { ...S.section, gap: 8 }, children: [
      /* @__PURE__ */ jsxs4("div", { style: { ...S.row, gap: 8 }, children: [
        /* @__PURE__ */ jsx4(Dot, { color: TONE2[words.tone], size: 8 }),
        /* @__PURE__ */ jsx4("span", { style: { ...S.text, ...S.ellipsis, fontWeight: 600, flex: 1 }, children: words.text }),
        state.status?.runtime?.version && /* @__PURE__ */ jsxs4("span", { style: S.faint, children: [
          "Kumi ",
          state.status.runtime.version
        ] })
      ] }),
      setupMessage && /* @__PURE__ */ jsx4("span", { style: S.muted, children: setupMessage }),
      state.error && /* @__PURE__ */ jsxs4("span", { style: { ...S.muted, color: T.warn }, children: [
        "Kumi's backend didn't answer: ",
        state.error
      ] }),
      state.status?.unsure && /* @__PURE__ */ jsxs4("div", { style: { display: "flex", flexDirection: "column", gap: 4 }, children: [
        /* @__PURE__ */ jsxs4("span", { style: { ...S.muted, color: T.warn }, children: [
          "Kumi isn't sure your project is as it was after ",
          clean(state.status.unsure.op ?? "its last action", 40),
          state.status.unsure.reason ? `: ${clean(state.status.unsure.reason, 160)}` : "",
          ". ",
          cleanupWords(state.status.unsure.cleanup),
          " It needs a fresh look before another change."
        ] }),
        /* @__PURE__ */ jsx4("div", { style: S.row, children: /* @__PURE__ */ jsx4(TextButton, { onClick: () => void ask(ctx, LOOK_AGAIN_REQUEST, { send: true, session }), children: "Ask Kumi to look again" }) })
      ] }),
      step && /* @__PURE__ */ jsx4("div", { style: S.row, children: /* @__PURE__ */ jsx4(TextButton, { onClick: () => void ask(ctx, step.request, { send: true, session: connected ? session : null }), strong: true, children: step.label }) })
    ] }),
    /* @__PURE__ */ jsx4(Section, { title: "Focus", children: /* @__PURE__ */ jsx4(FocusView, { focus: state.focus, now }) }),
    /* @__PURE__ */ jsx4(
      Section,
      {
        title: /* @__PURE__ */ jsxs4(Fragment, { children: [
          "History ",
          /* @__PURE__ */ jsx4("span", { style: { textTransform: "none", letterSpacing: 0 }, children: undoable ? `· ${undoable} can be undone` : "" })
        ] }),
        children: /* @__PURE__ */ jsx4(
          HistoryView,
          {
            history: state.history,
            now,
            action: (item) => canUndo(state.status, item) ? /* @__PURE__ */ jsx4(TextButton, { onClick: () => void ask(ctx, undoRequest(item), { send: true, session }), title: "Ask Kumi to undo this change", children: "Undo" }) : null
          }
        )
      }
    ),
    /* @__PURE__ */ jsxs4("div", { style: { ...S.row, padding: "10px 12px", gap: 8 }, children: [
      /* @__PURE__ */ jsx4(TextButton, { onClick: onOpenMap, children: "Open the sound map" }),
      !connected && state.status?.owner && /* @__PURE__ */ jsx4("span", { style: S.faint, children: "Undo needs a connected Kumi." })
    ] })
  ] });
}

// src/desktop/composer.tsx
import { jsx as jsx5, jsxs as jsxs5 } from "react/jsx-runtime";
var noChange = () => () => {
};
function useFocusedChat() {
  const atom = host2.state.focusedStoredSessionId;
  const subscribe = useCallback(
    (fn) => atom ? atom.listen ? atom.listen(() => fn()) : atom.subscribe(() => fn()) : noChange(),
    [atom]
  );
  const read = () => atom?.get() ?? null;
  return useSyncExternalStore2(subscribe, read, read);
}
function FocusHint({ store }) {
  useWant(store, "focus");
  const state = useKumi(store);
  const chat = useFocusedChat();
  const owner = ownerSession(state);
  if (!state.status?.connected || !owner || chat !== owner) return null;
  const path = focusPath(state.focus);
  if (!path.length) return null;
  const tempo = bpm(state.focus?.transport?.tempo);
  return /* @__PURE__ */ jsxs5(
    "div",
    {
      style: { ...S.row, gap: 6, padding: "2px 10px", maxWidth: "100%" },
      title: "What Kumi sees selected in your DAW: words like “this track” in this chat mean it.",
      children: [
        /* @__PURE__ */ jsx5(Dot, { color: colorOf(state.focus?.focus?.track) ?? T.faint, size: 7 }),
        /* @__PURE__ */ jsxs5("span", { style: { ...S.faint, ...S.ellipsis }, children: [
          "Kumi sees ",
          path.join(" › "),
          tempo ? ` · ${tempo}` : ""
        ] })
      ]
    }
  );
}

// src/desktop/chip.tsx
import { DropdownMenu, DropdownMenuContent, DropdownMenuItem, DropdownMenuSeparator, DropdownMenuTrigger, StatusDot } from "@hermes/plugin-sdk";
import { jsx as jsx6, jsxs as jsxs6 } from "react/jsx-runtime";
function StatusChip({
  ctx,
  store,
  onOpenMap,
  onOpenSettings
}) {
  const state = useKumi(store);
  if (state.available !== true) return null;
  const words = chipWords(state.status, state.focus?.transport?.tempo);
  const step = nextStep(state);
  const connected = Boolean(state.status?.connected);
  return /* @__PURE__ */ jsxs6(DropdownMenu, { children: [
    /* @__PURE__ */ jsx6(DropdownMenuTrigger, { asChild: true, children: /* @__PURE__ */ jsxs6(
      "button",
      {
        "aria-label": `Kumi: ${words.text}`,
        title: "Kumi",
        type: "button",
        style: {
          display: "inline-flex",
          alignItems: "center",
          gap: 5,
          height: "100%",
          padding: "0 6px",
          border: 0,
          background: "transparent",
          color: "var(--ui-text-tertiary)",
          font: "inherit",
          fontSize: "0.6875rem",
          fontVariantNumeric: "tabular-nums",
          cursor: "pointer",
          whiteSpace: "nowrap"
        },
        children: [
          /* @__PURE__ */ jsx6(StatusDot, { tone: words.tone }),
          words.text
        ]
      }
    ) }),
    /* @__PURE__ */ jsxs6(DropdownMenuContent, { align: "end", side: "top", children: [
      /* @__PURE__ */ jsx6(DropdownMenuItem, { onSelect: onOpenMap, children: "Open the sound map" }),
      step && /* @__PURE__ */ jsx6(DropdownMenuItem, { onSelect: () => void ask(ctx, step.request, { send: true, session: connected ? ownerSession(state) : null }), children: step.label }),
      /* @__PURE__ */ jsx6(DropdownMenuSeparator, {}),
      /* @__PURE__ */ jsx6(DropdownMenuItem, { onSelect: onOpenSettings, children: "Kumi settings" })
    ] })
  ] });
}

// src/desktop/map.tsx
import { host as host3 } from "@hermes/plugin-sdk";
import { useState as useState4 } from "react";

// src/shared/MapCanvas.tsx
import { useEffect as useEffect4, useMemo, useRef, useState as useState3 } from "react";

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
import { jsx as jsx7, jsxs as jsxs7 } from "react/jsx-runtime";
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
  const [size, setSize] = useState3({ width: 0, height: 0 });
  const [view, setView] = useState3(null);
  const [query, setQuery] = useState3("");
  const [hidden, setHidden] = useState3(() => /* @__PURE__ */ new Set());
  const [hover, setHover] = useState3(null);
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
  useEffect4(() => {
    const box = boxRef.current;
    if (!box) return;
    const observer = new ResizeObserver((entries) => {
      const rect = entries[0]?.contentRect;
      if (rect) setSize({ width: Math.floor(rect.width), height: Math.floor(rect.height) });
    });
    observer.observe(box);
    return () => observer.disconnect();
  }, [hasPoints]);
  useEffect4(() => {
    if (size.width > 0 && size.height > 0) setView((current) => current ?? fitView(size.width, size.height));
  }, [size.width, size.height]);
  useEffect4(() => setView(size.width > 0 ? fitView(size.width, size.height) : null), [model.revision]);
  useEffect4(() => {
    if (!foundIds || size.width === 0) return;
    const framed = frame(model, foundIds, size.width, size.height);
    if (framed) setView(framed);
  }, [foundIds, model, size.width, size.height]);
  useEffect4(() => {
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
  useEffect4(() => {
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
    return /* @__PURE__ */ jsx7("div", { style: { ...S.muted, padding: 16 }, children: empty });
  }
  const chosen = selected ? model.sounds[selected] : void 0;
  const hovered = hover ? model.points[hover.index] : void 0;
  const hoveredSound = hovered ? model.sounds[hovered.id] : void 0;
  const shown = model.points.reduce((count, _, index) => count + (visible(index) ? 1 : 0), 0);
  return /* @__PURE__ */ jsxs7("div", { style: { display: "flex", flexDirection: "column", height: "100%", minHeight: 240, color: T.text }, children: [
    /* @__PURE__ */ jsxs7("div", { style: { ...S.row, flexWrap: "wrap", padding: "8px 10px", gap: 6, borderBottom: `1px solid ${T.border}` }, children: [
      /* @__PURE__ */ jsx7(
        "input",
        {
          "aria-label": "Search sounds",
          onChange: (event) => setQuery(event.target.value),
          placeholder: "Search the map: name, class or folder",
          style: { flex: "1 1 180px", minWidth: 140, font: "inherit", fontSize: 12, padding: "3px 8px", borderRadius: 3, border: `1px solid ${T.border}`, background: "transparent", color: T.text },
          value: query
        }
      ),
      /* @__PURE__ */ jsxs7("span", { style: S.faint, children: [
        shown === model.points.length ? `${model.points.length} sounds` : `${shown} of ${model.points.length} sounds`,
        model.revision ? ` · map ${model.revision}` : ""
      ] })
    ] }),
    /* @__PURE__ */ jsx7("div", { style: { ...S.row, flexWrap: "wrap", gap: 4, padding: "6px 10px" }, children: model.groups.map((group) => {
      const off = hidden.has(group.key);
      return /* @__PURE__ */ jsxs7(
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
            /* @__PURE__ */ jsx7("span", { style: { width: 8, height: 8, borderRadius: "50%", background: group.color, opacity: off ? 0.3 : 1 } }),
            group.name,
            " ",
            /* @__PURE__ */ jsx7("span", { style: { color: T.faint }, children: group.count })
          ]
        },
        group.key
      );
    }) }),
    /* @__PURE__ */ jsxs7("div", { ref: boxRef, style: { position: "relative", flex: 1, minHeight: 160, overflow: "hidden" }, children: [
      /* @__PURE__ */ jsx7(
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
      hovered && hoveredSound && hover && /* @__PURE__ */ jsxs7(
        "div",
        {
          style: { position: "absolute", left: Math.min(hover.x + 12, Math.max(0, size.width - 220)), top: Math.max(0, hover.y - 34), maxWidth: 220, pointerEvents: "none", padding: "3px 7px", borderRadius: 3, background: T.card, border: `1px solid ${T.border}`, fontSize: 11.5, color: T.text, ...S.ellipsis },
          children: [
            clean(hoveredSound.name, 48),
            " ",
            /* @__PURE__ */ jsxs7("span", { style: { color: T.muted }, children: [
              "· ",
              clean(hoveredSound.class ?? "", 16)
            ] })
          ]
        }
      )
    ] }),
    selected && chosen && /* @__PURE__ */ jsxs7("div", { style: { ...S.row, justifyContent: "space-between", gap: 8, padding: "8px 10px", borderTop: `1px solid ${T.border}` }, children: [
      /* @__PURE__ */ jsxs7("div", { style: { display: "flex", flexDirection: "column", minWidth: 0 }, children: [
        /* @__PURE__ */ jsx7("span", { style: { ...S.text, ...S.ellipsis, fontWeight: 600 }, children: clean(chosen.name, 80) }),
        /* @__PURE__ */ jsxs7("span", { style: { ...S.row, gap: 4, marginTop: 2 }, children: [
          chosen.class && /* @__PURE__ */ jsx7(Chip, { children: clean(chosen.class, 16) }),
          chosen.kind && /* @__PURE__ */ jsx7(Chip, { children: clean(chosen.kind, 16) }),
          typeof chosen.duration === "number" && /* @__PURE__ */ jsx7(Chip, { children: seconds(chosen.duration) }),
          chosen.folder && /* @__PURE__ */ jsx7("span", { style: { ...S.faint, ...S.ellipsis }, children: clean(chosen.folder, 60) })
        ] })
      ] }),
      /* @__PURE__ */ jsx7("div", { style: { ...S.row, gap: 6 }, children: actions?.(selected, chosen) })
    ] })
  ] });
}

// src/desktop/map.tsx
import { Fragment as Fragment2, jsx as jsx8, jsxs as jsxs8 } from "react/jsx-runtime";
function errorText(error) {
  const text = error instanceof Error ? error.message : String(error ?? "");
  const json = text.indexOf("{");
  if (json >= 0) {
    try {
      const body = JSON.parse(text.slice(json));
      if (body && typeof body === "object" && "detail" in body && typeof body.detail === "string") return body.detail;
    } catch {
    }
  }
  return text || "no answer";
}
function MapWorkspace({ ctx, store, audition }) {
  useWant(store, "map");
  const state = useKumi(store);
  const playing = usePlaying(audition);
  const [selected, setSelected] = useState4(null);
  const play = (id) => {
    audition.play(id).catch((error) => host3.notify({ kind: "warning", message: `Kumi couldn't play that sound: ${errorText(error)}` }));
  };
  const map = state.map;
  const empty = state.available === false ? "Kumi's agent half is off in this profile, so there's no map to show." : map === null ? "Reading the sound map…" : "No sound map yet. Add a sample folder in Kumi's settings and ask Kumi to scan it; the map is made from what it finds there.";
  return /* @__PURE__ */ jsx8(
    MapCanvas,
    {
      actions: (id, sound) => {
        const request = soundRequest(id, sound.name);
        return /* @__PURE__ */ jsxs8(Fragment2, { children: [
          /* @__PURE__ */ jsx8(TextButton, { onClick: () => playing === id ? audition.stop() : play(id), title: "Play it here (nothing changes in your DAW)", children: playing === id ? "■ Stop" : "▶ Play" }),
          request && /* @__PURE__ */ jsx8(
            TextButton,
            {
              onClick: () => void ask(ctx, request, { send: false, session: ownerSession(state) }),
              strong: true,
              title: "Put a request to use this sound in the chat, to finish and send",
              children: "Use…"
            }
          )
        ] });
      },
      empty,
      found: state.found,
      map,
      onPick: (id) => {
        setSelected(id);
        play(id);
      },
      playing,
      selected
    }
  );
}

// src/desktop/settings.tsx
import { host as host4 } from "@hermes/plugin-sdk";
import { useState as useState5 } from "react";
import { Fragment as Fragment3, jsx as jsx9, jsxs as jsxs9 } from "react/jsx-runtime";
var OPEN_MAP_ON_FIND = "openMapOnFind";
var STATE_WORDS = {
  ready: "Ready",
  needs_runtime: "Not set up yet",
  outdated_runtime: "Needs the pinned version",
  damaged_runtime: "Needs reinstalling",
  needs_daw_component: "Needs its DAW part",
  no_pin: "No pinned release in this plugin",
  unsupported_platform: "Not available on this system",
  unknown: "Unknown"
};
function Line({ label, children }) {
  return /* @__PURE__ */ jsxs9("div", { style: { display: "grid", gridTemplateColumns: "9rem 1fr", gap: 8, alignItems: "baseline", padding: "5px 0", borderTop: `1px solid ${T.border}` }, children: [
    /* @__PURE__ */ jsx9("span", { style: S.muted, children: label }),
    /* @__PURE__ */ jsx9("span", { style: { ...S.text, minWidth: 0, overflowWrap: "anywhere" }, children })
  ] });
}
function megabytes(bytes) {
  return typeof bytes === "number" && bytes > 0 ? `${(bytes / 1048576).toFixed(0)} MB` : null;
}
function SettingsPage({ ctx, store, onOpenMap }) {
  useWant(store, "setup");
  const state = useKumi(store);
  const [openOnFind, setOpenOnFind] = useState5(() => ctx.storage.get(OPEN_MAP_ON_FIND, true));
  const setup = state.setup;
  const step = nextStep(state);
  const daw = state.status?.daw ?? setup?.daw;
  const runtime = setup?.runtime;
  const component = setup?.daw_component;
  return /* @__PURE__ */ jsxs9("div", { style: { display: "flex", flexDirection: "column", gap: 18, maxWidth: 640, color: T.text }, children: [
    /* @__PURE__ */ jsxs9("div", { style: { display: "flex", flexDirection: "column", gap: 4 }, children: [
      /* @__PURE__ */ jsx9("span", { style: { fontSize: 15, fontWeight: 600 }, children: "Kumi" }),
      /* @__PURE__ */ jsxs9("span", { style: S.muted, children: [
        "Kumi reads and changes your ",
        dawName(daw),
        " project through typed changes you approve, keeps HISTORY with undo, measures audio on this computer, and maps the sample folders you allow."
      ] })
    ] }),
    state.available === false ? /* @__PURE__ */ jsx9("span", { style: S.text, children: "Kumi's agent half is off in this profile. Turn on the kumi plugin's Agent switch in Capabilities → Plugins and restart the gateway; its settings form appears under Agent settings." }) : /* @__PURE__ */ jsxs9(Fragment3, { children: [
      /* @__PURE__ */ jsxs9("div", { children: [
        /* @__PURE__ */ jsx9("span", { style: { ...S.heading, display: "block", marginBottom: 4 }, children: "Setup" }),
        /* @__PURE__ */ jsx9(Line, { label: "State", children: STATE_WORDS[setup?.state ?? "unknown"] ?? clean(setup?.state ?? "", 40) }),
        setup?.message && /* @__PURE__ */ jsx9(Line, { label: "Next", children: clean(setup.message, 300) }),
        runtime && /* @__PURE__ */ jsxs9(Line, { label: "Kumi runtime", children: [
          runtime.installed ? `Kumi ${clean(runtime.version ?? "", 20)}` : "Not installed",
          setup?.pinned_version && /* @__PURE__ */ jsxs9("span", { style: S.faint, children: [
            " · pinned ",
            clean(setup.pinned_version, 20)
          ] }),
          setup?.target && /* @__PURE__ */ jsxs9("span", { style: S.faint, children: [
            " · ",
            clean(setup.target, 40)
          ] })
        ] }),
        runtime && !runtime.installed && megabytes(setup?.download_bytes) && /* @__PURE__ */ jsxs9(Line, { label: "Download", children: [
          megabytes(setup?.download_bytes),
          megabytes(setup?.unpacked_bytes) && /* @__PURE__ */ jsxs9("span", { style: S.faint, children: [
            " (",
            megabytes(setup?.unpacked_bytes),
            " unpacked)"
          ] })
        ] }),
        component && /* @__PURE__ */ jsxs9(Line, { label: `${dawName(daw)} part`, children: [
          clean(component.name ?? "DAW component", 40),
          ": ",
          clean(component.state ?? "unknown", 20).replace("_", " "),
          component.path && /* @__PURE__ */ jsx9("div", { style: { ...S.faint, marginTop: 2 }, children: clean(component.path, 200) })
        ] }),
        setup?.existing_kumi?.path && /* @__PURE__ */ jsxs9(Line, { label: "Standalone Kumi", children: [
          clean(setup.existing_kumi.path, 120),
          /* @__PURE__ */ jsx9("span", { style: S.faint, children: setup.existing_kumi.adopted ? " · adopted" : " · left as it is" })
        ] })
      ] }),
      /* @__PURE__ */ jsxs9("div", { children: [
        /* @__PURE__ */ jsx9("span", { style: { ...S.heading, display: "block", marginBottom: 4 }, children: "Connection" }),
        /* @__PURE__ */ jsx9(Line, { label: "Now", children: /* @__PURE__ */ jsx9(Chip, { tone: state.status?.connected ? "good" : state.status?.stale && state.status?.owner ? "warn" : void 0, children: chipWords(state.status, state.focus?.transport?.tempo).text }) }),
        state.status?.owner?.session_id && /* @__PURE__ */ jsx9(Line, { label: "Kumi's chat", children: clean(state.status.owner.session_id, 80) })
      ] }),
      /* @__PURE__ */ jsxs9("div", { style: { ...S.row, gap: 8, flexWrap: "wrap" }, children: [
        step && /* @__PURE__ */ jsx9(
          TextButton,
          {
            onClick: () => void ask(ctx, step.request, { send: true, session: state.status?.connected ? ownerSession(state) : null }),
            strong: true,
            children: step.label
          }
        ),
        /* @__PURE__ */ jsx9(TextButton, { onClick: onOpenMap, children: "Open the sound map" }),
        /* @__PURE__ */ jsx9(TextButton, { onClick: () => host4.navigate("/settings?tab=plugins&agent=kumi"), children: "Agent settings" })
      ] })
    ] }),
    /* @__PURE__ */ jsxs9("div", { children: [
      /* @__PURE__ */ jsx9("span", { style: { ...S.heading, display: "block", marginBottom: 4 }, children: "This window" }),
      /* @__PURE__ */ jsxs9("label", { style: { ...S.row, gap: 8, padding: "5px 0", borderTop: `1px solid ${T.border}`, cursor: "pointer" }, children: [
        /* @__PURE__ */ jsx9(
          "input",
          {
            checked: openOnFind,
            onChange: (event) => {
              setOpenOnFind(event.target.checked);
              ctx.storage.set(OPEN_MAP_ON_FIND, event.target.checked);
            },
            type: "checkbox"
          }
        ),
        /* @__PURE__ */ jsx9("span", { style: S.text, children: "Open the sound map when Kumi finds sounds" })
      ] })
    ] })
  ] });
}

// src/desktop/plugin.tsx
import { jsx as jsx10 } from "react/jsx-runtime";
var PLUGIN_ID = "kumi";
var MAP_WORKSPACE = "kumi-sound-map";
var POLL_MS = 5e3;
var plugin = {
  id: PLUGIN_ID,
  name: "Kumi",
  description: "Your DAW beside the chat: FOCUS, HISTORY with undo, a sound map of your sample folders, and Kumi's audio measurements.",
  register(ctx) {
    const store = new KumiStore((path) => ctx.rest(path, { timeoutMs: 15e3 }));
    const audition = new Audition((id) => ctx.rest(`/audio?id=${encodeURIComponent(id)}`, { timeoutMs: 6e4 }));
    let closeMap = null;
    let mapPane = null;
    const openMap = () => {
      if (typeof host5.openWorkspace === "function") {
        closeMap = host5.openWorkspace(MAP_WORKSPACE, {
          title: "Sound map",
          minWidth: "26rem",
          render: () => /* @__PURE__ */ jsx10(MapWorkspace, { audition, ctx, store }),
          onClose: () => {
            closeMap = null;
          }
        });
        return;
      }
      mapPane ??= ctx.register({
        id: "map-pane",
        area: PANES_AREA,
        title: "Sound map",
        data: { placement: "main" },
        render: () => /* @__PURE__ */ jsx10(MapWorkspace, { audition, ctx, store })
      });
    };
    const openSettings = () => host5.navigate(typeof sdk.pluginSettingsHref === "function" ? sdk.pluginSettingsHref(PLUGIN_ID) : "/settings?tab=plugins");
    const refresh = () => void store.refreshStatus();
    refresh();
    ctx.setInterval(() => {
      if (document.visibilityState !== "hidden") refresh();
    }, POLL_MS);
    ctx.onEvent(`plugin.${PLUGIN_ID}.status.changed`, refresh);
    ctx.onEvent(`plugin.${PLUGIN_ID}.focus.changed`, () => void store.refresh("focus"));
    ctx.onEvent(`plugin.${PLUGIN_ID}.history.changed`, () => void store.refresh("history"));
    ctx.onEvent(`plugin.${PLUGIN_ID}.map.changed`, () => void store.refresh("map"));
    ctx.onEvent(`plugin.${PLUGIN_ID}.job.progress`, refresh);
    ctx.onEvent(`plugin.${PLUGIN_ID}.settings.changed`, () => {
      refresh();
      void store.refresh("setup");
    });
    ctx.onEvent("tool.complete", (event) => {
      const payload = event.payload && typeof event.payload === "object" ? event.payload : {};
      const name = "name" in payload && typeof payload.name === "string" ? payload.name : "";
      if (!name.startsWith("kumi_")) return;
      refresh();
      if (name !== "kumi_sounds") return;
      const found = foundSounds("result" in payload ? payload.result : void 0);
      if (!found?.ids.length) return;
      store.found(found.revision, found.ids);
      if (ctx.storage.get(OPEN_MAP_ON_FIND, true)) openMap();
    });
    for (const atom of [host5.state.profile, host5.state.connectionId]) {
      const stop = atom?.listen?.(() => {
        store.reset();
        audition.reset();
        refresh();
      });
      if (stop) ctx.onDispose(stop);
    }
    ctx.onDispose(() => {
      closeMap?.();
      audition.dispose();
    });
    const contributions = [
      {
        id: "pane",
        area: PANES_AREA,
        title: "Kumi",
        data: { placement: "right", width: "320px" },
        render: () => /* @__PURE__ */ jsx10(KumiPane, { ctx, onOpenMap: openMap, store })
      },
      {
        id: "status",
        area: STATUSBAR_AREAS.right,
        order: 60,
        render: () => /* @__PURE__ */ jsx10(StatusChip, { ctx, onOpenMap: openMap, onOpenSettings: openSettings, store })
      },
      {
        id: "open-map",
        area: PALETTE_AREA,
        data: { id: "kumi.map", label: "Kumi: open the sound map", keywords: ["kumi", "sound", "samples", "map", "library"], run: openMap }
      },
      {
        id: "undo-last",
        area: PALETTE_AREA,
        data: {
          id: "kumi.undo-last",
          label: "Kumi: undo the last change",
          keywords: ["kumi", "undo", "history", "revert"],
          run: () => {
            const state = store.get();
            const last = state.history?.items.find((item) => item.state !== "undone");
            if (!last || !canUndo(state.status, last)) {
              const unsure = Boolean(state.status?.unsure) || last?.state === "unsure" || last?.state === "partial";
              host5.notify({
                kind: "info",
                message: unsure ? "Kumi isn't sure of its last change: ask it to look at your project again first." : "Kumi has no change it can undo right now."
              });
              return;
            }
            void ask(ctx, undoRequest(last), { send: true, session: ownerSession(state) });
          }
        }
      },
      {
        id: "settings",
        area: PALETTE_AREA,
        data: { id: "kumi.settings", label: "Kumi: settings", keywords: ["kumi", "setup", "settings", "daw"], run: openSettings }
      }
    ];
    if (typeof sdk.TOOL_MEDIA_RENDERER_AREA === "string") {
      contributions.push({
        id: "listen-card",
        area: sdk.TOOL_MEDIA_RENDERER_AREA,
        data: {
          tools: ["kumi_listen"],
          render: (props) => /* @__PURE__ */ jsx10(ListenCard, { audition, media: props })
        }
      });
    }
    if (typeof sdk.COMPOSER_AREAS?.underside === "string") {
      contributions.push({ id: "focus-hint", area: sdk.COMPOSER_AREAS.underside, render: () => /* @__PURE__ */ jsx10(FocusHint, { store }) });
    }
    ctx.registerMany(contributions);
    ctx.registerSettingsPage?.({
      id: "kumi",
      title: "Kumi",
      icon: "pulse",
      render: () => /* @__PURE__ */ jsx10(SettingsPage, { ctx, onOpenMap: openMap, store })
    });
  }
};
var plugin_default = plugin;
export {
  MAP_WORKSPACE,
  PLUGIN_ID,
  POLL_MS,
  plugin_default as default
};
