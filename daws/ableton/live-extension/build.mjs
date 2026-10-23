// Builds dist/extension.js: Kumi's extension source with the Extensions SDK and the operation registry
// in one CommonJS file, as Live's Extension Host loads it. The SDK's licence forbids redistributing
// the SDK itself, so it comes from a local copy in vendor/ (never committed); the built bundle is
// committed, with its SHA-256, and CI checks the bundle against that when it can't rebuild it.
import { createHash } from "node:crypto";
import { existsSync, readFileSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const sdk = join(here, "..", "..", "vendor", "ableton-extensions-sdk-1.0.0-beta.1", "package 3", "dist", "index.cjs");
if (!existsSync(sdk)) {
  console.error(`The Extensions SDK isn't here (${sdk}); the committed dist/extension.js stays as it is.`);
  process.exit(1);
}
const { build } = await import("esbuild");
const manifest = JSON.parse(readFileSync(join(here, "manifest.json"), "utf8"));
const outfile = join(here, "dist", "extension.js");
await build({
  entryPoints: [join(here, "src", "extension.ts")],
  outfile,
  bundle: true,
  format: "cjs",
  platform: "node",
  target: "node22",
  alias: { "@ableton-extensions/sdk": sdk },
  define: { __KUMI_EXTENSION_VERSION__: JSON.stringify(manifest.version) },
  legalComments: "none",
  sourcesContent: false,
  logLevel: "info",
});
const sha = createHash("sha256").update(readFileSync(outfile)).digest("hex");
writeFileSync(join(here, "dist", "extension.js.sha256"), `${sha}  extension.js\n`);
console.log(`dist/extension.js ${sha}`);
