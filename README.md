# Window Zones

Window Zones is planned as a Rust, cross-platform, Rectangle-style window positioning utility.

V1 is a background utility, not a replacement window manager. It will listen for configured bindings and move or resize the currently focused OS-managed window into a named zone or onto another display.

See:

- `CONTEXT.md` for domain language.
- `docs/adr/0001-v1-window-positioning-utility.md` for the accepted v1 boundary and first implementation slice.

## First slice

The implemented core slices are platform-neutral only:

- integer pixel geometry
- built-in zones
- display-to-display movement calculations
- TOML config parsing
- a `WindowSystem` adapter contract
- an `execute_action` executor tested with fake adapters

Platform adapters for real hotkeys, focused-window detection, display enumeration, and move/resize calls are intentionally deferred.
