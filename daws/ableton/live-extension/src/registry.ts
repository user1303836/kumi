// The operation registry both channels share (protocol/ableton-live-v1.operations.json), embedded in
// the bundle. The host refuses a channel whose registry hash differs from its own.
import { createHash } from "node:crypto";
import registryJson from "../../../protocol/ableton-live-v1.operations.json";

type Schema = Record<string, unknown>;
interface Operation { id: string; method: string; request: Schema; result: Schema }

const registry = registryJson as { version: number; protocol: string; operations: Operation[] };
const byId = new Map(registry.operations.map((operation) => [operation.id, operation]));

function canonicalRegistry(value: unknown): string {
  if (value === null || typeof value !== "object") return JSON.stringify(value);
  if (Array.isArray(value)) return `[${value.map(canonicalRegistry).join(",")}]`;
  const object = value as Record<string, unknown>;
  return `{${Object.keys(object).sort().map((key) => `${JSON.stringify(key)}:${canonicalRegistry(object[key])}`).join(",")}}`;
}

export const REGISTRY_HASH = createHash("sha256").update(canonicalRegistry(registry)).digest("hex");

export function hasOperation(id: string): boolean { return byId.has(id); }

function matchesType(value: unknown, type: string): boolean {
  if (type === "null") return value === null;
  if (type === "object") return typeof value === "object" && value !== null && !Array.isArray(value);
  if (type === "array") return Array.isArray(value);
  if (type === "integer") return typeof value === "number" && Number.isSafeInteger(value);
  if (type === "number") return typeof value === "number" && Number.isFinite(value);
  return typeof value === type;
}

/** The bridge's validate_registry_value (crates/ableton-mcp-server/src/registry.rs), for one value against one schema. */
export function validate(schema: Schema, value: unknown, path = "$"): void {
  const declared = Array.isArray(schema.type) ? schema.type as string[] : [schema.type as string];
  if (!declared.some((type) => matchesType(value, type))) throw new Error(`${path} does not match registry type`);
  if (schema.const !== undefined && value !== schema.const) throw new Error(`${path} does not match registry constant`);
  if (Array.isArray(schema.enum) && !schema.enum.some((item) => item === value)) throw new Error(`${path} is outside registry enum`);
  if (typeof value === "string") {
    if (typeof schema.minLength === "number" && value.length < schema.minLength) throw new Error(`${path} is shorter than registry minimum`);
    if (typeof schema.maxLength === "number" && value.length > schema.maxLength) throw new Error(`${path} exceeds registry maximum`);
    if (typeof schema.pattern === "string" && !new RegExp(schema.pattern).test(value)) throw new Error(`${path} does not match registry pattern`);
  }
  if (typeof value === "number" && ((typeof schema.minimum === "number" && value < schema.minimum) || (typeof schema.maximum === "number" && value > schema.maximum))) throw new Error(`${path} is outside registry numeric bounds`);
  if (Array.isArray(value)) {
    if (typeof schema.minItems === "number" && value.length < schema.minItems) throw new Error(`${path} is below registry item bound`);
    if (typeof schema.maxItems === "number" && value.length > schema.maxItems) throw new Error(`${path} exceeds registry item bound`);
    value.forEach((item, index) => validate(schema.items as Schema, item, `${path}[${index}]`));
  }
  if (typeof value === "object" && value !== null && !Array.isArray(value)) {
    const object = value as Record<string, unknown>;
    const properties = (schema.properties ?? {}) as Record<string, Schema>;
    if (typeof schema.maxProperties === "number" && Object.keys(object).length > schema.maxProperties) throw new Error(`${path} exceeds registry property bound`);
    for (const required of (schema.required ?? []) as string[]) if (!(required in object)) throw new Error(`${path}.${required} is required by registry`);
    for (const key of Object.keys(object)) {
      if (key in properties) validate(properties[key]!, object[key], `${path}.${key}`);
      else if (schema.additionalProperties === false) throw new Error(`${path}.${key} is not allowed by registry`);
      else if (typeof schema.additionalProperties === "object") validate(schema.additionalProperties as Schema, object[key], `${path}.${key}`);
    }
  }
}

export function validateRequest(id: string, value: unknown): void {
  const operation = byId.get(id); if (!operation) throw new Error(`operation is not in the registry: ${id}`);
  validate(operation.request, value, `${id}.request`);
}

export function validateResult(id: string, value: unknown): void {
  const operation = byId.get(id); if (!operation) throw new Error(`operation is not in the registry: ${id}`);
  validate(operation.result, value, `${id}.result`);
}
