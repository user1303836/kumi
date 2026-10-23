// The loopback wire the Remote Script speaks (remote-script/ableton_mcp_remote_script.py): one JSON
// object per line, each signed with HMAC-SHA256 over its canonical text. The bridge host talks to
// this extension exactly as it talks to the Remote Script.
import { createHmac, randomBytes, timingSafeEqual } from "node:crypto";

export const LOOPBACK_PROTOCOL = "ableton-loopback/v1";
export const LIVE_PROTOCOL = "ableton-live/v1";
// The Remote Script's wire bounds (MAX_WIRE_BYTES and the rest).
export const MAX_FRAME_BYTES = 256 * 1_048_576;
const MAX_DEPTH = 256;
const MAX_STRING = 1_048_576;
const MAX_ARRAY = 10_000_000;
const MAX_KEYS = 1_000_000;

/** Canonical JSON: sorted keys, no spaces, -0 as 0; the text both ends sign. */
export function canonical(value: unknown, depth = 0): string {
  if (depth > MAX_DEPTH) throw new Error("wire payload is too deeply nested");
  if (value === null || typeof value === "boolean") return JSON.stringify(value);
  if (typeof value === "string") { if (value.length > MAX_STRING) throw new Error("wire string is too large"); return JSON.stringify(value); }
  if (typeof value === "number") { if (!Number.isFinite(value)) throw new Error("wire number is not finite"); return JSON.stringify(Object.is(value, -0) ? 0 : value); }
  if (Array.isArray(value)) { if (value.length > MAX_ARRAY) throw new Error("wire array is too large"); return `[${value.map((item) => canonical(item, depth + 1)).join(",")}]`; }
  if (typeof value === "object") {
    const object = value as Record<string, unknown>; const keys = Object.keys(object);
    if (keys.length > MAX_KEYS) throw new Error("wire object is too large");
    return `{${keys.sort().map((key) => `${JSON.stringify(key)}:${canonical(object[key], depth + 1)}`).join(",")}}`;
  }
  throw new Error("unsupported wire value");
}

export function sign(secret: string, payload: object): string {
  const text = canonical(payload);
  if (Buffer.byteLength(text) > MAX_FRAME_BYTES) throw new Error("wire payload is too large");
  return createHmac("sha256", secret).update(text).digest("base64url");
}

export function signed<T extends object>(secret: string, payload: T): T & { mac: string } {
  return { ...payload, mac: sign(secret, payload) };
}

/** True when `mac` signs everything else in the frame. */
export function verify(secret: string, frame: Record<string, unknown>): boolean {
  const { mac, ...unsigned } = frame;
  if (typeof mac !== "string") return false;
  let expected: Buffer;
  try { expected = Buffer.from(sign(secret, unsigned)); } catch { return false; }
  const received = Buffer.from(mac);
  return expected.length === received.length && timingSafeEqual(expected, received);
}

export function token(bytes = 18): string { return randomBytes(bytes).toString("base64url"); }
