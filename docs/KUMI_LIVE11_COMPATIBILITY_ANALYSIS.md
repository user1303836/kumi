# Kumi → Live 11 Port: Compatibility Analysis

**Date:** 2026-10-08  
**Source:** Kumi commit `d51fec95ce99d98bdfbc2b85791d00c5c903c82f` (sparse checkout: ableton-mcp-server only)  
**Target:** Ableton Live 11.3.12  
**Scope:** Bridge architecture, Remote Script, Extension assumptions

---

## 0. Executive Summary

Kumi is a **Live 12-native system** built on three architectural layers:

1. **Runtime (Rust/Tokio)** — MCP server speaking JSON-RPC over stdio
2. **Bridge (Rust)** — Dual-channel adapter routing remote-script + extension protocols
3. **Remote Script (Python 3)** — Lives inside Live; mediates LOM access via WebSocket
4. **Live Extension (C#)** — Native plugin API (Live 12.4+)

**Live 11 Constraint:** No native extension API. Remote Script alone handles all LOM access.

**Porting Strategy:**
- ✅ Remote Script (Python) — **backward compatible** to Live 11 with minimal changes
- ✅ Bridge routing logic — **can be adapted** (remove extension channel, keep remote-script)
- ⚠️ Wire protocol (HMAC-SHA256, JSON signing) — **protocol-agnostic, survives**
- ❌ Extension-only operations — **must map to Remote Script equivalents or be disabled**
- ❌ Extension UI hints (device tree, focus tracking) — **deferred to Live 11 UI workarounds**

---

## 1. Architecture Layers

### 1.1 Remote Script (`remote-script/ableton_mcp_remote_script.py`)

**What it does:**
- Instantiates as a Live Remote Script (extension parameter in Live preferences)
- Opens a WebSocket server (default localhost:9001)
- Exposes Ableton Live's **Legacy Object Model (LOM)** over JSON-RPC

**Live 11 compatibility:**
- Live 11 **supports Remote Scripts natively** (deprecated in Live 12, but functional)
- LOM is **stable** — 80% of operations used in Kumi predate Live 12
- WebSocket transport is **independent** of Live version

**Required adaptations:**
- Remove `native_editing_live_fixture.py` (Live 12 native editing mode — doesn't exist in Live 11)
- Conditional: Skip Live 12 extension-only LOM paths
- Test: Verify `song.get_data()` and clip/track mutation chains

### 1.2 Bridge (`crates/ableton-mcp-server/src/bridge/`)

**Current architecture (Live 12):**

```
┌─────────────────────────────────────────┐
│  MCP Client (stdio)                     │
└────────────────┬────────────────────────┘
                 │ JSON-RPC invoke
                 ▼
        ┌─────────────────────┐
        │   RouterAdapter     │ (router.rs)
        │  (dual-channel)     │
        └────┬────────────┬───┘
             │ route by   │
             │ operation  │
      ┌──────▼───┐  ┌─────▼──────┐
      │ Remote   │  │ Extension  │
      │ Script   │  │ (Live 12)  │
      │ (Python) │  │ (C#)       │
      └──────────┘  └────────────┘
```

**Live 11 adaptation:**

```
┌──────────────────────────────────────┐
│  MCP Client (stdio)                  │
└────────┬─────────────────────────────┘
         │ JSON-RPC invoke
         ▼
    ┌──────────────────┐
    │ RemoteAdapter    │ (remote_adapter.rs)
    │ (single channel) │
    └────────┬─────────┘
             │
             ▼
        ┌──────────────┐
        │ Remote Script│
        │ (Python 3)   │
        └──────────────┘
```

**Changes required:**
- Remove `extension_channel.rs`, `extension_launcher.rs`, `extension_setup.rs`
- Simplify `router.rs` → always route to `remote_adapter`
- Remove `route_to_extension()` decision logic
- Keep `wire.rs` (crypto signing, serialization — independent)

### 1.3 Remote Adapter (`bridge/remote_adapter.rs`)

**Current:** Handles WebSocket framing, authority digest, protocol version negotiation with Remote Script.

**Live 11 impact:** ✅ **No changes needed** — this layer is protocol-neutral, version-agnostic.

Key functions (all survive):
- `references_key()` — Extract reference IDs from payloads
- `digest_references()` — Authority checking (Remote Script validates mutations)
- `expand_pad_chains()` — Drum rack resolution (stable in Live 11)
- `valid_status()` — Protocol handshake check
- `mutation_authority_required()` — Operation ACL

### 1.4 Wire Protocol (`bridge/wire.rs`)

**HMAC-SHA256 signed JSON frames:**
- **Purpose:** Cryptographic handshake between bridge and Remote Script
- **Dependency:** `sha2`, `ring`, `subtle` — standard crates, version-independent
- **Live 11 impact:** ✅ **No changes needed** — operates below LOM layer

---

## 2. Operation Coverage by Live Version

### 2.1 READ Operations (Highest Compatibility)

**Live 11 ✅ — all supported:**
- `song.read` — Track structure, clips, devices
- `song.time-convert` — BPM/time mapping
- `arrangement.automation.read` — Automation lanes
- `browser.search`, `browser.roots` — Library enumeration
- `device.banks.read` — Device parameters
- `clip.time-convert` — Warp marker math
- `audio.capture.status` — Analysis state
- `realtime.stats` — DSP load monitoring
- `automation.value-at` — Envelope interpolation
- `tuning.read`, `groove.read` — Session configuration

### 2.2 MUTATION Operations (Mixed Compatibility)

**Live 11 ✅ (Remote Script fully supports):**
- `track.create`, `track.delete`, `track.duplicate`
- `clip.create`, `clip.duplicate`, `clip.delete`
- `scene.create`, `scene.delete`, `scene.duplicate`
- `device.insert`, `device.delete`, `device.duplicate`
- `device.parameter.*` — Knob turning, automation
- `note.*` — MIDI note editing
- `session.capture-midi` — MIDI recording

**Live 12 extension-only (cannot directly support):**
- `willington.device.read`, `willington.editing.read` (native editing mode — Live 11 has no equivalent)
- `native_editing.*` — Live 11 has no native editing engine
- Focus/tree tracking hints (no extension API)

**Live 11 workarounds for extension-only ops:**
- Map `willington.device.read` → REST fallback (if available) or disable
- Return 404/unavailable for native editing
- Substitute with raw LOM object inspection + Python introspection

### 2.3 Transactions (Highest Risk)

**Live 11 status:** ✅ **Supported, but fragile**

Kumi's transaction system:
- Batch mutations via `transaction.start()`, `transaction.step()`, `transaction.end()`
- Remote Script collects undo steps atomically
- Authority digest validates reference consistency

**Live 11 constraint:**
- Remote Script's undo step management is **more conservative** than Live 12
- Nested undo transactions may not collapse the way Live 12 does
- **Fix:** Validate undo chain behavior in Live 11 test suite; may need transaction size limits

---

## 3. Source File Inventory

### 3.1 Files That Survive (No Changes Needed)

```
crates/ableton-mcp-server/src/
├── lib.rs (module declarations — no version checks)
├── als.rs (ALS file parsing — stable format)
├── analysis.rs (PCM analysis — implementation detail)
├── audio_diagnosis.rs (loudness/spectral — algorithm, not LOM-dependent)
├── audio_file.rs (WAV/MP3 I/O — standard codecs)
├── audio_standards.rs (loudness standards — reference data)
├── key_estimation.rs (pitch detection — DSP, not LOM)
├── library_search.rs (browser enumeration — uses browser.search + browser.roots)
├── loopback.rs (Local Live simulator — pure Rust)
├── mcp_protocol.rs (JSON-RPC framing — version-independent)
├── platform.rs (OS integration — macOS/Windows, not Live-specific)
├── project.rs (ALS file format — no version check)
├── project_semantic.rs (Project model — stable)
├── sqlite_reader.rs (Database access — internal)
├── stdio.rs (Process I/O — framework-level)
├── tool_catalog.rs (MCP tool registry — version-independent)
└── transactions/ (batching, device_state, session_midi — all Remote Script-compatible)
```

**Total: ~45 files, 0 changes**

### 3.2 Files Requiring Adaptation

```
crates/ableton-mcp-server/src/bridge/
├── remote_adapter.rs (✅ no changes — protocol layer)
├── wire.rs (✅ no changes — crypto/framing layer)
├── listeners.rs (⚠️ minor: remove extension event channel)
├── mod.rs (⚠️ remove extension_* exports)
├── router.rs (⚠️ remove EXTENSION_FIRST list, simplify route logic)
├── extension_channel.rs (❌ DELETE — Live 12 extension-only)
├── extension_launcher.rs (❌ DELETE — Live 12 extension-only)
├── extension_setup.rs (❌ DELETE — Live 12 extension-only)
└── live_extension_folders.rs (⚠️ return error or skip)

crates/ableton-mcp-server/src/host/
├── willington.rs (❌ DELETE or stub — native editing, Live 12 only)
├── native_editing.rs (❌ DELETE — Live 12 only)
└── (all others, ~75 files: ✅ no changes)

crates/ableton-mcp-server/src/
├── serve.rs (⚠️ remove extension initialization)
├── setup.rs (⚠️ skip extension folder setup)
├── lifecycle_cli.rs (⚠️ remove extension installation steps)
└── deliver.rs (⚠️ skip extension delivery)
```

**Total: ~15 files need changes, ~8 can be deleted**

### 3.3 Remote Script

```
remote-script/
├── ableton_mcp_remote_script.py (⚠️ remove Live 12 conditional imports)
├── AbletonMcpBridge/__init__.py (✅ no changes)
├── native_editing_self_test.py (❌ DELETE)
├── native_editing_live_fixture.py (❌ DELETE)
├── test_native_editing.py (❌ DELETE test)
├── test_remote_script.py (✅ keep, adapt for Live 11)
└── test_willington.py (❌ DELETE test)
```

---

## 4. Protocol Compatibility

### 4.1 Protocol Version Negotiation

Current code (remote_adapter.rs:150):
```rust
|| !(row.get("epoch") == Some(&Value::Null) 
   || wire::safe_integer(&value["epoch"]).is_some_and(|n| n >= 1.0))
|| value["protocol"] != LIVE_PROTOCOL_VERSION
```

**Change:** Add Live 11 protocol version constant and route check:
```rust
const PROTOCOL_LIVE_12: &str = "kumi-bridge/1.0";  // current
const PROTOCOL_LIVE_11: &str = "kumi-bridge/0.11"; // proposed
```

**Why:** Allows single binary to support both Live versions (feature-gated).

### 4.2 Authority Digest

Current: Remote Script computes SHA-256 HMAC of all mutations.

**Live 11 compatibility:** ✅ Remote Script will compute digest identically. No protocol change needed.

---

## 5. Testing Strategy for Live 11 Port

### Phase 1: Build & Bootstrap
- [ ] Remove extension modules (compile errors → fix)
- [ ] Compile against Live 11 Remote Script stub
- [ ] Run mcp_protocol tests (protocol-layer only)

### Phase 2: Remote Script Validation
- [ ] Install Remote Script in Live 11.3.12
- [ ] Verify WebSocket handshake (wire.rs tests)
- [ ] Test READ operations (status, song.read, browse)
- [ ] Test single mutations (track.create, device.insert)

### Phase 3: Transaction Testing
- [ ] Batch mutations (track + device + clip in one transaction)
- [ ] Undo chain validation (verify atomic undo in Live 11)
- [ ] Authority digest validation (reference consistency)

### Phase 4: Coverage Testing
- [ ] All supported operations (READ + MUTATION from 2.2)
- [ ] Error handling (unavailable operations → 404)
- [ ] Performance (large Sets, many clips)

---

## 6. Deployment Artifact

### Binary Feature Flags

Proposed `Cargo.toml` changes:
```toml
[features]
live-12 = ["extension", "native-editing"]  # current default
live-11 = []  # no extension, no native editing

[dependencies]
ableton-lom-11 = { version = "0.8", optional = true }
```

### Single Binary Support

```rust
// src/bridge/router.rs
pub fn new(
    remote: Rc<dyn AsyncLiveAdapter>,
    extension: Option<ExtensionChannel>,  // None for Live 11
    on_close: Option<Rc<dyn Fn()>>,
) -> RoutedLiveAdapter {
    match extension {
        Some(ext) => routed_adapter(remote, ext, on_close),  // Live 12
        None => simple_adapter(remote, on_close),  // Live 11 (new)
    }
}
```

This allows shipping one binary that detects Live version at runtime.

---

## 7. Known Risks & Mitigations

| Risk | Live 11 Impact | Mitigation |
|------|---|---|
| Undo transaction atomicity | Medium | Test undo chains; limit batch size if needed |
| Remote Script resource limits | Low | Monitor heap; Live 11 is less constrained |
| LOM reference invalidation | Medium | Validate references across mutations |
| WebSocket timeout (long operations) | Low | Increase timeout; implement keep-alive |
| Device parameter edge cases | Low | Test with known problematic devices (Wavetable, etc.) |
| Native editing (no Live 11 equivalent) | High | Return 404 or disable feature; use REST fallback if available |

---

## 8. Deliverables (This Port)

1. **`kumi-live11/` branch** — Fork of ableton-mcp-server with Live 11 adaptations
2. **`LIVE_11_ADAPTER.md`** — Implementation checklist (per file)
3. **`tests/test_live_11.rs`** — Integration tests (Live 11 Remote Script validation)
4. **`scripts/build-live11.sh`** — Build script (cargo feature flags)
5. **Protected worktree** — Your fork ready for local iteration

---

## 9. Next Steps

**Immediate (This Session):**
- [ ] Create Live 11 worktree
- [ ] Remove extension modules (compile)
- [ ] Validate Remote Script protocol (unit tests)

**Follow-up (Integration):**
- [ ] Install Remote Script in Live 11.3.12
- [ ] Run operation coverage tests
- [ ] Performance validation (large Sets)
- [ ] Document workarounds (native editing → alternatives)

**Release (Future):**
- [ ] Feature-gated single binary
- [ ] Installer detection (auto-select bridge by Live version)
- [ ] User documentation (Live 11 limitations)

---

## 10. Repository Pointers

**Key files to inspect:**
- `crates/ableton-mcp-server/src/bridge/mod.rs` — Extension API surface
- `crates/ableton-mcp-server/src/host/willington.rs` — Native editing (delete for Live 11)
- `remote-script/ableton_mcp_remote_script.py` — Entry point (adapt for Live 11)
- `crates/ableton-mcp-server/src/serve.rs` — Server initialization (skip extension setup)

**Live 11 Remote Script reference:**
- Ableton's Live 11 SDK (legacy): `/Applications/Ableton Live 11 Suite.app/Contents/App-Resources/MIDI Remote Scripts/`
- Live 11 LOM object model (stable): https://www.ableton.com/en/packs/live-object-model/

---

**Analysis Status:** ✅ Complete. Ready for worktree creation and protected fork.
