// Create the old installation with the last JavaScript bridge's own lifecycle, from the published
// Kumi 1.7.5 bundle (input.bundle): its source fixture package/archive, receipt, config, secret,
// and managed Live assets. The fixture writes the preexisting user-owned secret with the bundle's
// secure writer too: a mode of 0600 alone does not remove inherited Windows access rules.
import fs from 'node:fs';import path from 'node:path';import {pathToFileURL} from 'node:url';
const input=JSON.parse(process.argv[2]),root=process.cwd();
const dist=path.join(input.bundle,'apps/mcp-server/dist/src');
let source=fs.readFileSync(new URL('./legacy_lifecycle_fixture.mjs',import.meta.url),'utf8');
for(const name of ['delivery','live','lifecycle'])source=source.replaceAll(`"../src/${name}.js"`,JSON.stringify(pathToFileURL(path.join(dist,`${name}.js`)).href));
source=source.replace('new URL("../../../../LICENSE.md", import.meta.url)',JSON.stringify(path.join(root,'LICENSE.md')));
source=source.replace('new URL("../../../../protocol/ableton-live-v1.operations.json", import.meta.url)',JSON.stringify(path.join(input.bundle,'protocol/ableton-live-v1.operations.json')));
source+=`
const input=${JSON.stringify(input)};
const packageRoot=fixturePackage(input.root,input.version,'actual-node-install');
if(input.clean){
 const manifestPath=join(packageRoot,'release-manifest.json');
 const manifest=JSON.parse(readFileSync(manifestPath,'utf8'));manifest.source.dirty=false;
 const bytes=Buffer.from(JSON.stringify(manifest)+'\\n');writeFileSync(manifestPath,bytes);
 const artifact=artifacts.get(packageRoot);createArtifact(artifact.path,bytes,packageRoot);
 artifact.sha256=sha(readFileSync(artifact.path));
}

const stateDirectory=join(input.root,'State ü space');
const custom=join(input.root,'Custom configuration ü');mkdirSync(custom,{recursive:true});chmodSync(custom,0o700);
const overrides=input.custom?{configPath:join(custom,'owner config.json'),secretPath:join(custom,'owner secret.key')}:{};
if(input.custom){writeSecretFile(overrides.secretPath,'a'.repeat(64));assert.equal(secretPermissions(overrides.secretPath),'owner-only');}
const options=await withPorts(lifecycleOptions(input.root,packageRoot,'install',{timeoutMs:1379,enableBridgeDiagnostics:true,allowDirtyPrivateBuild:!input.clean,...overrides}));
const installed=await runLifecycle(options);
export default {options,installed,receipt:receipt(options)};
`;
const {default:result}=await import('data:text/javascript;base64,'+Buffer.from(source).toString('base64'));
process.stdout.write(JSON.stringify(result));
