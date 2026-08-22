// App shell: console state + the live-endpoint boot. On mount the app
// probes /v1/health; when `covenant serve` answers, it loads the served
// contract (and selects it) and submits the demo payloads to /v1/validate,
// /v1/check and /v1/diff so the Editor / Check report / PR gate screens
// render REAL engine results. Unreachable server = demo mode, marked as
// such by every screen's endpoint chip.
import { useCallback, useEffect, useMemo, useState } from 'react';
import { api, DEMO_CONSUMERS, DEMO_DIFF_NEW, DEMO_DIFF_OLD, DEMO_NDJSON, DEMO_YAML } from './api';
import { ConsoleView } from './generated/ConsoleView';
import {
  emptyLive, initialState, renderVals,
  type ConsoleState, type LiveData,
} from './vals';

export default function App() {
  const [state, setState] = useState<ConsoleState>(initialState);
  const [live, setLive] = useState<LiveData>(emptyLive);

  const set = useCallback(
    (patch: Partial<ConsoleState>) => setState((prev) => ({ ...prev, ...patch })),
    [],
  );

  useEffect(() => {
    let cancelled = false;
    (async () => {
      const health = await api.health();
      if (!health.ok || cancelled) return;
      const contract = await api.contract();
      // Select the served contract so the Detail screen and its chip agree.
      if (contract.ok && contract.data.contract.id) {
        set({ contractId: contract.data.contract.id });
      }
      const [validate, check, diffR, gateStats, dlq] = await Promise.all([
        api.validate(DEMO_YAML),
        api.check(DEMO_NDJSON),
        api.diff(DEMO_DIFF_OLD, DEMO_DIFF_NEW, DEMO_CONSUMERS),
        api.gateStats(),
        api.dlq(50),
      ]);
      if (cancelled) return;
      // Serve is up (health answered); record per-endpoint failures so each
      // screen's chip can say "this endpoint failed" instead of pretending
      // demo data is live.
      const errors: LiveData['errors'] = {};
      if (!contract.ok) errors.contract = contract;
      if (!validate.ok) errors.validate = validate;
      if (!check.ok) errors.check = check;
      if (!diffR.ok) errors.diff = diffR;
      if (!gateStats.ok) errors.gate = gateStats;
      if (!dlq.ok) errors.dlq = dlq;
      setLive({
        up: true,
        version: health.data.version,
        contract: contract.ok ? contract.data : null,
        validate: validate.ok ? validate.data : null,
        check: check.ok ? check.data : null,
        diffR: diffR.ok ? diffR.data : null,
        gateStats: gateStats.ok ? gateStats.data : null,
        dlq: dlq.ok ? dlq.data : null,
        errors,
      });
    })();
    return () => { cancelled = true; };
  }, [set]);

  const v = useMemo(() => renderVals(state, set, live), [state, set, live]);
  return <ConsoleView v={v} />;
}
