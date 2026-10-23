// Run the retained production extension against its fake Live to test the native bridge's actual wire.
import { createRequire } from "node:module";
import { createInterface } from "node:readline";
import { join } from "node:path";
import { pathToFileURL } from "node:url";
const [storage, repository] = process.argv.slice(2);
const directory = join(repository, "apps", "live-extension");
const { fakeLive } = await import(pathToFileURL(join(directory, "test", "fake-live.mjs")));
const fake = fakeLive({ storage, temp: join(storage, "temp"), liveTemp: join(storage, "live-temp") });
const extension = createRequire(import.meta.url)(join(directory, "dist", "extension.js"));
console.log = (...args) => console.error(...args);
extension.activate(fake.activation);
createInterface({ input: process.stdin }).on("line", (line) => {
  const command = JSON.parse(line);
  if ("delay" in command) fake.model.renderDelayMs = command.delay;
  if (command.point) fake.model.commands.get("kumi.point")(fake.model.handle(fake.model[command.point]));
  process.stdout.write("ok\n");
});
process.on("SIGTERM", async () => { await extension.deactivate(); process.exit(0); });
