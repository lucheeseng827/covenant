// Typed client for the `covenant serve` /v1 API. The shapes mirror the
// serde serialization in src/{report,diff,spec}.rs — if those change, this
// file is the compile-time tripwire on the frontend side.

export interface LintFinding {
  level: 'error' | 'warning';
  path: string;
  message: string;
}

export interface ValidateResponse {
  findings: LintFinding[];
  enforceable: boolean;
}

export interface RuleCount {
  field: string;
  rule: string;
  count: number;
}

export interface ViolationSample {
  model: string;
  field?: string;
  rule: string;
  row?: number;
  value?: string;
  message: string;
}

export interface CheckReport {
  contract_id: string;
  contract_version: string;
  owner?: string;
  model: string;
  source: string;
  /** Present when the run was scoped to a consumer manifest
   *  (`check --as-consumer`) — the verdict covers only that consumer's
   *  declared fields, with strict mode off. */
  as_consumer?: string;
  rows: number;
  violations: number;
  per_rule: RuleCount[];
  samples: ViolationSample[];
}

export type DiffSeverity = 'info' | 'risky' | 'breaking';
export type DiffImpact = 'producers' | 'consumers' | 'both';

export interface DiffChange {
  severity: DiffSeverity;
  impact: DiffImpact;
  path: string;
  message: string;
}

export interface ImpactedConsumer {
  consumer: string;
  owner?: string;
  severity: DiffSeverity;
  fields: string[];
  changes: string[];
}

export interface ConsumerImpactReport {
  contract: string;
  manifests: number;
  consumers_of_contract: number;
  impacted: ImpactedConsumer[];
  unaffected: string[];
}

export interface DiffReport {
  old_version: string;
  new_version: string;
  changes: DiffChange[];
  /** Present only when consumer manifests were sent with the request. */
  consumer_impact?: ConsumerImpactReport;
}

export interface FieldDoc {
  type: string;
  required: boolean;
  nullable: boolean;
  unique: boolean;
  pattern?: string;
  min?: number;
  max?: number;
  min_length?: number;
  max_length?: number;
  allowed?: unknown[];
  format?: string;
  description?: string;
  /** Advisory deprecation marker — "" for a bare flag, else the migration note. */
  deprecated?: string;
}

export interface ModelDoc {
  strict: boolean;
  description?: string;
  fields: Record<string, FieldDoc>;
}

// All three fields are required: the Rust `Policy` serializes every field
// (no skip_serializing_if), and `Contract.policy` is #[serde(default)] with
// no skip either — a served contract always carries a complete policy.
export interface PolicyDoc {
  on_violation: 'block' | 'warn';
  max_violations: number;
  sample_violations: number;
}

export interface ContractDoc {
  covenant: number;
  id: string;
  version: string;
  name?: string;
  owner?: string;
  description?: string;
  models: Record<string, ModelDoc>;
  policy: PolicyDoc;
}

export interface ContractInfo {
  source: string;
  model: string;
  contract: ContractDoc;
  findings: LintFinding[];
}

export interface Health {
  status: string;
  version: string;
  contract: string;
  model: string;
}

/** One snapshot from `covenant gate --stats <path>` (the Phase-2 sidecar). */
export interface GateStatsSnapshot {
  covenant_gate_stats: number;
  contract: string;
  version: string;
  model: string;
  started_at: string;
  updated_at: string;
  records: number;
  passed: number;
  blocked: number;
  warned: number;
  /** Bucketed upper bound; null until a record has been timed. */
  p99_validate_micros?: number | null;
  per_rule: RuleCount[];
  /** Rolling 10-second throughput buckets, oldest first. */
  recent: { records: number; blocked: number }[];
}

export interface GateStatsResponse {
  configured: boolean;
  hint?: string;
  note?: string;
  stats?: GateStatsSnapshot;
  age_seconds?: number | null;
}

/** One dead letter from the gate's DLQ file (`ts` is additive — old files
 *  lack it). The server only serves envelope-shaped lines, but `record` and
 *  `violations` stay optional here so rendering must guard anyway — an
 *  older or foreign server must never blank the screen. */
export interface DlqEnvelopeDoc {
  contract_id?: string;
  contract_version?: string;
  model?: string;
  row?: number;
  ts?: string;
  record?: unknown;
  violations?: ViolationSample[];
}

export interface DlqResponse {
  configured: boolean;
  hint?: string;
  note?: string;
  source?: string;
  /** Newest first. Absent (vs empty) = the DLQ file was not readable. */
  entries?: DlqEnvelopeDoc[];
  skipped?: number;
  total_bytes?: number;
}

// ---- request plumbing: every call resolves to a discriminated result ----

/** Why a request failed: the server never answered / timed out (`network`),
 *  answered non-2xx (`status`, with the code), or answered 2xx with a body
 *  that is not the expected JSON (`parse`). */
export interface ApiError {
  reason: 'network' | 'status' | 'parse';
  status?: number;
}

export type ApiResult<T> = { ok: true; data: T } | ({ ok: false } & ApiError);

const TIMEOUT_MS = 10_000;

function timeoutSignal(): AbortSignal | undefined {
  // AbortSignal.timeout is baseline in 2024+ browsers; older ones just get
  // no timeout rather than a crash.
  return typeof AbortSignal !== 'undefined' && 'timeout' in AbortSignal
    ? AbortSignal.timeout(TIMEOUT_MS)
    : undefined;
}

