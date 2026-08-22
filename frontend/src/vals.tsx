// The console's data layer: the design mock's state model (`renderVals`)
// ported verbatim from the mock's script, typed, plus the Covenant live
// layer that overlays real /v1 responses and stamps every screen with an
// endpoint-status chip. Demo values match the mock exactly; live values
// replace them when `covenant serve` is reachable.
import type { ChangeEvent, ReactNode } from 'react';
import type {
  ApiError, CheckReport, ContractInfo, DiffReport, DlqEnvelopeDoc, DlqResponse,
  GateStatsResponse, ValidateResponse,
} from './api';

export type Screen =
  | 'trust' | 'teams' | 'registry' | 'detail' | 'editor' | 'report'
  | 'diff' | 'gate' | 'dlq' | 'alerts' | 'install';

export interface ConsoleState {
  screen: Screen;
  query: string;
  filter: string;
  contractId: string;
  detailTab: string;
  authorMode: string | null;
  reportLayout: string | null;
  healthLayout: string | null;
  dlqIndex: number;
  replayed: boolean;
  blockOnWarn: boolean;
  autoDlq: boolean;
  onViolation: string;
}

export const initialState: ConsoleState = {
  screen: 'trust', query: '', filter: 'All', contractId: 'orders',
  detailTab: 'Fields', authorMode: null, reportLayout: null, healthLayout: null,
  dlqIndex: 0, replayed: false, blockOnWarn: true, autoDlq: true, onViolation: 'block',
};

/** The non-health endpoints a screen can be backed by; keys of `LiveData.errors`. */
export type EndpointKey = 'contract' | 'validate' | 'check' | 'diff' | 'gate' | 'dlq';

export interface LiveData {
  up: boolean;
  version: string | null;
  contract: ContractInfo | null;
  validate: ValidateResponse | null;
  check: CheckReport | null;
  diffR: DiffReport | null;
  gateStats: GateStatsResponse | null;
  dlq: DlqResponse | null;
  /** Per-endpoint failure while the server is up — the chip distinguishes
   *  "this endpoint failed" from "serve is offline". */
  errors: Partial<Record<EndpointKey, ApiError>>;
}

export const emptyLive: LiveData = {
  up: false, version: null, contract: null, validate: null, check: null, diffR: null,
  gateStats: null, dlq: null,
  errors: {},
};

type Click = () => void;

interface NavItem { label: string; badge: string; badgeColor: string; onClick: Click; bg: string; color: string; mark: string }
interface NavGroup { title: string; items: NavItem[] }
interface SegOpt { label: string; onClick: Click; bg: string; color: string }
interface Stat { label: string; value: string; note: string; color: string }
interface ContractRow {
  id: string; name: string; version: string; source: string; owner: string; ownerEmail: string;
  points: string; last: string; health: string; status: string;
  dot: string; lastColor: string; onClick: Click;
}
interface DetailInfo {
  id: string; name: string; version: string; source: string; owner: string; ownerEmail: string;
  points: string; last: string; health: string; status: string;
}
interface Tab { label: string; onClick: Click; color: string; border: string }
interface FilterChip { label: string; onClick: Click; bg: string; border: string; color: string }
interface DetailField { name: string; type: string; rules: string[]; health: string; healthColor: string }
interface PolicyRow { label: string; value: string; note: string }
interface ExitCode { code: string; meaning: string; color: string }
interface EnforcementPoint { title: string; dot: string; body: string; cmd: string; stat: string }
interface VersionRow { version: string; title: string; by: string; severity: string; color: string; when: string }
interface CodeLine { t: string; c: string }
interface FormField { name: string; type: string; rules: string[]; coverage: string }
interface Finding { level: string; color: string; path: string; message: string }
interface RuleGroup { field: string; rule: string; count: string; pct: string; sample: string }
interface SampleRow { row: string; field: string; rule: string; value: string }
interface LegendRow { label: string; color: string }
interface DiffFinding { severity: string; color: string; path: string; message: string; who: string; impact: string }
interface ConsumerRow { name: string; uses: string; impact: string; color: string }
interface ThroughputBar { bad: string; good: string }
interface LiveRule { field: string; rule: string; rate: string; pct: string }
interface BlockedRow { time: string; key: string; why: string }
interface DlqViolation { field: string; rule: string; message: string }
interface DlqEntry {
  id: string; when: string; rules: string; row: number;
  /** '—' for live entries: the transport-agnostic gate has no partitions. */
  partition: number | string;
  json: CodeLine[]; violations: DlqViolation[];
}
interface DlqListRow { id: string; when: string; rules: string; onClick: Click; bg: string; mark: string }
interface HeatCell { color: string; title: string }
interface HeatRow { team: string; summary: string; color: string; cells: HeatCell[] }
interface TeamCard { team: string; dot: string; contracts: string; pass: string; blocked: string; mttr: string; color: string; note: string }
interface SettingRow {
  label: string; note: string; isChoice?: boolean; options: SegOpt[];
  isToggle?: boolean; trackBg?: string; justify?: string; knob?: string; onClick?: Click;
}
interface RouteRow { owner: string; target: string; when: string }
interface InstallStep { n: string; title: string; body: string; cmd: string }
interface Guarantee { field: string; promise: string; evidence: string }
interface DriftEvent { title: string; detail: string; when: string; color: string }

export interface Vals {
  query: string; onQuery: (e: ChangeEvent<HTMLInputElement>) => void;
  navGroups: NavGroup[]; screenTitle: string; screenSub: string; epChip: ReactNode;
  isTrust: boolean; isRegistry: boolean; isDetail: boolean; isEditor: boolean; isReport: boolean;
  isDiff: boolean; isGate: boolean; isDlq: boolean; isTeams: boolean; isAlerts: boolean; isInstall: boolean;
  goDetail: Click; goEditor: Click; goReport: Click; goDlq: Click; goRegistry: Click; noop: (e?: unknown) => void;
  trustStats: Stat[]; trustGuarantees: Guarantee[]; driftEvents: DriftEvent[];
  registryFilters: FilterChip[]; visibleContracts: ContractRow[]; registryCount: string;
  detail: DetailInfo; detailTabs: Tab[];
  detailFieldsTab: boolean; detailPolicyTab: boolean; detailEnforceTab: boolean; detailHistoryTab: boolean;
  detailFields: DetailField[]; policyRows: PolicyRow[]; exitCodes: ExitCode[];
  enforcementPoints: EnforcementPoint[]; versionHistory: VersionRow[];
  authorTabs: SegOpt[]; isYamlMode: boolean; isFormMode: boolean; editorHint: string;
  yamlLines: CodeLine[]; formFields: FormField[]; lintFindings: Finding[];
  reportTabs: SegOpt[]; isByRule: boolean; isBySample: boolean;
  ruleGroups: RuleGroup[]; sampleRows: SampleRow[];
  diffLegend: LegendRow[]; diffFindings: DiffFinding[]; consumers: ConsumerRow[];
  gateStats: Stat[]; throughput: ThroughputBar[]; liveRules: LiveRule[]; blockedStream: BlockedRow[];
  dlqList: DlqListRow[]; dlqDetail: DlqEntry; replayLabel: string; replay: Click;
  healthTabs: SegOpt[]; isHeatmap: boolean; isCards: boolean; heatRows: HeatRow[]; teamCards: TeamCard[];
  settingRows: SettingRow[]; routes: RouteRow[];
  installSteps: InstallStep[];
}

