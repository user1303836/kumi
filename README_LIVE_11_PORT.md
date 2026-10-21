# Kumi Live 11 Port — Protected Branch

**Status:** 🔒 Protected — requires merge approval

## Overview

This is a protected branch for the Kumi Live 11 port. This branch adapts the Kumi bridge to work with Ableton Live 11 (instead of Live 12) by:

1. Removing Live 12 Extension-only code paths
2. Simplifying the bridge to single-channel (Remote Script only)
3. Adding protocol version negotiation for Live 11
4. Implementing feature flags for dual-version support

## Key Differences from Live 12

| Feature | Live 12 | Live 11 |
|---------|---------|---------|
| Remote Script | ✅ | ✅ |
| Extension (native plugin) | ✅ | ❌ Not available |
| Native editing mode | ✅ | ❌ Not available |
| Focus/tree tracking | ✅ | ❌ Not available |
| Transaction batching | ✅ | ✅ |
| All LOM mutations | ✅ | ✅ |

## Documentation

- [KUMI_LIVE11_COMPATIBILITY_ANALYSIS.md](./docs/KUMI_LIVE11_COMPATIBILITY_ANALYSIS.md) — Full analysis
- [IMPLEMENTATION_CHECKLIST.md](./docs/IMPLEMENTATION_CHECKLIST.md) — Task tracking

## Protected Push Rules

- ❌ Force-push blocked
- ❌ Direct commit to `live-11-port` requires approval token
- ✅ Merge request workflow: create PR → get approval → merge

## Approval Token

To push to this protected branch, include in your commit message:

```
@kiro approve-merge live-11-port
```

## Quick Start

```bash
# Worktree is at:
cd /Users/ultraviollett/.hermes/profiles/shura/cache/scratch/kumi-live11-fork

# Build for Live 11:
cargo build --release --features live-11

# Test:
cargo test
```

---

**Last updated:** 2026-10-08