async function request<T>(path: string, init?: RequestInit): Promise<ApiResult<T>> {
  let r: Response;
  try {
    r = await fetch(path, { ...init, signal: timeoutSignal() });
  } catch {
    return { ok: false, reason: 'network' };
  }
  if (!r.ok) return { ok: false, reason: 'status', status: r.status };
  try {
    return { ok: true, data: (await r.json()) as T };
  } catch {
    return { ok: false, reason: 'parse', status: r.status };
  }
}

const get = <T,>(path: string) => request<T>(path);

const post = <T,>(path: string, body: string, contentType: string) =>
  request<T>(path, { method: 'POST', body, headers: { 'content-type': contentType } });

export const api = {
  health: () => get<Health>('/v1/health'),
  contract: () => get<ContractInfo>('/v1/contract'),
  gateStats: () => get<GateStatsResponse>('/v1/gate/stats'),
  dlq: (limit = 50) => get<DlqResponse>(`/v1/dlq?limit=${limit}`),
  validate: (yaml: string) => post<ValidateResponse>('/v1/validate', yaml, 'application/yaml'),
  check: (ndjson: string) => post<CheckReport>('/v1/check', ndjson, 'application/x-ndjson'),
  diff: (oldYaml: string, newYaml: string, consumers?: string[]) =>
    post<DiffReport>(
      '/v1/diff',
      JSON.stringify({ old: oldYaml, new: newYaml, consumers }),
      'application/json',
    ),
};

// ---- demo payloads the live screens submit (mirror the bundled examples) ----

export const DEMO_YAML = [
  'covenant: 1', 'id: orders', 'version: 1.2.0', 'owner: data-platform@acme.io', '',
  'models:', '  orders:', '    strict: true', '    fields:',
  '      order_id:     { type: string, required: true, unique: true, pattern: "^ord_[a-z0-9]{12}$" }',
  '      amount_cents: { type: integer, required: true, min: 0, max: 5000000 }',
  '      currency:     { type: string, required: true, allowed: [USD, EUR, GBP] }',
  '      customer_email: { type: string, format: email, nullable: true }',
  '      trace_id:     { type: uuid }',
  '      created_at:   { type: timestamp, required: true }', '',
  'policy:', '  on_violation: block', '  max_violations: 0', '  sample_violations: 10',
].join('\n');

export const DEMO_NDJSON = [
  '{"order_id":"ord_a1b2c3d4e5f6","amount_cents":12999,"currency":"USD","created_at":"2026-08-11T09:30:00Z"}',
  '{"order_id":"ORD-UPPER","amount_cents":-1500,"currency":"BTC","created_at":"2026-08-11T09:31:00Z"}',
  '{"order_id":"ord_a1b2c3d4e5f6","amount_cents":100,"currency":"USD","created_at":"2026-08-11T09:32:00Z"}',
  '{"amount_cents":9001,"currency":"USD","customer_email":"not-an-email","created_at":"2026-08-11T09:33:00Z"}',
  '{"order_id":"ord_d4e5f6a1b2c3","amount_cents":100,"currency":"USD","created_at":"2026-08-11T09:34:00Z","ledger_ref":"L-8821"}',
].join('\n');

// The PR-gate demo diff is DEMO_YAML with edits applied by literal
// replacement. A stale literal would silently produce an empty diff, so
// every replacement asserts the literal still exists — editing DEMO_YAML
// without updating these fails loudly (in dev, on first load).
function mustReplace(src: string, from: string, to: string): string {
  if (!src.includes(from)) {
    throw new Error(`DEMO_DIFF_NEW is stale: literal ${JSON.stringify(from)} not found in DEMO_YAML`);
  }
  return src.replace(from, to);
}

// Consumer manifests for the demo blast radius. The manifests are exact
// copies of examples/consumers/ (same ids, owners, and field lists), but the
// console submits them against ITS demo diff (DEMO_DIFF_NEW below), not the
// repo's orders_v2.yaml — so the verdicts here differ from the README's CLI
// sample: customer_email's removal breaks the rollup, the integer→float
// widening flags every amount_cents reader, and ops_alerting stays safe.
export const DEMO_CONSUMERS = [
  [
    'consumer: 1', 'id: finance_daily_rollup', 'owner: finance-eng@acme.io',
    'consumes:', '  - contract: orders', '    fields: [order_id, customer_email, currency, amount_cents]',
  ].join('\n'),
  [
    'consumer: 1', 'id: looker_revenue', 'owner: analytics@acme.io',
    'consumes:', '  - contract: orders', '    fields: [currency, amount_cents, created_at]',
  ].join('\n'),
  [
    'consumer: 1', 'id: ml_churn_features', 'owner: ml-platform@acme.io',
    'consumes:', '  - contract: orders', '    fields: [order_id, amount_cents]',
  ].join('\n'),
  [
    'consumer: 1', 'id: ops_alerting', 'owner: ops@acme.io',
    'consumes:', '  - contract: orders', '    fields: [created_at]',
  ].join('\n'),
];

export const DEMO_DIFF_OLD = DEMO_YAML;
export const DEMO_DIFF_NEW = [
  ['version: 1.2.0', 'version: 1.2.1'],
  ['      customer_email: { type: string, format: email, nullable: true }\n', ''],
  ['allowed: [USD, EUR, GBP]', 'allowed: [USD, EUR]'],
  ['amount_cents: { type: integer,', 'amount_cents: { type: float,'],
].reduce((src, [from, to]) => mustReplace(src, from, to), DEMO_YAML);
