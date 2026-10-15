# Live 11 Port — Implementation Checklist

**Branch:** live-11-port  
**Worktree:** kumi-live11-fork  
**Status:** Ready for implementation

---

## Phase 1: Build & Compile

- [ ] Remove extension modules (extension_channel.rs, extension_launcher.rs, extension_setup.rs)
- [ ] Remove willington.rs, native_editing.rs from host/
- [ ] Adapt router.rs (single-channel routing)
- [ ] Adapt listeners.rs (remove extension event channel)
- [ ] Adapt serve.rs (skip extension initialization)
- [ ] Adapt setup.rs (skip extension folder setup)
- [ ] Adapt lifecycle_cli.rs (remove extension install steps)
- [ ] Adapt delivery.rs (skip extension delivery)
- [ ] Adapt remote-script/ableton_mcp_remote_script.py (remove Live 12 conditionals)
- [ ] Add Cargo.toml feature flags (live-11)
- [ ] Add mcp_protocol.rs Live 11 version constant
- [ ] Remove Live 12-only test files
- [ ] `cargo build --release` succeeds with zero errors
- [ ] `cargo test` passes

## Phase 2: Protocol Validation

- [ ] Wire signing/verification tests pass
- [ ] Protocol version negotiation works
- [ ] Remote Script handshake succeeds
- [ ] Authority digest validation passes

## Phase 3: Live 11 Integration

- [ ] Install Remote Script in Live 11.3.12
- [ ] MCP server starts
- [ ] Connects to Live 11 successfully
- [ ] `status` query returns Live 11 adapter info

## Phase 4: Operation Coverage

- [ ] READ operations all return data
  - [ ] song.read
  - [ ] arrangement.automation.read
  - [ ] browser.search
  - [ ] device.banks.read
  - [ ] audio.capture.status
  - [ ] realtime.stats
- [ ] MUTATION operations work
  - [ ] track.create, track.delete
  - [ ] clip.create, clip.delete
  - [ ] scene.create, scene.delete
  - [ ] device.insert, device.delete
  - [ ] device.parameter.set
  - [ ] note.edit
- [ ] TRANSACTIONS succeed
  - [ ] Batch mutations work
  - [ ] Undo chains atomic in Live 11

## Phase 5: Performance

- [ ] Large Sets (100+ tracks) stay responsive
- [ ] No memory leaks in sustained operations
- [ ] WebSocket connection stability over 1hr

## Excluded Features (Live 11)

These Live 12-only features are unavailable in Live 11:

- willington.device.read → 404 unavailable
- willington.editing.read → 404 unavailable
- native_editing.* → 404 unavailable
- Extension UI hints → unavailable

---

**Merge approval:** `@kiro approve-merge live-11-port`
