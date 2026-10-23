// The socket the bridge host connects to. It speaks the Remote Script's protocol: a signed hello with
// the registry hash, then signed requests (status, invoke) answered by id, and signed events pushed
// to every connection. There is no preflight/prepare handshake here: the shared secret is the
// authority, and only Kumi's host holds it.
import { createServer, type Server, type Socket } from "node:net";
import { hasOperation, REGISTRY_HASH, validateRequest, validateResult } from "./registry.js";
import { LIVE_PROTOCOL, LOOPBACK_PROTOCOL, MAX_FRAME_BYTES, signed, token, verify } from "./wire.js";

export interface Handlers {
  operations: readonly string[];
  status(): Record<string, unknown>;
  invoke(operation: string, args: Record<string, unknown>): Promise<Record<string, unknown>>;
}

const REQUIRED = ["version", "id", "method", "nonce", "sequence", "bridgeEpoch", "connectionChallenge", "deadlineMs", "mac"];
const OPTIONAL = ["operation", "args", "ref", "transactionId", "idempotencyKey", "stateDigest", "ownershipToken"];
const ID = /^[A-Za-z0-9_-]{1,128}$/;
// Until a connection's first signed request, any local process may be on the other end: its lines stay small, it
// gets a while to sign in, and there are only so many connections.
const UNSIGNED_FRAME_BYTES = 64 * 1024;
const SIGN_IN_WITHIN_MS = 10_000;
const MAX_CONNECTIONS = 16;

interface Connection { socket: Socket; challenge: string; lastSequence: number; eventSequence: number; pieces: Buffer[]; buffered: number; signedIn: boolean; signIn?: NodeJS.Timeout }

export class ExtensionServer {
  readonly bridgeEpoch = token(24);
  /** This process's own epoch for events; the host checks references against the Remote Script's. */
  readonly epoch = 1 + Math.floor(Math.random() * (Number.MAX_SAFE_INTEGER - 1));
  private readonly server: Server;
  private readonly connections = new Set<Connection>();
  // Changes to the Set go one at a time, in the order they arrive.
  private queue: Promise<unknown> = Promise.resolve();

  constructor(private readonly secret: string, private readonly handlers: Handlers, private readonly log: (line: string) => void = () => undefined) {
    this.server = createServer((socket) => this.accept(socket));
  }

  listen(port = 0): Promise<number> {
    return new Promise((resolve, reject) => {
      this.server.once("error", reject);
      this.server.listen(port, "127.0.0.1", () => { const address = this.server.address(); resolve(typeof address === "object" && address ? address.port : port); });
    });
  }

  close(): Promise<void> {
    for (const connection of this.connections) connection.socket.destroy();
    return new Promise((resolve) => this.server.close(() => resolve()));
  }

  get clients(): number { return this.connections.size; }

  /** An event for every connected host, each numbered in its connection's own sequence. */
  broadcast(type: string, payload: Record<string, unknown>, ref?: string): void {
    for (const connection of this.connections) {
      connection.eventSequence += 1;
      const event = { epoch: this.epoch, sequence: connection.eventSequence, type, ...(ref ? { ref } : {}), payload };
      this.send(connection, { version: LOOPBACK_PROTOCOL, id: "event", ok: true, bridgeEpoch: this.bridgeEpoch, connectionChallenge: connection.challenge, result: { event } });
    }
  }

  private accept(socket: Socket): void {
    if (this.connections.size >= MAX_CONNECTIONS) { socket.destroy(); return; }
    socket.setNoDelay(true);
    const connection: Connection = { socket, challenge: token(24), lastSequence: 0, eventSequence: 0, pieces: [], buffered: 0, signedIn: false };
    connection.signIn = setTimeout(() => { if (!connection.signedIn) socket.destroy(); }, SIGN_IN_WITHIN_MS);
    connection.signIn.unref();
    this.connections.add(connection);
    socket.on("data", (chunk: Buffer) => this.onData(connection, chunk));
    socket.on("error", () => undefined);
    socket.on("close", () => { clearTimeout(connection.signIn); this.connections.delete(connection); });
    this.send(connection, { version: LOOPBACK_PROTOCOL, id: "hello", ok: true, bridgeEpoch: this.bridgeEpoch, connectionChallenge: connection.challenge, result: { protocol: LIVE_PROTOCOL, registryHash: REGISTRY_HASH, maxDeadlineMs: 600_000 } });
  }

  private send(connection: Connection, payload: Record<string, unknown>): void {
    if (connection.socket.destroyed) return;
    connection.socket.write(`${JSON.stringify(signed(this.secret, payload))}\n`);
  }

