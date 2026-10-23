// What a rendered file holds, from its header: Live renders WAV or AIFF (its Record File Type setting).
import { closeSync, openSync, readSync, statSync } from "node:fs";

export interface AudioInfo { format: "wav" | "aiff"; channels: number; sampleRate: number; bitDepth: number | null; seconds: number; bytes: number }

function head(path: string, length: number): Buffer {
  const fd = openSync(path, "r");
  try { const buffer = Buffer.alloc(length); const read = readSync(fd, buffer, 0, length, 0); return buffer.subarray(0, read); }
  finally { closeSync(fd); }
}

/** An 80-bit IEEE 754 extended float, as AIFF stores its sample rate. */
function extended(buffer: Buffer, offset: number): number {
  const exponent = buffer.readUInt16BE(offset) & 0x7fff;
  const mantissa = buffer.readUInt32BE(offset + 2) * 2 ** 32 + buffer.readUInt32BE(offset + 6);
  if (exponent === 0 && mantissa === 0) return 0;
  return mantissa * 2 ** (exponent - 16383 - 63);
}

export function audioInfo(path: string): AudioInfo {
  const bytes = statSync(path).size;
  const buffer = head(path, Math.min(bytes, 1 << 16));
  const tag = buffer.toString("ascii", 0, 4); const kind = buffer.toString("ascii", 8, 12);
  if ((tag === "RIFF" || tag === "RF64") && kind === "WAVE") {
    let channels = 0; let sampleRate = 0; let bitDepth: number | null = null; let dataBytes = 0;
    for (let offset = 12; offset + 8 <= buffer.length;) {
      const id = buffer.toString("ascii", offset, offset + 4); const size = buffer.readUInt32LE(offset + 4);
      if (id === "fmt ") { channels = buffer.readUInt16LE(offset + 10); sampleRate = buffer.readUInt32LE(offset + 12); bitDepth = buffer.readUInt16LE(offset + 22); }
      if (id === "data") { dataBytes = size === 0xffffffff ? bytes - offset - 8 : size; break; }
      offset += 8 + size + (size & 1);
    }
    if (!channels || !sampleRate || !bitDepth) throw new Error("the render isn't a WAV file Kumi can read");
    return { format: "wav", channels, sampleRate, bitDepth, seconds: dataBytes / (channels * (bitDepth / 8) * sampleRate), bytes };
  }
  if (tag === "FORM" && (kind === "AIFF" || kind === "AIFC")) {
    for (let offset = 12; offset + 8 <= buffer.length;) {
      const id = buffer.toString("ascii", offset, offset + 4); const size = buffer.readUInt32BE(offset + 4);
      if (id === "COMM") {
        const channels = buffer.readUInt16BE(offset + 8); const frames = buffer.readUInt32BE(offset + 10);
        const bitDepth = buffer.readUInt16BE(offset + 14); const sampleRate = Math.round(extended(buffer, offset + 16));
        return { format: "aiff", channels, sampleRate, bitDepth, seconds: sampleRate ? frames / sampleRate : 0, bytes };
      }
      offset += 8 + size + (size & 1);
    }
  }
  throw new Error("the render is neither WAV nor AIFF");
}
