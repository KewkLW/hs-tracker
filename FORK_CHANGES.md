# S10 tracker enhancements

This fork is based on HS Tracker 1.0.1 and groups the following additions.

## Number display preferences

Settings offers three consistent display styles across the overlay, dashboard,
saved runs, graphs, and copied run cards:

- Standard `K / M / B` abbreviations (default)
- Hero Siege `k / kk / kkk` abbreviations
- Full comma-separated numbers

The choice is reactive across open windows, persists between launches, and
falls back to Standard when an older settings file has no valid value.

## Hero-level and run tools

- The dashboard shows current hero-level progress and ten target levels with
  cumulative XP and ETA at the current session rate.
- Saved runs show the same forecast at that run's XP/hour, plus observed level
  completion times measured in active playtime.
- The first incomplete level is labeled `observed remainder`; timers survive a
  run reset or app restart and reset safely on character changes or rollbacks.
- `Start new run` files the current non-empty run and starts fresh counters from
  the Runs page without restarting the game or tracker.

Hero-XP requirements are explicitly labeled as a community-curve estimate. The
curve interpolates published Season 9 anchors plus one observed Season 10 value
at hero level 10, and extrapolates its final segment above hero level 149. Older
saved runs remain compatible, but cannot show fields they never recorded.

## Startup placement

Settings can choose the monitor and initial face used on the next launch:

- remember the previous position or center on a named monitor;
- open the full dashboard or compact overlay; and
- fall back safely when a chosen monitor is disconnected.

Automatic game detection no longer opens a compact overlay beside an already
visible dashboard.

## Passive market-observer groundwork

`HS_MARKET_OBSERVER=1` enables a disabled-by-default protocol research mode. It
does not provide a market API or price checker yet. It sends no requests, reads
no game memory, performs no injection, replay, MITM, or certificate bypass, and
records no raw packet payloads or credential values.

The observer restricts capture to exact Hero Siege endpoint tuples (plus any
explicit `HS_MARKET_ENDPOINTS`), suppresses the raw Debug Log, redacts dynamic
routes and sensitive fields, and stores only structural observations and
one-second TLS-framing summaries. Flow and adapter identities are process-salted
opaque tags rather than raw addresses, ports, or device GUIDs. The log rotates
at 16 MB and retains one older segment. See [MARKET_OBSERVER.md](MARKET_OBSERVER.md)
for the experiment and its limits.

## Validation

- `npm test` — JavaScript and Rust suites
- `npm run build` — production Svelte/Vite bundle
- `cargo clippy --manifest-path src-tauri/Cargo.toml --lib -- -D warnings`
- `git diff --check`

The number/launch controls and Windows monitor placement have production-build
coverage but do not yet have dedicated UI or multi-monitor integration tests.
