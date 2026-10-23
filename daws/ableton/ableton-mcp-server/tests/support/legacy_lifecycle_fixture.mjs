// The fixture helpers of the last JavaScript bridge lifecycle test (apps/mcp-server/test/lifecycle.test.ts
// at v1.7.6), compiled once to JavaScript. legacy_install.mjs points the ../src imports at the
// published Kumi 1.7.5 bundle, so the old installation comes from the code that shipped.
import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { gzipSync } from "node:zlib";
import { spawnSync } from "node:child_process";
import { chmodSync, existsSync, lstatSync, mkdirSync, mkdtempSync, readFileSync, renameSync, rmSync, symlinkSync, truncateSync, writeFileSync } from "node:fs";
import { createServer } from "node:net";
import { tmpdir } from "node:os";
import { dirname, join, parse, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import test from "node:test";
import { secretPermissions, writeSecretFile } from "../src/delivery.js";
import { LIVE_REGISTRY_HASH } from "../src/live.js";
import { assertNoLinkedAncestors, runLifecycle } from "../src/lifecycle.js";
const sha = (value) => createHash("sha256").update(value).digest("hex");
const mitLicense = readFileSync(new URL("../../../../LICENSE.md", import.meta.url), "utf8");
// The registry the bundle's bridge was built with, carried as a release carries it (its manifest names its hash).
const legacyRegistry = readFileSync(new URL("../../../../protocol/ableton-live-v1.operations.json", import.meta.url), "utf8");
const artifacts = new Map();
function createArtifact(path, manifest, packageRoot, extras = {}) {
    const parsed = JSON.parse(manifest.toString("utf8"));
    const names = [...new Set(["package/package.json", "package/release-manifest.json", ...Object.keys(parsed.files).map((name) => `package/${name}`), ...Object.keys(extras)])].sort();
    const chunks = [];
    for (const name of names) {
        const content = name in extras ? Buffer.from(extras[name]) : name === "package/release-manifest.json" ? manifest : readFileSync(join(packageRoot, name.slice("package/".length)));
        const header = Buffer.alloc(512);
        const write = (value, offset, length) => header.write(value.slice(0, length), offset, "ascii");
        write(name, 0, 100);
        write("0000600\0", 100, 8);
        write("0000000\0", 108, 8);
        write("0000000\0", 116, 8);
        write(`${content.length.toString(8).padStart(11, "0")}\0`, 124, 12);
        write("00000000000\0", 136, 12);
        header.fill(0x20, 148, 156);
        header[156] = "0".charCodeAt(0);
        write("ustar\0", 257, 6);
        write("00", 263, 2);
        const checksum = [...header].reduce((sum, value) => sum + value, 0);
        write(`${checksum.toString(8).padStart(6, "0")}\0 `, 148, 8);
        chunks.push(header, content, Buffer.alloc(Math.ceil(content.length / 512) * 512 - content.length));
    }
    writeFileSync(path, gzipSync(Buffer.concat([...chunks, Buffer.alloc(1024)])));
}
function fixturePackage(root, version, marker, policy = "current") {
    const packageRoot = join(root, `candidate ${version} ü ${policy}`);
    const legacy = policy === "legacy";
    const license = legacy ? "UNLICENSED" : "MIT";
    const nodeMajors = policy === "node25" ? [22, 24, 25] : [22, 24];
    const nodeRange = nodeMajors.map((major) => `>=${major} <${major + 1}`).join(" || ");
    const packageMetadata = `${JSON.stringify({ name: "@ableton-mcp/mcp-server", version, private: true, license, type: "module", ...(!legacy ? { engines: { node: nodeRange }, abletonMcpSupport: { nodeMajors } } : {}) })}\n`;
    const files = new Map([
        ["package.json", packageMetadata],
        ["LICENSE.md", !legacy ? mitLicense : "# Legacy private license notice\n"],
        ["dist/src/cli.js", `#!/usr/bin/env node\n// ${marker}\n`],
        ["remote-script/AbletonMcpBridge/__init__.py", "def create_instance(c_instance):\n    return None\n"],
        ["remote-script/AbletonMcpBridge/ableton_mcp_remote_script.py", `class AbletonMcpBridge:\n    marker = ${JSON.stringify(marker)}\n`],
        ["remote-script/AbletonMcpBridge/ableton-live-v1.operations.json", legacyRegistry],
        ...Array.from({ length: 9 }, (_, index) => [`release-docs/doc-${index}.md`, `# ${marker} ${index}\n`]),
    ]);
    for (const [name, content] of files) {
        const path = join(packageRoot, ...name.split("/"));
        mkdirSync(dirname(path), { recursive: true });
        writeFileSync(path, content);
    }
    const manifest = {
        schema: !legacy ? "ableton-mcp-release/v2" : "ableton-mcp-private-release/v1",
        package: { name: "@ableton-mcp/mcp-server", version, license, private: true },
        source: { commit: sha(marker).slice(0, 40), dirty: true },
        build: { runtime: "TypeScript compiled JavaScript", nodeRange: !legacy ? nodeRange : ">=22 <26", ...(!legacy ? { nodeMajors } : {}), recipe: "test fixture", builder: { node: process.versions.node, npm: "fixture", typescript: "fixture", platform: process.platform, architecture: process.arch, runnerImage: "fixture", runnerImageVersion: "fixture", packageLockSha256: "a".repeat(64), workflowSha256: "b".repeat(64) } },
        protocol: { registryHash: LIVE_REGISTRY_HASH },
        distribution: { channel: !legacy ? "local-npm-tarball" : "private-local-npm-tarball", published: false, signed: false, notarized: false, integrityIsIdentityProof: false },
        algorithm: "sha256",
        files: Object.fromEntries([...files].map(([name, content]) => [name, sha(content)])),
        roles: Object.fromEntries([...files.keys()].map((name) => [name, name === "LICENSE.md" ? !legacy ? "license" : "private-license" : name === "package.json" ? "package-metadata" : name.startsWith("dist/src/") ? "compiled-runtime" : name.startsWith("release-docs/") ? "documentation" : "ableton-remote-script"])),
    };
    const manifestBytes = Buffer.from(`${JSON.stringify(manifest)}\n`);
    writeFileSync(join(packageRoot, "release-manifest.json"), manifestBytes);
    const artifactPath = join(root, `candidate-${version}-${marker}-${policy}.tgz`);
    createArtifact(artifactPath, manifestBytes, packageRoot);
    artifacts.set(packageRoot, { path: artifactPath, sha256: sha(readFileSync(artifactPath)) });
    return packageRoot;
}
function artifactOptions(packageRoot) { return { artifactPath: artifacts.get(packageRoot).path, artifactSha256: artifacts.get(packageRoot).sha256 }; }
function rebindInstalledReceiptToPriorPackage(options, packageRoot) {
    const saved = receipt(options);
    const manifestPath = join(packageRoot, "release-manifest.json");
    const manifest = JSON.parse(readFileSync(manifestPath, "utf8"));
    assert.ok(["ableton-mcp-private-release/v1", "ableton-mcp-release/v2"].includes(manifest.schema));
    const config = structuredClone(saved.config);
    config.server.args[0] = join(packageRoot, "dist", "src", "cli.js");
    writeFileSync(saved.configPath, `${JSON.stringify(config, null, 2)}\n`);
    Object.assign(saved, {
        packageRoot,
        packageVersion: manifest.package.version,
        artifactSha256: artifacts.get(packageRoot).sha256,
        releaseManifestSha256: sha(readFileSync(manifestPath)),
        registryHash: manifest.protocol.registryHash,
        config,
        configSha256: sha(readFileSync(saved.configPath)),
    });
    writeFileSync(join(options.stateDirectory, "install-receipt.json"), `${JSON.stringify(saved, null, 2)}\n`);
}
function lifecycleOptions(root, packageRoot, action, overrides = {}) {
    const remoteScriptsDirectory = join(root, "Live Remote Scripts ü");
    mkdirSync(remoteScriptsDirectory, { recursive: true });
    return {
        action,
        packageRoot,
        stateDirectory: join(root, "State ü space"),
        remoteScriptsDirectory,
        artifactPath: artifacts.get(packageRoot).path,
        artifactSha256: artifacts.get(packageRoot).sha256,
        host: "127.0.0.1",
        port: 19_765,
        realtimePort: 19_766,
        apply: true,
        confirmLiveStopped: true,
        allowDirtyPrivateBuild: true,
        ...overrides,
    };
}
function receipt(options) {
    return JSON.parse(readFileSync(join(options.stateDirectory, "install-receipt.json"), "utf8"));
}
async function freePort() {
    return await new Promise((resolvePromise, reject) => {
        const server = createServer();
        server.once("error", reject);
        server.listen(0, "127.0.0.1", () => {
            const address = server.address();
            const port = typeof address === "object" && address ? address.port : 0;
            server.close((error) => error ? reject(error) : resolvePromise(port));
        });
    });
}
async function withPorts(options) {
    const port = await freePort();
    let realtimePort = await freePort();
    while (realtimePort === port)
        realtimePort = await freePort();
    return { ...options, port, realtimePort };
}
