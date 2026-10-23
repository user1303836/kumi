// Kumi's Live extension: a thin shim over the Extensions SDK that the bridge host reaches over a
// signed loopback socket, as it reaches the Remote Script. Kumi's bridge starts Live's own Extension
// Host with it (or Live loads it, when a producer installs kumi.ablx); either way it writes where it
// listens into its storage directory, where the bridge looks.
import { chmodSync, existsSync, mkdirSync, readFileSync, renameSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { initialize, type ActivationContext } from "@ableton-extensions/sdk";
import { OPERATIONS, transactionGroup } from "./operations.js";
import { REGISTRY_HASH, validateRequest } from "./registry.js";
import { registerPointing } from "./pointing.js";
import { ExtensionServer } from "./server.js";
import { token } from "./wire.js";

declare const __KUMI_EXTENSION_VERSION__: string;
const VERSION = typeof __KUMI_EXTENSION_VERSION__ === "string" ? __KUMI_EXTENSION_VERSION__ : "0.0.0";
const OPERATION_IDS = ["status", ...Object.keys(OPERATIONS), "transaction.group"];

function log(line: string): void { console.log(`[kumi] ${line}`); }

/** The shared secret: the bridge writes it before starting the host; made here when Live loads kumi.ablx itself. */
function secretIn(storage: string): string {
  const path = join(storage, "secret");
  if (existsSync(path)) { const value = readFileSync(path, "utf8").trim(); if (value.length >= 32) return value; }
  const value = token(32);
  writeFileSync(path, `${value}\n`, { mode: 0o600 });
  return value;
}

function writeEndpoint(storage: string, endpoint: Record<string, unknown>): void {
  const path = join(storage, "endpoint.json"); const partial = `${path}.${process.pid}.tmp`;
  writeFileSync(partial, `${JSON.stringify(endpoint)}\n`, { mode: 0o600 });
  try { chmodSync(partial, 0o600); } catch { /* Windows keeps its own ACLs */ }
  renameSync(partial, path);
}

let running: { server: ExtensionServer; watchdog: NodeJS.Timeout | undefined; storage: string } | undefined;

/** Stops listening and forgets the endpoint (a host unloading the extension, or tests). */
export async function deactivate(): Promise<void> {
  const current = running; running = undefined;
  if (!current) return;
  clearInterval(current.watchdog);
  try { rmSync(join(current.storage, "endpoint.json"), { force: true }); } catch { /* already gone */ }
  await current.server.close();
}

export function activate(activation: ActivationContext): void {
  const context = initialize(activation, "1.0.0");
  const storage = context.environment.storageDirectory ?? join(tmpdir(), "kumi-live-extension");
  const temp = context.environment.tempDirectory ?? join(storage, "tmp");
  mkdirSync(storage, { recursive: true, mode: 0o700 });
  const environment = { rendersDir: join(temp, "renders") };
  const secret = secretIn(storage);
  const group = transactionGroup(validateRequest);
  const server: ExtensionServer = new ExtensionServer(secret, {
    operations: OPERATION_IDS,
    status: (): Record<string, unknown> => ({
      connected: true, adapter: "extension", epoch: server.epoch, protocol: "ableton-live/v1", registryHash: REGISTRY_HASH,
      operations: OPERATION_IDS, capabilities: [], provenance: "real-live",
      environment: { liveVersion: null, os: process.platform, api: "Live Extensions SDK" },
      extension: { version: VERSION, apiVersion: activation.hostApiVersion, pid: process.pid, storageDirectory: storage, tempDirectory: temp },
    }),
    invoke: async (operation, args) => {
      if (operation === "transaction.group") return group(context, args, environment);
      const run = OPERATIONS[operation];
      if (!run) throw new Error(`operation unavailable on the Extensions channel: ${operation}`);
      return run(context, args, environment);
    },
  }, log);
  void server.listen().then((port) => {
    writeEndpoint(storage, { version: 1, host: "127.0.0.1", port, pid: process.pid, extensionVersion: VERSION, registryHash: REGISTRY_HASH, apiVersion: activation.hostApiVersion, startedAt: Date.now() });
    log(`listening on 127.0.0.1:${port}; storage ${storage}; temp ${temp}`);
  }, (error: unknown) => log(`couldn't listen: ${error instanceof Error ? error.message : String(error)}`));
  void registerPointing(context, (payload) => server.broadcast("pointed", payload), log).catch((error: unknown) => log(`right-click actions unavailable: ${error instanceof Error ? error.message : String(error)}`));

  // A host Kumi's bridge started (it says so in the environment) outlives a Live that quit or crashed:
  // then it ends, so the next Live gets a fresh one and no stale endpoint misleads the bridge. Live's own
  // host ends with Live, and a read that fails meanwhile (a big Set loading) mustn't switch the
  // extension off for good, so there's no watch in it. Two minutes without an answer, not less: a big
  // Set can keep Live from answering that long while it loads.
  let misses = 0;
  const watchdog = process.env.KUMI_LAUNCHED_HOST === "1" ? setInterval(() => {
    try { void context.application.song.tempo; misses = 0; }
    catch {
      misses += 1;
      if (misses < 24) return;
      clearInterval(watchdog); log("Live is gone; stopping");
      try { rmSync(join(storage, "endpoint.json"), { force: true }); } catch { /* already gone */ }
      void server.close().finally(() => process.exit(0));
    }
  }, 5_000) : undefined;
  watchdog?.unref();
  running = { server, watchdog, storage };
}
