## What and why

<!-- What does this change, and why? Link the issue if there is one. -->

## Testing

- [ ] `cargo test --workspace --locked`
- [ ] `hsmp-gate g0` (or the pre-push hook)
- [ ] `scripts/e2e-test.sh` (network, server or sidecar changes)
- [ ] Tested in game (Lua mod changes): describe below

<!-- How you tested it, and anything a reviewer should try. -->

## Checklist

- [ ] Docs updated (players / hosting / development) where behaviour, flags, ports or files changed
- [ ] `CHANGELOG.md` updated under "Unreleased"
- [ ] Lua changes follow docs/development/lua-mods.md (game thread only, no soft-object reads,
      world guard, Blueprint names with spaces, only the Director changes levels)
- [ ] No secrets, personal paths or game files added