  private onData(connection: Connection, chunk: Buffer): void {
    // A request arrives in pieces: keep them until one holds a line end, then join once.
    if (connection.socket.destroyed) return;
    const bound = () => connection.signedIn ? MAX_FRAME_BYTES : UNSIGNED_FRAME_BYTES;
    connection.pieces.push(chunk); connection.buffered += chunk.length;
    if (chunk.indexOf(10) < 0) {
      if (connection.buffered > bound()) connection.socket.destroy();
      return;
    }
    let buffer = Buffer.concat(connection.pieces); connection.pieces = []; connection.buffered = 0;
    for (let index = buffer.indexOf(10); index >= 0; index = buffer.indexOf(10)) {
      const line = buffer.subarray(0, index); buffer = buffer.subarray(index + 1);
      // Checked as each line comes: the first signed one lifts the bound for those after it.
      if (line.length > bound()) { connection.socket.destroy(); return; }
      // Nothing a frame does may reject unhandled: Node would end the whole Extension Host for it.
      if (line.length > 0) this.onFrame(connection, line.toString("utf8")).catch((error: unknown) => {
        this.log(`a request failed unexpectedly: ${error instanceof Error ? error.message : String(error)}`);
        this.error(connection, "invalid", "request failed");
      });
    }
    if (buffer.length > 0) { connection.pieces.push(buffer); connection.buffered = buffer.length; }
  }

  private error(connection: Connection, id: unknown, message: string): void {
    this.send(connection, { version: LOOPBACK_PROTOCOL, id: typeof id === "string" && ID.test(id) ? id : "invalid", ok: false, bridgeEpoch: this.bridgeEpoch, connectionChallenge: connection.challenge, error: message });
  }

  private async onFrame(connection: Connection, text: string): Promise<void> {
    let request: Record<string, unknown>;
    try { request = JSON.parse(text) as Record<string, unknown>; } catch { this.error(connection, "invalid", "malformed request"); return; }
    // Only an object can be a request (Object.keys(null) throws).
    if (typeof request !== "object" || request === null || Array.isArray(request)) { this.error(connection, "invalid", "malformed request"); return; }
    const keys = Object.keys(request);
    const now = Date.now();
    if (!REQUIRED.every((key) => keys.includes(key)) || keys.some((key) => !REQUIRED.includes(key) && !OPTIONAL.includes(key))
      || request.version !== LOOPBACK_PROTOCOL || request.bridgeEpoch !== this.bridgeEpoch || request.connectionChallenge !== connection.challenge
      || typeof request.id !== "string" || !ID.test(request.id) || typeof request.deadlineMs !== "number" || !(request.deadlineMs >= now && request.deadlineMs <= now + 600_000)
      || typeof request.nonce !== "string" || request.nonce.length < 16 || request.nonce.length > 256
      || typeof request.sequence !== "number" || !Number.isSafeInteger(request.sequence) || request.sequence <= connection.lastSequence) {
      this.error(connection, request.id, "invalid request"); return;
    }
    if (!verify(this.secret, request)) { this.error(connection, request.id, "authentication or replay check failed"); return; }
    if (!connection.signedIn) { connection.signedIn = true; clearTimeout(connection.signIn); }
    connection.lastSequence = request.sequence as number;
    const id = request.id;
    try {
      let result: Record<string, unknown>;
      if (request.method === "status") result = this.handlers.status();
      else if (request.method === "invoke" || request.method === "mutate") {
        const operation = String(request.operation ?? "");
        const args = (request.args ?? {}) as Record<string, unknown>;
        if (!hasOperation(operation) || !this.handlers.operations.includes(operation)) throw new Error(`operation unavailable on the Extensions channel: ${operation}`);
        validateRequest(operation, args);
        const deadline = request.deadlineMs as number;
        const run = this.queue.then(() => {
          // Queued behind other changes past its deadline: the host has given up on it, so it mustn't happen late.
          if (Date.now() > deadline) throw new Error("the request's deadline passed before Live could start it; nothing changed");
          return this.handlers.invoke(operation, args);
        });
        this.queue = run.catch(() => undefined);
        result = await run;
        validateResult(operation, result);
      } else throw new Error(`method unavailable on the Extensions channel: ${String(request.method)}`);
      this.send(connection, { version: LOOPBACK_PROTOCOL, id, ok: true, bridgeEpoch: this.bridgeEpoch, connectionChallenge: connection.challenge, result });
    } catch (error) {
      const message = error instanceof Error ? error.message : error === undefined || error === null ? "Live refused it without giving a reason" : String(error);
      this.log(`request ${id} failed: ${message}`);
      this.error(connection, id, message.slice(0, 1024));
    }
  }
}
