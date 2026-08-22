# Covenant Console (frontend)

The console is a Vite + React + TypeScript app, dynamically linked to the
`covenant serve` `/v1` API through the typed client in `src/api.ts` — the
compile-time tripwire against the serde shapes in `src/{report,diff,spec}.rs`.

## Layout

| Path | What it is |
|---|---|
| `src/api.ts` | typed `/v1` client + the demo payloads the live screens submit |
| `src/vals.tsx` | the design mock's state model (ported verbatim, typed) + the live overlay + per-screen endpoint chips |
| `src/App.tsx` | state + the boot effect that probes `/v1/health` and loads live data |
| `src/generated/ConsoleView.tsx` | GENERATED 1-to-1 from the Claude Design mock — do not edit by hand |
| `src/index.css` | the vendored nocturne design system (Google-Fonts import removed on purpose) + console chrome + endpoint-chip styles |

## Build

```bash
npm ci
npm run build      # tsc --noEmit && vite build -> dist/
```

`dist/` is **committed**: `cargo build --features serve` embeds it via
rust-embed, so the Rust build never needs a Node toolchain. Rebuild and
commit `dist/` whenever `src/` changes.

## Live vs demo

On load the app probes `/v1/health`. When `covenant serve` is unreachable,
every screen runs on the mock's demo data. When it answers, the fallback is
per endpoint: the Editor / Check report / PR gate / Contract detail screens
render real engine results, and with the Phase-2 file taps wired
(`serve --gate-stats/--dlq` reading what `gate --stats/--dlq` writes) the
Stream-gate and Dead-letters screens go live too — while a screen whose
endpoint is unwired or failing falls back to demo data alone, with its chip
saying exactly why, and every other screen keeps its live results. Either way the
header chip states exactly which endpoint backs the screen — or, when none is
wired, which endpoint is missing. The chip is keyed on
each endpoint's own outcome, not just the health probe: `live` (the call
succeeded), `<endpoint> failed` (serve is up but that call errored — the
tooltip says how), `demo · serve offline`, `endpoint missing`, or `static`.