const RED = '#d98a8a', GREEN = '#84d9a8', ACC = '#9184d9', MUTED = '#8b8f9f';

const CONTRACTS = [
  { id: 'orders', name: 'Orders stream', version: '1.2.0', source: 'kafka · checkout.orders', owner: 'data-platform', ownerEmail: 'data-platform@acme.io', points: 'check · gate · lib', last: '2 min ago', health: 'clean', status: 'clean · 4.2M rows today' },
  { id: 'payments', name: 'Payments ledger', version: '3.1.0', source: 'parquet · s3://lake/payments/', owner: 'payments-eng', ownerEmail: 'payments-eng@acme.io', points: 'check · gate', last: '11 min ago', health: 'blocked', status: '412 violations · budget 0' },
  { id: 'customer_profiles', name: 'Customer profiles', version: '2.0.0', source: 'postgres · public.customers', owner: 'growth-eng', ownerEmail: 'growth-eng@acme.io', points: 'check', last: '1 h ago', health: 'warn', status: 'schema drift: 2 new columns' },
  { id: 'checkout_events', name: 'Checkout events', version: '4.3.1', source: 'kafka · checkout.events', owner: 'data-platform', ownerEmail: 'data-platform@acme.io', points: 'gate', last: '4 min ago', health: 'clean', status: 'clean · 18.9M rows today' },
  { id: 'inventory_snapshots', name: 'Inventory snapshots', version: '1.0.4', source: 'parquet · s3://lake/inventory/', owner: 'supply-eng', ownerEmail: 'supply-eng@acme.io', points: 'check', last: '26 min ago', health: 'clean', status: 'clean · nightly' },
  { id: 'shipments', name: 'Shipments', version: '2.4.0', source: 'ndjson · nightly export', owner: 'supply-eng', ownerEmail: 'supply-eng@acme.io', points: 'check · lib', last: '3 h ago', health: 'warn', status: 'on_violation: warn' },
];

const HEALTH_DOT: Record<string, string> = { clean: '#4d5566', warn: ACC, blocked: RED };

/* Per-screen endpoint status: `live: null` means the backing endpoint DOES
   NOT EXIST yet; a string names what the screen
   calls when served, and `key` is where its outcome lands in
   `LiveData.errors`. */
const EP: Record<Screen, { live: string | null; key?: EndpointKey; label: string }> = {
  trust:    { live: null, label: 'demo — needs GET /v1/metrics (trust ledger history) · not implemented, Phase 3 fleet' },
  teams:    { live: null, label: 'demo — needs GET /v1/teams (per-team outcomes) · not implemented, Phase 3 fleet' },
  registry: { live: null, label: 'demo — needs GET /v1/contracts (registry) · not implemented, Phase 3 registry' },
  detail:   { live: 'GET /v1/contract', key: 'contract', label: 'partial — fields & policy live from GET /v1/contract; per-field health needs fleet history (missing)' },
  editor:   { live: 'POST /v1/validate', key: 'validate', label: 'live — lint panel from POST /v1/validate' },
  report:   { live: 'POST /v1/check', key: 'check', label: 'live — report from POST /v1/check (demo NDJSON)' },
  diff:     { live: 'POST /v1/diff', key: 'diff', label: 'live — findings + consumer blast radius from POST /v1/diff (demo manifests; aggregating manifests across repositories is out of scope for this build)' },
  gate:     { live: 'GET /v1/gate/stats', key: 'gate', label: 'live — stats from GET /v1/gate/stats (the gate\'s --stats sidecar); blocked tail from GET /v1/dlq' },
  dlq:      { live: 'GET /v1/dlq', key: 'dlq', label: 'live — dead letters from GET /v1/dlq; replay needs POST /v1/dlq/replay, which this build does not serve' },
  alerts:   { live: null, label: 'demo — needs /v1/settings + /v1/routes · not implemented, Phase 3 notifications' },
  install:  { live: '', label: 'static — no endpoint needed' },
};

const SEV_COLOR: Record<string, string> = {
  breaking: RED, risky: '#d2cefd', info: MUTED, error: RED, warning: '#d2cefd',
};

function apiErrorText(err: ApiError): string {
  switch (err.reason) {
    case 'status': return `answered HTTP ${err.status}`;
    case 'parse': return 'returned an unparseable body';
    default: return 'did not answer (network error or timeout)';
  }
}

/* The gate/dlq endpoints exist but answer honestly when their file taps
   aren't wired (`configured: false`) or the files aren't there yet — the
   chip shows demo state with the server's own hint instead of a green
   "live" over demo numbers. */
function notLive(screen: Screen, live: LiveData): { label: string; text: string } | null {
  if (screen === 'gate' && live.gateStats) {
    if (!live.gateStats.configured) {
      return { label: 'demo · not wired', text: live.gateStats.hint ?? '' };
    }
    if (!live.gateStats.stats) {
      return { label: 'demo · no stats yet', text: live.gateStats.note ?? '' };
    }
    // The gate screen's blocked-records panel comes from /v1/dlq — a chip
    // saying plain "live" while that panel shows demo rows would be the
    // exact lie the chip exists to prevent.
    if (!live.dlq?.configured || !live.dlq.entries) {
      return {
        label: 'partial · no DLQ tap',
        text: 'stats are live; the blocked-records panel is demo until serve is started with --dlq <path>',
      };
    }
  }
  if (screen === 'dlq' && live.dlq) {
    if (!live.dlq.configured) {
      return { label: 'demo · not wired', text: live.dlq.hint ?? '' };
    }
    if (!live.dlq.entries) {
      return { label: 'demo · no DLQ file', text: live.dlq.note ?? '' };
    }
  }
  return null;
}

function chip(screen: Screen, live: LiveData): ReactNode {
  const ep = EP[screen];
  const err = ep.key ? live.errors[ep.key] : undefined;
  const unwired = live.up ? notLive(screen, live) : null;
  let cls: string;
  let label: string;
  let text: string;
  if (ep.live === '') { cls = 'static'; label = 'static'; text = ep.label; }
  else if (ep.live === null) { cls = 'miss'; label = 'endpoint missing'; text = ep.label; }
  else if (!live.up) { cls = 'off'; label = 'demo · serve offline'; text = `demo — serve not running; would call ${ep.live} (start: covenant serve -c orders.yaml)`; }
  else if (err) { cls = 'fail'; label = `${ep.live} failed`; text = `demo — serve is up but ${ep.live} ${apiErrorText(err)}; showing demo data`; }
  else if (unwired) { cls = 'off'; label = unwired.label; text = unwired.text; }
  else { cls = 'live'; label = ep.live; text = ep.label; }
  return (
    <div
      className={`epchip ${cls}`}
      title={text}
      role="note"
      tabIndex={0}
      aria-label={`Endpoint status: ${label}. ${text}`}
    >
      <span className="dot" />{label}
    </div>
  );
}

export function renderVals(
  s: ConsoleState,
  set: (patch: Partial<ConsoleState>) => void,
  live: LiveData,
): Vals {
  const go = (screen: Screen, extra?: Partial<ConsoleState>) => set({ screen, ...(extra ?? {}) });

  const seg = (options: string[], current: string, pick: (v: string) => void): SegOpt[] =>
    options.map((label) => ({
      label, onClick: () => pick(label),
      bg: label === current ? 'rgba(145,132,217,.18)' : 'transparent',
      color: label === current ? '#d2cefd' : MUTED,
    }));

  const reportLayout = s.reportLayout ?? 'By rule';
  const healthLayout = s.healthLayout ?? 'Heatmap';
  const authorMode = s.authorMode ?? 'YAML';

  const nav: { title: string; items: { id: Screen; label: string; badge?: string; badgeColor?: string }[] }[] = [
    { title: 'Overview', items: [
      { id: 'trust', label: 'Trust ledger' },
      { id: 'teams', label: 'Contract health' },
    ] },
    { title: 'Contracts', items: [
      { id: 'registry', label: 'Registry', badge: '6' },
      { id: 'detail', label: 'Contract detail' },
      { id: 'editor', label: 'Editor' },
    ] },
    { title: 'Enforcement', items: [
      { id: 'report', label: 'Check reports', badge: '1', badgeColor: RED },
      { id: 'diff', label: 'PR gate', badge: '2', badgeColor: RED },
      { id: 'gate', label: 'Stream gate' },
      { id: 'dlq', label: 'Dead letters', badge: '1.2k', badgeColor: MUTED },
    ] },
    { title: 'Settings', items: [
      { id: 'alerts', label: 'Alerts & policy' },
      { id: 'install', label: 'Install' },
    ] },
  ];
  const navGroups: NavGroup[] = nav.map((g) => ({
    title: g.title,
    items: g.items.map((i) => ({
      label: i.label, badge: i.badge || '', badgeColor: i.badgeColor || MUTED,
      onClick: () => go(i.id),
      bg: s.screen === i.id ? 'rgba(145,132,217,.14)' : 'transparent',
      color: s.screen === i.id ? '#e9e9ed' : '#9ba0b1',
      mark: s.screen === i.id ? ACC : 'transparent',
    })),
  }));

  const titles: Record<Screen, [string, string]> = {
    trust: ['Trust ledger', 'Is this data safe to build on?'],
    registry: ['Contract registry', '6 contracts · production'],
    detail: ['analytics.orders', 'Contract detail · v1.2.0'],
    editor: ['Contract editor', 'orders.yaml · draft'],
    report: ['Check report', 'payments.parquet · run #4812'],
    diff: ['PR gate', 'payments contract · PR #2381'],
    gate: ['Stream gate', 'checkout.payments · live'],
    dlq: ['Dead letters', 'payments gate · last hour'],
    teams: ['Contract health', 'Enforcement outcomes by owning team'],
    alerts: ['Alerts & policy', 'Organisation defaults'],
    install: ['Install', 'One binary, three enforcement points'],
  };
  const t = titles[s.screen] || ['', ''];

  const detailBase = CONTRACTS.find((c) => c.id === s.contractId) || CONTRACTS[0];
  const q = s.query.trim().toLowerCase();
  const visibleContracts: ContractRow[] = CONTRACTS
    .filter((c) => s.filter === 'All'
      || (s.filter === 'Needs attention' && c.health !== 'clean')
      || (s.filter === 'Mine' && c.owner === 'data-platform')
      || (s.filter === 'Streaming' && c.source.startsWith('kafka')))
    .filter((c) => !q || (c.id + c.source + c.owner).toLowerCase().includes(q))
    .map((c) => ({
      ...c,
      dot: HEALTH_DOT[c.health],
      lastColor: c.health === 'blocked' ? RED : MUTED,
      onClick: () => go('detail', { contractId: c.id }),
    }));

  const yamlLines: CodeLine[] = ([
    ['# Orders stream — enforced at the checkout producer boundary.', '#6f7386'],
    ['covenant: 1', '#cfd3e5'],
    ['id: orders', '#cfd3e5'],
    ['version: 1.2.0', '#cfd3e5'],
    ['owner: data-platform@acme.io', '#cfd3e5'],
    ['', '#cfd3e5'],
    ['models:', '#d2cefd'],
    ['  orders:', '#d2cefd'],
    ['    strict: true          # undeclared fields are violations', '#cfd3e5'],
    ['    fields:', '#d2cefd'],
    ['      order_id:     { type: string, required: true, unique: true,', '#cfd3e5'],
    ['                      pattern: "^ord_[a-z0-9]{12}$" }', '#cfd3e5'],
    ['      amount_cents: { type: integer, required: true, min: 0, max: 5000000 }', '#cfd3e5'],
    ['      currency:     { type: string, required: true, allowed: [USD, EUR, GBP] }', '#cfd3e5'],
    ['      customer_email: { type: string, format: email, nullable: true }', '#cfd3e5'],
    ['      trace_id:     { type: uuid }', '#cfd3e5'],
    ['      created_at:   { type: timestamp, required: true }', '#cfd3e5'],
    ['', '#cfd3e5'],
    ['policy:', '#d2cefd'],
    ['  on_violation: block     # block | warn', '#cfd3e5'],
    ['  max_violations: 0       # tolerated budget before a check fails', '#cfd3e5'],
    ['  sample_violations: 10   # examples kept per (field, rule)', '#cfd3e5'],
  ] as [string, string][]).map(([tx, c]) => ({ t: tx, c }));

  const dlq: DlqEntry[] = [
    { id: 'pay_9f3a21c7', when: '19:04:11', rules: 'currency · allowed', row: 44182, partition: 3,
      json: [['{', '#cfd3e5'], ['  "payment_id": "pay_9f3a21c7",', '#cfd3e5'], ['  "amount_cents": 4200,', '#cfd3e5'], ['  "currency": "BTC",', RED], ['  "captured_at": "2026-08-11T19:04:10Z",', '#cfd3e5'], ['  "producer": "checkout-svc 4.18.0"', '#6f7386'], ['}', '#cfd3e5']].map(([tx, c]) => ({ t: tx, c })),
      violations: [{ field: 'currency', rule: 'allowed', message: 'value "BTC" not in allowed set [EUR, GBP, USD]' }] },
    { id: 'pay_20b7de40', when: '19:04:09', rules: 'amount_cents · min', row: 44179, partition: 1,
      json: [['{', '#cfd3e5'], ['  "payment_id": "pay_20b7de40",', '#cfd3e5'], ['  "amount_cents": -1500,', RED], ['  "currency": "USD",', '#cfd3e5'], ['  "captured_at": "2026-08-11T19:04:08Z"', '#cfd3e5'], ['}', '#cfd3e5']].map(([tx, c]) => ({ t: tx, c })),
      violations: [{ field: 'amount_cents', rule: 'min', message: 'value -1500 is below min 0 — refunds belong on the refunds contract' }] },
    { id: 'pay_5c1e88fa', when: '19:03:58', rules: 'settled_at · required_missing', row: 44160, partition: 2,
      json: [['{', '#cfd3e5'], ['  "payment_id": "pay_5c1e88fa",', '#cfd3e5'], ['  "amount_cents": 9900,', '#cfd3e5'], ['  "currency": "EUR"', '#cfd3e5'], ['}', '#cfd3e5']].map(([tx, c]) => ({ t: tx, c })),
      violations: [{ field: 'settled_at', rule: 'required_missing', message: 'required field is absent from the record' }] },
    { id: 'pay_74aa0b19', when: '19:03:51', rules: 'payment_id · unique', row: 44151, partition: 0,
      json: [['{', '#cfd3e5'], ['  "payment_id": "pay_74aa0b19",', RED], ['  "amount_cents": 1200,', '#cfd3e5'], ['  "currency": "GBP"', '#cfd3e5'], ['}', '#cfd3e5']].map(([tx, c]) => ({ t: tx, c })),
      violations: [{ field: 'payment_id', rule: 'unique', message: 'duplicate value — first seen at row 41022' }] },
    { id: 'pay_b0c4d772', when: '19:03:40', rules: 'ledger_ref · unexpected_field', row: 44140, partition: 3,
      json: [['{', '#cfd3e5'], ['  "payment_id": "pay_b0c4d772",', '#cfd3e5'], ['  "amount_cents": 3300,', '#cfd3e5'], ['  "currency": "USD",', '#cfd3e5'], ['  "ledger_ref": "L-8821"', RED], ['}', '#cfd3e5']].map(([tx, c]) => ({ t: tx, c })),
      violations: [{ field: 'ledger_ref', rule: 'unexpected_field', message: 'strict: true — the field is not declared in the contract' }] },
  ];
  const dlqDetail = dlq[s.dlqIndex] || dlq[0];

  const rnd = (i: number, a: number, b: number) => a + ((i * 2654435761) % 1000) / 1000 * (b - a);
  const throughput: ThroughputBar[] = Array.from({ length: 36 }, (_, i) => {
    const bad = i > 26 ? rnd(i, 6, 34) : rnd(i, 0, 3);
    return { bad: bad.toFixed(0) + '%', good: (100 - bad).toFixed(0) + '%' };
  });

  const heatTeams: [string, number, string][] = [
    ['data-platform', 0.05, 'clean 14 d'], ['payments-eng', 0.75, '412 blocked'],
    ['growth-eng', 0.3, '2 drifts'], ['supply-eng', 0.12, 'clean 9 d'], ['ml-platform', 0.22, '1 blocked merge'],
  ];
  const heatRows: HeatRow[] = heatTeams.map(([team, sev, summary], ti) => ({
    team, summary, color: sev > 0.5 ? RED : sev > 0.25 ? '#d2cefd' : MUTED,
    cells: Array.from({ length: 14 }, (_, i) => {
      const vv = Math.max(0, Math.min(1, sev * (0.35 + ((i * 7 + ti * 13) % 10) / 6)));
      const color = vv < 0.15 ? 'rgba(233,233,237,.07)'
        : vv < 0.4 ? 'rgba(145,132,217,.35)'
        : vv < 0.7 ? 'rgba(217,138,138,.5)' : RED;
      return { color, title: team + ' · day ' + (14 - i) };
    }),
  }));

  const vals: Vals = {
    query: s.query, onQuery: (e) => set({ query: e.target.value }),
    navGroups, screenTitle: t[0], screenSub: t[1], epChip: chip(s.screen, live),
    isTrust: s.screen === 'trust', isRegistry: s.screen === 'registry', isDetail: s.screen === 'detail',
    isEditor: s.screen === 'editor', isReport: s.screen === 'report', isDiff: s.screen === 'diff',
    isGate: s.screen === 'gate', isDlq: s.screen === 'dlq', isTeams: s.screen === 'teams',
    isAlerts: s.screen === 'alerts', isInstall: s.screen === 'install',
    goDetail: () => go('detail', { contractId: s.contractId, detailTab: 'Fields' }),
    goEditor: () => go('editor'), goReport: () => go('report'),
    goDlq: () => go('dlq'), goRegistry: () => go('registry'), noop: () => {},

    trustStats: [
      { label: 'Last enforced', value: '2 min ago', note: 'every batch, not sampled', color: '#e9e9ed' },
      { label: 'Rows admitted (24h)', value: '4,218,904', note: '0 rejected', color: GREEN },
      { label: 'Violations (24h)', value: '0', note: 'budget 0 · policy block', color: GREEN },
      { label: 'Contract age', value: 'v1.2.0', note: 'stable 41 days', color: '#e9e9ed' },
    ],
    trustGuarantees: [
      { field: 'order_id', promise: 'Unique, and always matches ord_ + 12 lowercase chars', evidence: '4.2M checked' },
      { field: 'amount_cents', promise: 'Whole cents between 0 and 5,000,000 — never negative', evidence: '4.2M checked' },
      { field: 'currency', promise: 'One of USD, EUR, GBP — nothing else reaches the table', evidence: '4.2M checked' },
      { field: 'customer_email', promise: 'A valid email when present; may be null for guest orders', evidence: '3.1M non-null' },
      { field: 'created_at', promise: 'RFC 3339 timestamp, always present', evidence: '4.2M checked' },
    ],
    driftEvents: [
      { title: 'currency gained no new values', detail: 'A producer tried to emit BTC on Aug 4 — 61 records were blocked at the gate, none landed.', when: '7 days ago', color: ACC },
      { title: 'v1.2.0 — max raised to 5,000,000', detail: 'Minor bump; existing consumers unaffected.', when: '41 days ago', color: MUTED },
      { title: 'trace_id added as optional uuid', detail: 'Info-level change, no consumer action needed.', when: '41 days ago', color: MUTED },
    ],

    registryFilters: ['All', 'Needs attention', 'Mine', 'Streaming'].map((label) => ({
      label, onClick: () => set({ filter: label }),
      bg: s.filter === label ? 'rgba(145,132,217,.16)' : 'transparent',
      border: s.filter === label ? '#5d5294' : 'rgba(233,233,237,.16)',
      color: s.filter === label ? '#d2cefd' : '#b2b6ca',
    })),
    visibleContracts, registryCount: visibleContracts.length + ' of ' + CONTRACTS.length + ' contracts',

    detail: detailBase,
    detailTabs: ['Fields', 'Policy', 'Enforcement', 'History'].map((label) => ({
      label, onClick: () => set({ detailTab: label }),
      color: s.detailTab === label ? '#e9e9ed' : MUTED,
      border: s.detailTab === label ? ACC : 'transparent',
    })),
    detailFieldsTab: s.detailTab === 'Fields', detailPolicyTab: s.detailTab === 'Policy',
    detailEnforceTab: s.detailTab === 'Enforcement', detailHistoryTab: s.detailTab === 'History',
    detailFields: [
      { name: 'order_id', type: 'string', rules: ['required', 'unique', 'pattern ^ord_[a-z0-9]{12}$'], health: 'clean', healthColor: MUTED },
      { name: 'amount_cents', type: 'integer', rules: ['required', 'min 0', 'max 5000000'], health: 'clean', healthColor: MUTED },
      { name: 'currency', type: 'string', rules: ['required', 'allowed USD·EUR·GBP'], health: '61 blocked (7 d)', healthColor: ACC },
      { name: 'customer_email', type: 'string', rules: ['nullable', 'format email'], health: 'clean', healthColor: MUTED },
      { name: 'trace_id', type: 'uuid', rules: ['optional'], health: 'clean', healthColor: MUTED },
      { name: 'created_at', type: 'timestamp', rules: ['required', 'RFC 3339'], health: 'clean', healthColor: MUTED },
    ],
    policyRows: [
      { label: 'on_violation', value: 'block', note: 'Dirty records are withheld and the run exits 1.' },
      { label: 'max_violations', value: '0', note: 'No tolerated budget — one bad row fails the check.' },
      { label: 'sample_violations', value: '10', note: 'Examples kept per (field, rule); counts stay exact.' },
      { label: 'strict', value: 'true', note: 'Undeclared fields are violations, not silently passed.' },
    ],
    exitCodes: [
      { code: '0', meaning: 'clean — data conforms, diff acceptable', color: GREEN },
      { code: '1', meaning: 'the data or the diff violates the contract', color: RED },
      { code: '2', meaning: 'the run failed — bad flags, unreadable file, invalid contract', color: '#d2cefd' },
    ],
    enforcementPoints: [
      { title: 'CI file gate', dot: ACC, body: 'Blocks a merge when an exported file breaks the contract.', cmd: 'covenant check exports/*.parquet \\\n  -c orders.yaml', stat: '46 runs today · 0 failures' },
      { title: 'Stream gate', dot: ACC, body: 'Sits in the pipe: clean records out, violations to the DLQ.', cmd: 'kcat -C -t orders_raw -e \\\n  | covenant gate -c orders.yaml', stat: '4.2M records · 61 blocked (7 d)' },
      { title: 'Embedded (Arrow)', dot: '#4d5566', body: 'The same compiled contract validating RecordBatches in-process.', cmd: 'validate_batch(model, &batch,\n  rows, Some(&mut unique), &mut c)', stat: 'used by the nightly rollup job' },
    ],
    versionHistory: [
      { version: '1.2.0', title: 'amount_cents max raised to 5,000,000', by: 'j.park · merged', severity: 'risky', color: '#d2cefd', when: '41 days ago' },
      { version: '1.1.0', title: 'trace_id added as optional uuid', by: 'j.park · merged', severity: 'info', color: MUTED, when: '41 days ago' },
      { version: '1.0.0', title: 'Contract published, gate enabled on checkout.orders', by: 'a.silva · merged', severity: 'info', color: MUTED, when: '4 months ago' },
    ],

    authorTabs: seg(['Form', 'YAML'], authorMode, (v) => set({ authorMode: v })),
    isYamlMode: authorMode === 'YAML', isFormMode: authorMode === 'Form',
    editorHint: authorMode === 'YAML' ? 'The file in your repo — this is what CI runs' : 'Same contract, one row per field',
    yamlLines,
    formFields: [
      { name: 'order_id', type: 'string', rules: ['required', 'unique', 'pattern'], coverage: '100% conform' },
      { name: 'amount_cents', type: 'integer', rules: ['required', 'min 0', 'max 5000000'], coverage: '100% conform' },
      { name: 'currency', type: 'string', rules: ['required', 'allowed'], coverage: '99.9% conform' },
      { name: 'customer_email', type: 'string', rules: ['nullable', 'format email'], coverage: '74% present' },
      { name: 'created_at', type: 'timestamp', rules: ['required'], coverage: '100% conform' },
    ],
    lintFindings: [
      { level: 'error', color: RED, path: 'models.orders.fields.amount_cents', message: 'min 0 is greater than max -1 — the contract would never compile.' },
      { level: 'warning', color: '#d2cefd', path: 'models.orders.fields.trace_id', message: 'No constraints beyond type — consider required or a format.' },
      { level: 'warning', color: '#d2cefd', path: 'policy', message: 'sample_violations 10 with a 4M-row source keeps reports readable; counts stay exact.' },
    ],

    reportTabs: seg(['By rule', 'Sample rows'], reportLayout, (v) => set({ reportLayout: v })),
    isByRule: reportLayout === 'By rule', isBySample: reportLayout === 'Sample rows',
    ruleGroups: [
      { field: 'currency', rule: 'allowed', count: '198', pct: '48%', sample: 'row 44182: value "BTC" not in allowed set [EUR, GBP, USD]' },
      { field: 'amount_cents', rule: 'min', count: '141', pct: '34%', sample: 'row 44179: value -1500 is below min 0' },
      { field: 'settled_at', rule: 'required_missing', count: '52', pct: '13%', sample: 'row 44160: required field is absent from the record' },
      { field: 'payment_id', rule: 'unique', count: '18', pct: '4%', sample: 'row 44151: duplicate value — first seen at row 41022' },
      { field: 'ledger_ref', rule: 'unexpected_field', count: '3', pct: '1%', sample: 'row 44140: strict: true — field not declared in the contract' },
    ],
    sampleRows: [
      { row: '44182', field: 'currency', rule: 'allowed', value: '"BTC"' },
      { row: '44179', field: 'amount_cents', rule: 'min', value: '-1500' },
      { row: '44176', field: 'currency', rule: 'allowed', value: '"BTC"' },
      { row: '44160', field: 'settled_at', rule: 'required_missing', value: '<absent>' },
      { row: '44151', field: 'payment_id', rule: 'unique', value: '"pay_74aa0b19"' },
      { row: '44140', field: 'ledger_ref', rule: 'unexpected_field', value: '"L-8821"' },
      { row: '44122', field: 'amount_cents', rule: 'min', value: '-800' },
      { row: '44119', field: 'currency', rule: 'allowed', value: '"BTC"' },
    ],

    diffLegend: [
      { label: 'breaking — blocks the merge', color: RED },
      { label: 'risky — needs a minor bump', color: '#d2cefd' },
      { label: 'info', color: MUTED },
    ],
    diffFindings: [
      { severity: 'breaking', color: RED, path: 'models.payments.fields.legacy_ref', message: 'Field removed. Anything selecting it starts failing at the next run.', who: 'Consumers relying on this guarantee lose it with no warning.', impact: 'consumers' },
      { severity: 'breaking', color: RED, path: 'version', message: 'A breaking change requires a major bump; 3.1.0 → 3.2.0 is minor.', who: 'Semver discipline is enforced, not suggested.', impact: 'both' },
      { severity: 'risky', color: '#d2cefd', path: 'models.payments.fields.currency', message: 'allowed narrowed from [USD, EUR, GBP, JPY] to [USD, EUR, GBP].', who: 'Producers feel this first — conforming data starts failing.', impact: 'producers' },
      { severity: 'risky', color: '#d2cefd', path: 'models.payments.fields.amount_cents', message: 'integer → float: representational widening, not a flat break.', who: 'Consumers parsing strict integers should be checked.', impact: 'consumers' },
      { severity: 'info', color: MUTED, path: 'models.payments.fields.settled_at', message: 'Description updated. No enforcement change.', who: 'No action needed.', impact: '—' },
    ],
    consumers: [
      { name: 'finance_daily_rollup (dbt)', uses: 'legacy_ref, currency', impact: 'breaks', color: RED },
      { name: 'Looker · Revenue by currency', uses: 'currency', impact: 'breaks', color: RED },
      { name: 'ml-platform · churn features', uses: 'amount_cents', impact: 'needs review', color: '#d2cefd' },
      { name: 'ops alerting job', uses: 'settled_at', impact: 'safe', color: MUTED },
    ],

    gateStats: [
      { label: 'Records seen', value: '2,481,033', note: 'since 18:00', color: '#e9e9ed' },
      { label: 'Passed through', value: '2,480,209', note: '99.97%', color: GREEN },
      { label: 'Blocked to DLQ', value: '824', note: 'policy block', color: RED },
      { label: 'Added latency', value: '0.41 ms', note: 'p99 per record', color: '#e9e9ed' },
    ],
    throughput,
    liveRules: [
      { field: 'currency', rule: 'allowed', rate: '412/min', pct: '80%' },
      { field: 'amount_cents', rule: 'min', rate: '188/min', pct: '38%' },
      { field: 'settled_at', rule: 'required_missing', rate: '61/min', pct: '14%' },
      { field: 'payment_id', rule: 'unique', rate: '9/min', pct: '4%' },
    ],
    blockedStream: [
      { time: '19:04:11', key: 'pay_9f3a21c7 · part 3', why: 'currency "BTC" not in allowed set [EUR, GBP, USD]' },
      { time: '19:04:09', key: 'pay_20b7de40 · part 1', why: 'amount_cents -1500 is below min 0' },
      { time: '19:03:58', key: 'pay_5c1e88fa · part 2', why: 'settled_at required field is absent' },
      { time: '19:03:51', key: 'pay_74aa0b19 · part 0', why: 'payment_id duplicate — first seen at row 41022' },
      { time: '19:03:40', key: 'pay_b0c4d772 · part 3', why: 'ledger_ref not declared (strict: true)' },
    ],

    dlqList: dlq.map((d, i) => ({
      id: d.id, when: d.when, rules: d.rules, onClick: () => set({ dlqIndex: i, replayed: false }),
      bg: i === s.dlqIndex ? 'rgba(145,132,217,.12)' : 'transparent',
      mark: i === s.dlqIndex ? ACC : 'transparent',
    })),
    dlqDetail,
    replayLabel: s.replayed ? 'Queued for replay' : 'Replay record',
    replay: () => set({ replayed: true }),

    healthTabs: seg(['Heatmap', 'Cards'], healthLayout, (v) => set({ healthLayout: v })),
    isHeatmap: healthLayout === 'Heatmap', isCards: healthLayout === 'Cards',
    heatRows,
    teamCards: [
      { team: 'data-platform', dot: MUTED, contracts: '2 contracts', pass: '100%', blocked: '0', mttr: '—', color: GREEN, note: 'Both contracts gated in-stream. No blocked merges this fortnight.' },
      { team: 'payments-eng', dot: RED, contracts: '1 contract', pass: '99.9%', blocked: '3', mttr: '4 h', color: RED, note: 'checkout-svc 4.18.0 is emitting BTC and negative refunds.' },
      { team: 'growth-eng', dot: ACC, contracts: '1 contract', pass: '99.4%', blocked: '1', mttr: '2 d', color: '#d2cefd', note: 'Two undeclared columns appeared in public.customers.' },
      { team: 'supply-eng', dot: MUTED, contracts: '2 contracts', pass: '100%', blocked: '0', mttr: '—', color: GREEN, note: 'One contract still on on_violation: warn — reporting only.' },
      { team: 'ml-platform', dot: MUTED, contracts: '1 contract', pass: '99.8%', blocked: '1', mttr: '6 h', color: MUTED, note: 'Feature job embeds the Arrow engine directly.' },
      { team: 'unowned', dot: ACC, contracts: '14 tables', pass: '—', blocked: '—', mttr: '—', color: '#d2cefd', note: '14 warehouse tables have no contract at all. Analysts cannot verify them.' },
    ],

    settingRows: [
      { label: 'Default on_violation', note: 'block withholds the record; warn passes it and reports.', isChoice: true,
        options: seg(['block', 'warn'], s.onViolation, (v) => set({ onViolation: v })) },
      { label: 'Fail PRs on risky changes', note: 'Breaking always blocks. Risky is your call.', isToggle: true, options: [],
        trackBg: s.blockOnWarn ? 'rgba(145,132,217,.5)' : 'rgba(233,233,237,.12)',
        justify: s.blockOnWarn ? 'flex-end' : 'flex-start', knob: s.blockOnWarn ? '#e7e5fe' : '#75798c',
        onClick: () => set({ blockOnWarn: !s.blockOnWarn }) },
      { label: 'Write dead letters by default', note: 'Every gate gets a DLQ path unless overridden.', isToggle: true, options: [],
        trackBg: s.autoDlq ? 'rgba(145,132,217,.5)' : 'rgba(233,233,237,.12)',
        justify: s.autoDlq ? 'flex-end' : 'flex-start', knob: s.autoDlq ? '#e7e5fe' : '#75798c',
        onClick: () => set({ autoDlq: !s.autoDlq }) },
    ],
    routes: [
      { owner: 'data-platform@acme.io', target: '#data-platform-alerts (Slack)', when: 'on block' },
      { owner: 'payments-eng@acme.io', target: 'PagerDuty · payments primary', when: 'on block' },
      { owner: 'growth-eng@acme.io', target: 'growth-eng@acme.io (digest)', when: 'daily' },
    ],
    installSteps: [
      { n: '1', title: 'Install the binary', body: 'No runtime, no container, no agent. Works the same on a laptop and in CI.', cmd: 'curl -sSL covenant.sh/install | sh\ncovenant --version' },
      { n: '2', title: 'Scaffold and lint a contract', body: 'Start from the generated file, then edit it down to what you actually promise.', cmd: 'covenant init orders.yaml\ncovenant validate orders.yaml' },
      { n: '3', title: 'Gate a file in CI', body: 'Exit 1 fails the job. This is the whole integration.', cmd: 'covenant check exports/orders.parquet -c orders.yaml' },
      { n: '4', title: 'Put it in the data path', body: 'Transport-agnostic: pipe it between consumer and producer, or read a nightly dump.', cmd: 'kcat -C -t orders_raw -e \\\n  | covenant gate -c orders.yaml --dlq orders.dlq.ndjson \\\n  | kcat -P -t orders_validated' },
    ],
  };

  return overlay(vals, s, set, live);
}

/* Overlay real /v1 responses onto the demo vals. React escapes all rendered
   text, so server data needs no HTML escaping here. */
function overlay(
  v: Vals,
  s: ConsoleState,
  set: (patch: Partial<ConsoleState>) => void,
  live: LiveData,
): Vals {
  if (!live.up) return v;

  if (live.validate) {
    v.lintFindings = live.validate.findings.map((f) => ({
      level: f.level, color: SEV_COLOR[f.level] || MUTED, path: f.path, message: f.message,
    }));
    if (!v.lintFindings.length) {
      v.lintFindings = [{ level: 'ok', color: GREEN, path: 'contract', message: 'No findings — the contract lints clean.' }];
    }
  }

  if (live.check) {
    const report = live.check;
    const total = report.violations || 0;
    v.ruleGroups = report.per_rule.map((rc) => {
      const sample = report.samples.find((x) => (x.field || '') === rc.field && x.rule === rc.rule);
      return {
        field: rc.field || '<record>', rule: rc.rule, count: String(rc.count),
        pct: total ? Math.round((rc.count / total) * 100) + '%' : '0%',
        sample: sample ? sample.message : '—',
      };
    });
    v.sampleRows = report.samples.map((x) => ({
      row: x.row == null ? 'schema' : String(x.row), field: x.field || '<record>',
      rule: x.rule, value: x.value == null ? '—' : JSON.stringify(x.value),
    }));
  }

  if (live.diffR) {
    v.diffFindings = live.diffR.changes.map((c) => ({
      severity: c.severity, color: SEV_COLOR[c.severity] || MUTED, path: c.path, message: c.message,
      who: c.impact === 'producers' ? 'Producers feel this first — their data starts failing.'
        : c.impact === 'consumers' ? 'Consumers relying on this guarantee lose it.'
        : 'Both sides of the contract are affected.',
      impact: c.impact,
    }));
    const ci = live.diffR.consumer_impact;
    if (ci) {
      v.consumers = [
        ...ci.impacted.map((c) => ({
          name: c.consumer + (c.owner ? ' · ' + c.owner : ''),
          uses: c.fields.length ? c.fields.join(', ') : 'contract-wide',
          impact: c.severity === 'breaking' ? 'breaks'
            : c.severity === 'risky' ? 'needs review' : 'note',
          color: SEV_COLOR[c.severity] || MUTED,
        })),
        ...ci.unaffected.map((name) => ({ name, uses: '—', impact: 'safe', color: MUTED })),
      ];
    }
  }

  const gs = live.gateStats;
  if (gs?.configured && gs.stats) {
    const st = gs.stats;
    const fmt = (n: number) => n.toLocaleString('en-US');
    const share = (a: number, b: number) => (b ? ((a / b) * 100).toFixed(2) + '%' : '—');
    const stale = gs.age_seconds != null && gs.age_seconds > 30;
    v.gateStats = [
      { label: 'Records seen', value: fmt(st.records),
        note: stale ? `snapshot ${gs.age_seconds}s old — gate stopped?` : `since ${st.started_at.slice(11, 19)}Z`,
        color: '#e9e9ed' },
      { label: 'Passed through', value: fmt(st.passed), note: share(st.passed, st.records), color: GREEN },
      { label: 'Blocked to DLQ', value: fmt(st.blocked),
        note: st.warned ? `${fmt(st.warned)} warned through` : 'policy block',
        color: st.blocked ? RED : GREEN },
      { label: 'Added latency', value: st.p99_validate_micros != null
          ? (st.p99_validate_micros / 1000).toFixed(2) + ' ms' : '—',
        note: 'p99 validate (bucketed)', color: '#e9e9ed' },
    ];
    const recent = st.recent.slice(-36);
    const pad = Array.from({ length: Math.max(0, 36 - recent.length) }, () => ({ records: 0, blocked: 0 }));
    v.throughput = [...pad, ...recent].map((b) => {
      const bad = b.records ? Math.round((b.blocked / b.records) * 100) : 0;
      return { bad: bad + '%', good: 100 - bad + '%' };
    });
    const maxCount = Math.max(1, ...st.per_rule.map((r) => r.count));
    v.liveRules = st.per_rule.slice(0, 8).map((r) => ({
      field: r.field || '<record>', rule: r.rule,
      rate: '×' + fmt(r.count),
      pct: Math.round((r.count / maxCount) * 100) + '%',
    }));
  }

  const dl = live.dlq;
  if (dl?.configured && dl.entries) {
    const entries = dl.entries;
    const when = (e: DlqEnvelopeDoc) => (e.ts ? e.ts.slice(11, 19) : '—');
    const ruleSummary = (e: DlqEnvelopeDoc) => {
      const seen: string[] = [];
      for (const vv of e.violations ?? []) {
        const s = (vv.field || '<record>') + ' · ' + vv.rule;
        if (!seen.includes(s)) seen.push(s);
      }
      return seen.join(', ') || '—';
    };
    v.blockedStream = entries.slice(0, 5).map((e) => ({
      time: when(e), key: `row ${e.row ?? '?'}`, why: e.violations?.[0]?.message ?? '—',
    }));
    const idx = Math.max(0, Math.min(s.dlqIndex, entries.length - 1));
    v.dlqList = entries.map((e, i) => ({
      id: `row ${e.row ?? '?'}`, when: when(e), rules: ruleSummary(e),
      onClick: () => set({ dlqIndex: i, replayed: false }),
      bg: i === idx ? 'rgba(145,132,217,.12)' : 'transparent',
      mark: i === idx ? ACC : 'transparent',
    }));
    const sel = entries[idx];
    // JSON.stringify(undefined) is undefined — normalize the record so a
    // malformed envelope can never throw mid-render and blank the screen.
    v.dlqDetail = sel
      ? {
          id: `row ${sel.row ?? '?'}`, when: sel.ts ?? '—', rules: ruleSummary(sel),
          row: sel.row ?? 0, partition: '—',
          json: (JSON.stringify(sel.record ?? null, null, 2) ?? 'null').split('\n')
            .map((t) => ({ t, c: '#cfd3e5' })),
          violations: (sel.violations ?? []).map((vv) => ({
            field: vv.field || '<record>', rule: vv.rule, message: vv.message,
          })),
        }
      : {
          id: '—', when: '—', rules: '—', row: 0, partition: '—',
          json: [{ t: '(no dead letters in the DLQ file yet)', c: MUTED }],
          violations: [],
        };
    // Replay needs POST /v1/dlq/replay, which the runtime deliberately doesn't
    // carry — never pretend on live data.
    v.replayLabel = 'Replay — not available';
    v.replay = () => {};
  }

  if (live.contract) {
    const doc = live.contract.contract;
    // The server names the model it resolved and serves — trust that, not
    // whichever key happens to come first in the models map.
    const model = doc.models[live.contract.model] ?? null;
    if (model && s.contractId === doc.id) {
      v.detail = {
        id: doc.id, name: doc.name || doc.id, version: doc.version,
        owner: (doc.owner || '').split('@')[0] || '—', ownerEmail: doc.owner || '—',
        source: 'served · ' + (live.contract.source || 'contract'),
        // The runtime keeps no fleet history, so none of these demo stats
        // have a live backing — show them as absent rather than plausible.
        points: '—', last: '—', health: '—', status: '— (needs fleet history)',
      };
      v.detailFields = Object.entries(model.fields).map(([name, f]) => {
        const rules: string[] = [];
        if (f.required) rules.push('required');
        if (f.unique) rules.push('unique');
        if (f.nullable) rules.push('nullable');
        if (f.pattern) rules.push('pattern ' + f.pattern);
        if (f.min != null) rules.push('min ' + f.min);
        if (f.max != null) rules.push('max ' + f.max);
        if (f.allowed) rules.push('allowed ' + f.allowed.join('·'));
        if (f.format) rules.push('format ' + f.format);
        if (f.deprecated != null) rules.push('deprecated' + (f.deprecated ? ' — ' + f.deprecated : ''));
        if (!rules.length) rules.push('optional');
        return { name, type: f.type, rules, health: '— (needs fleet history)', healthColor: MUTED };
      });
      const p = doc.policy;
      v.policyRows = [
        { label: 'on_violation', value: p.on_violation, note: 'Dirty records are withheld and the run exits 1.' },
        { label: 'max_violations', value: String(p.max_violations), note: 'Tolerated budget for the whole run.' },
        { label: 'sample_violations', value: String(p.sample_violations), note: 'Examples kept per (field, rule); counts stay exact.' },
        { label: 'strict', value: String(model.strict), note: 'Undeclared fields are violations, not silently passed.' },
      ];
    }
  }
  return v;
}
