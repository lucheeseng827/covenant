/* eslint-disable */
// GENERATED from the Claude Design mock "Covenant Console.dc.html" — the
// markup and inline styles are the mock's own, mechanically converted to
// TSX (1-to-1). Do not edit by hand; regenerate with the converter script
// kept alongside the mock export. Interpolation is JSX, so React escapes
// all data — server responses included — by construction.
import { Fragment } from 'react';
import type { Vals } from '../vals';

export function ConsoleView({ v }: { v: Vals }) {
  const { authorTabs, blockedStream, consumers, detail, detailEnforceTab, detailFields, detailFieldsTab, detailHistoryTab, detailPolicyTab, detailTabs, diffFindings, diffLegend, dlqDetail, dlqList, driftEvents, editorHint, enforcementPoints, exitCodes, formFields, gateStats, goDetail, goDlq, goEditor, goRegistry, goReport, healthTabs, heatRows, installSteps, isAlerts, isByRule, isBySample, isCards, isDetail, isDiff, isDlq, isEditor, isFormMode, isGate, isHeatmap, isInstall, isRegistry, isReport, isTeams, isTrust, isYamlMode, lintFindings, liveRules, navGroups, noop, onQuery, policyRows, query, registryCount, registryFilters, replay, replayLabel, reportTabs, routes, ruleGroups, sampleRows, screenSub, screenTitle, settingRows, teamCards, throughput, trustGuarantees, trustStats, versionHistory, visibleContracts, yamlLines } = v;
  return (
    <>


<div style={{display: 'flex', height: '100vh', overflow: 'hidden', fontFamily: 'var(--font-body)', fontSize: '14px'}}>

  <nav style={{width: '236px', flex: 'none', display: 'flex', flexDirection: 'column', background: '#12131f', borderRight: '1px solid var(--color-divider)'}}>
    <div style={{display: 'flex', alignItems: 'center', gap: '9px', padding: '18px 16px 14px'}}>
      <div style={{width: '22px', height: '22px', border: '1.5px solid var(--color-accent)', borderRadius: '6px', display: 'flex', alignItems: 'center', justifyContent: 'center'}}>
        <div style={{width: '7px', height: '7px', background: 'var(--color-accent)', borderRadius: '2px'}}></div>
      </div>
      <div style={{fontFamily: 'var(--font-heading)', fontWeight: '500', fontSize: '15px', letterSpacing: '-0.01em'}}>Covenant</div>
    </div>

    <div style={{flex: '1', overflowY: 'auto', padding: '4px 10px 16px', display: 'flex', flexDirection: 'column', gap: '14px'}}>
      {(navGroups).map((group, group_i) => (<Fragment key={group_i}>
        <div style={{display: 'flex', flexDirection: 'column', gap: '2px'}}>
          <div style={{fontSize: '10px', letterSpacing: '.09em', textTransform: 'uppercase', color: '#75798c', padding: '6px 8px 4px'}}>{group.title}</div>
          {(group.items).map((item, item_i) => (<Fragment key={item_i}>
            <div onClick={item.onClick} style={{display: 'flex', alignItems: 'center', gap: '8px', padding: '7px 8px', borderRadius: '6px', cursor: 'pointer', color: item.color, background: item.bg, fontSize: '13.5px'}} data-hv="b">
              <div style={{width: '3px', height: '14px', borderRadius: '2px', background: item.mark}}></div>
              <span>{item.label}</span>
              <span style={{marginLeft: 'auto', fontSize: '11px', color: item.badgeColor}}>{item.badge}</span>
            </div>
          </Fragment>))}
        </div>
      </Fragment>))}
    </div>

    <div style={{padding: '12px 14px', borderTop: '1px solid var(--color-divider)', display: 'flex', alignItems: 'center', gap: '9px'}}>
      <div style={{width: '26px', height: '26px', borderRadius: '50%', background: 'var(--color-accent-800)', color: 'var(--color-accent-200)', display: 'flex', alignItems: 'center', justifyContent: 'center', fontSize: '11px'}}>AM</div>
      <div style={{lineHeight: '1.25'}}>
        <div style={{fontSize: '12.5px'}}>Ana Mehta</div>
        <div style={{fontSize: '11px', color: '#75798c'}}>Analytics · viewer</div>
      </div>
    </div>
  </nav>

  <main style={{flex: '1', display: 'flex', flexDirection: 'column', minWidth: '0'}}>
    <header style={{flex: 'none', display: 'flex', alignItems: 'center', gap: '14px', padding: '0 24px', height: '56px', borderBottom: '1px solid var(--color-divider)', background: '#191b29'}}>
      <div style={{minWidth: '0'}}>
        <div style={{fontFamily: 'var(--font-heading)', fontSize: '15px', letterSpacing: '-0.01em'}}>{screenTitle}</div>
        <div style={{fontSize: '11.5px', color: '#8b8f9f', whiteSpace: 'nowrap', overflow: 'hidden', textOverflow: 'ellipsis'}}>{screenSub}</div>
      </div>
      <div style={{marginLeft: 'auto', display: 'flex', alignItems: 'center', gap: '10px'}}>
        <div style={{display: 'flex', alignItems: 'center', gap: '7px', border: '1px solid var(--color-divider)', borderRadius: '8px', padding: '5px 10px', color: '#b2b6ca', fontSize: '12.5px', minWidth: '230px'}}>
          <span style={{color: '#75798c'}}>⌕</span>
          <input value={query} onChange={onQuery} placeholder="Search contracts, fields, topics" style={{border: '0', background: 'transparent', color: 'inherit', font: 'inherit', outline: 'none', width: '100%'}} />
        </div>
        <div style={{display: 'flex', alignItems: 'center', gap: '6px', border: '1px solid var(--color-divider)', borderRadius: '8px', padding: '5px 10px', fontSize: '12.5px', color: '#b2b6ca'}}>
          <div style={{width: '6px', height: '6px', borderRadius: '50%', background: '#84d9a8'}}></div>production
        </div>{v.epChip}
      </div>
    </header>

    <div style={{flex: '1', overflowY: 'auto', padding: '24px 28px 56px'}}>

      {(isTrust) ? (<Fragment>
      <div style={{maxWidth: '1080px', display: 'flex', flexDirection: 'column', gap: '20px'}}>
        <div style={{background: 'var(--color-surface)', borderRadius: 'var(--radius-lg)', boxShadow: 'var(--shadow-sm)', overflow: 'hidden'}}>
          <div style={{display: 'flex', gap: '28px', padding: '22px 24px', alignItems: 'flex-start', borderBottom: '1px solid var(--color-divider)'}}>
            <div style={{flex: '1', minWidth: '0'}}>
              <div style={{fontSize: '11px', letterSpacing: '.09em', textTransform: 'uppercase', color: '#75798c', marginBottom: '6px'}}>Can I trust this table?</div>
              <div style={{display: 'flex', alignItems: 'baseline', gap: '12px', flexWrap: 'wrap'}}>
                <h3 style={{margin: '0', fontSize: '26px'}}>analytics.orders</h3>
                <span style={{fontSize: '12.5px', color: '#8b8f9f', fontFamily: 'ui-monospace,Menlo,monospace'}}>contract orders v1.2.0</span>
              </div>
              <p style={{margin: '10px 0 0', color: '#b2b6ca', fontSize: '13.5px', maxWidth: '60ch', textWrap: 'pretty'}}>Every row that reached this table passed the contract at the producer boundary. Nothing here is sampled after the fact — the checks run in the pipeline, so a row you can see is a row that conformed.</p>
            </div>
            <div style={{flex: 'none', textAlign: 'right'}}>
              <div style={{display: 'inline-flex', alignItems: 'center', gap: '8px', border: '1px solid #3d6b54', background: 'rgba(132,217,168,.10)', color: '#84d9a8', borderRadius: '999px', padding: '6px 13px', fontSize: '13px'}}>
                <div style={{width: '7px', height: '7px', borderRadius: '50%', background: '#84d9a8'}}></div>Trusted
              </div>
              <div style={{fontSize: '11.5px', color: '#75798c', marginTop: '8px'}}>enforced at 3 boundaries</div>
            </div>
          </div>
          <div style={{display: 'grid', gridTemplateColumns: 'repeat(4,1fr)'}}>
            {(trustStats).map((s, s_i) => (<Fragment key={s_i}>
              <div style={{padding: '16px 20px', borderRight: '1px solid var(--color-divider)'}}>
                <div style={{fontSize: '11px', letterSpacing: '.07em', textTransform: 'uppercase', color: '#75798c'}}>{s.label}</div>
                <div style={{fontFamily: 'var(--font-heading)', fontSize: '22px', marginTop: '5px', color: s.color}}>{s.value}</div>
                <div style={{fontSize: '11.5px', color: '#8b8f9f', marginTop: '2px'}}>{s.note}</div>
              </div>
            </Fragment>))}
          </div>
        </div>

        <div style={{display: 'grid', gridTemplateColumns: '1.25fr 1fr', gap: '20px', alignItems: 'start'}}>
          <div style={{background: 'var(--color-surface)', borderRadius: 'var(--radius-lg)', boxShadow: 'var(--shadow-sm)', padding: '18px 20px'}}>
            <div style={{display: 'flex', alignItems: 'center', justifyContent: 'space-between', marginBottom: '12px'}}>
              <h5 style={{margin: '0'}}>What the contract guarantees</h5>
              <div onClick={goDetail} className="btn btn-ghost" style={{fontSize: '12.5px'}}>See all 6 fields →</div>
            </div>
            {(trustGuarantees).map((g, g_i) => (<Fragment key={g_i}>
              <div style={{display: 'flex', gap: '12px', padding: '9px 0', borderBottom: '1px solid rgba(233,233,237,.07)'}}>
                <div style={{width: '150px', flex: 'none', fontFamily: 'ui-monospace,Menlo,monospace', fontSize: '12.5px', color: 'var(--color-accent-300)'}}>{g.field}</div>
                <div style={{fontSize: '13px', color: '#cfd3e5', flex: '1'}}>{g.promise}</div>
                <div style={{fontSize: '11.5px', color: '#75798c', flex: 'none'}}>{g.evidence}</div>
              </div>
            </Fragment>))}
          </div>

          <div style={{background: 'var(--color-surface)', borderRadius: 'var(--radius-lg)', boxShadow: 'var(--shadow-sm)', padding: '18px 20px'}}>
            <h5 style={{margin: '0 0 12px'}}>Drift history</h5>
            {(driftEvents).map((e, e_i) => (<Fragment key={e_i}>
              <div style={{display: 'flex', gap: '12px', padding: '10px 0'}}>
                <div style={{flex: 'none', display: 'flex', flexDirection: 'column', alignItems: 'center', gap: '4px'}}>
                  <div style={{width: '9px', height: '9px', borderRadius: '50%', background: e.color, marginTop: '4px'}}></div>
                  <div style={{width: '1px', flex: '1', background: 'rgba(233,233,237,.12)'}}></div>
                </div>
                <div style={{minWidth: '0'}}>
                  <div style={{fontSize: '13px', color: '#e9e9ed'}}>{e.title}</div>
                  <div style={{fontSize: '12px', color: '#8b8f9f', textWrap: 'pretty'}}>{e.detail}</div>
                  <div style={{fontSize: '11px', color: '#696d7d', marginTop: '2px'}}>{e.when}</div>
                </div>
              </div>
            </Fragment>))}
          </div>
        </div>

        <div style={{display: 'flex', gap: '12px', alignItems: 'center', background: 'rgba(145,132,217,.07)', border: '1px solid var(--color-accent-800)', borderRadius: 'var(--radius-md)', padding: '13px 16px'}}>
          <div style={{fontSize: '13px', color: '#cfd3e5', flex: '1', textWrap: 'pretty'}}>Downstream of a table that isn't under contract? Ask its owner to add one — Covenant scaffolds the contract from a sample of the data.</div>
          <div onClick={goEditor} className="btn btn-primary" style={{flex: 'none'}}>Draft a contract</div>
        </div>
      </div>
      </Fragment>) : null}

      {(isRegistry) ? (<Fragment>
      <div style={{display: 'flex', flexDirection: 'column', gap: '16px'}}>
        <div style={{display: 'flex', alignItems: 'center', gap: '10px'}}>
          {(registryFilters).map((f, f_i) => (<Fragment key={f_i}>
            <div onClick={f.onClick} style={{cursor: 'pointer', fontSize: '12.5px', padding: '5px 11px', borderRadius: '999px', border: `1px solid ${f.border}`, color: f.color, background: f.bg}}>{f.label}</div>
          </Fragment>))}
          <div style={{marginLeft: 'auto', fontSize: '12.5px', color: '#8b8f9f'}}>{registryCount}</div>
        </div>
        <div style={{background: 'var(--color-surface)', borderRadius: 'var(--radius-lg)', boxShadow: 'var(--shadow-sm)', overflow: 'hidden'}}>
          <div style={{display: 'grid', gridTemplateColumns: '1.6fr 1.5fr .7fr 1fr .9fr 1fr', gap: '12px', padding: '11px 20px', borderBottom: '1px solid var(--color-divider)', fontSize: '11px', letterSpacing: '.07em', textTransform: 'uppercase', color: '#75798c'}}>
            <div>Contract</div><div>Boundary</div><div>Version</div><div>Owner</div><div>Enforced</div><div>Last check</div>
          </div>
          {(visibleContracts).map((c, c_i) => (<Fragment key={c_i}>
            <div onClick={c.onClick} style={{display: 'grid', gridTemplateColumns: '1.6fr 1.5fr .7fr 1fr .9fr 1fr', gap: '12px', padding: '13px 20px', borderBottom: '1px solid rgba(233,233,237,.07)', cursor: 'pointer', alignItems: 'center'}} data-hv="a">
              <div style={{display: 'flex', alignItems: 'center', gap: '10px', minWidth: '0'}}>
                <div style={{width: '7px', height: '7px', borderRadius: '50%', background: c.dot, flex: 'none'}}></div>
                <div style={{minWidth: '0'}}>
                  <div style={{fontSize: '13.5px', whiteSpace: 'nowrap', overflow: 'hidden', textOverflow: 'ellipsis'}}>{c.id}</div>
                  <div style={{fontSize: '11.5px', color: '#75798c'}}>{c.status}</div>
                </div>
              </div>
              <div style={{fontFamily: 'ui-monospace,Menlo,monospace', fontSize: '12px', color: '#b2b6ca', whiteSpace: 'nowrap', overflow: 'hidden', textOverflow: 'ellipsis'}}>{c.source}</div>
              <div style={{fontSize: '12.5px', color: '#b2b6ca'}}>{c.version}</div>
              <div style={{fontSize: '12.5px', color: '#b2b6ca'}}>{c.owner}</div>
              <div style={{fontSize: '12px', color: '#8b8f9f'}}>{c.points}</div>
              <div style={{fontSize: '12.5px', color: c.lastColor}}>{c.last}</div>
            </div>
          </Fragment>))}
        </div>
      </div>
      </Fragment>) : null}

      {(isDetail) ? (<Fragment>
      <div style={{display: 'flex', flexDirection: 'column', gap: '18px', maxWidth: '1120px'}}>
        <div style={{display: 'flex', alignItems: 'flex-start', gap: '20px'}}>
          <div style={{flex: '1'}}>
            <h3 style={{margin: '0 0 4px'}}>{detail.name}</h3>
            <div style={{fontSize: '12.5px', color: '#8b8f9f', fontFamily: 'ui-monospace,Menlo,monospace'}}>{detail.source} · owner {detail.ownerEmail}</div>
          </div>
          <div onClick={goEditor} className="btn btn-secondary">Edit contract</div>
          <div onClick={goReport} className="btn btn-primary">Run check</div>
        </div>

        <div style={{display: 'flex', gap: '4px', borderBottom: '1px solid var(--color-divider)'}}>
          {(detailTabs).map((t, t_i) => (<Fragment key={t_i}>
            <div onClick={t.onClick} style={{cursor: 'pointer', padding: '8px 13px', fontSize: '13px', color: t.color, borderBottom: `2px solid ${t.border}`, marginBottom: '-1px'}}>{t.label}</div>
          </Fragment>))}
        </div>

        {(detailFieldsTab) ? (<Fragment>
        <div style={{background: 'var(--color-surface)', borderRadius: 'var(--radius-lg)', boxShadow: 'var(--shadow-sm)', overflow: 'hidden'}}>
          <div style={{display: 'grid', gridTemplateColumns: '1.2fr .8fr 2.2fr .9fr', gap: '12px', padding: '11px 20px', borderBottom: '1px solid var(--color-divider)', fontSize: '11px', letterSpacing: '.07em', textTransform: 'uppercase', color: '#75798c'}}>
            <div>Field</div><div>Type</div><div>Enforced constraints</div><div>Health</div>
          </div>
          {(detailFields).map((f, f_i) => (<Fragment key={f_i}>
            <div style={{display: 'grid', gridTemplateColumns: '1.2fr .8fr 2.2fr .9fr', gap: '12px', padding: '12px 20px', borderBottom: '1px solid rgba(233,233,237,.07)', alignItems: 'center'}}>
              <div style={{fontFamily: 'ui-monospace,Menlo,monospace', fontSize: '12.5px', color: 'var(--color-accent-300)'}}>{f.name}</div>
              <div style={{fontSize: '12.5px', color: '#b2b6ca'}}>{f.type}</div>
              <div style={{display: 'flex', flexWrap: 'wrap', gap: '5px'}}>
                {(f.rules).map((r, r_i) => (<Fragment key={r_i}>
                  <span style={{fontFamily: 'ui-monospace,Menlo,monospace', fontSize: '11.5px', color: '#cfd3e5', background: 'rgba(233,233,237,.07)', borderRadius: '4px', padding: '2px 7px'}}>{r}</span>
                </Fragment>))}
              </div>
              <div style={{fontSize: '12.5px', color: f.healthColor}}>{f.health}</div>
            </div>
          </Fragment>))}
        </div>
        </Fragment>) : null}

        {(detailPolicyTab) ? (<Fragment>
        <div style={{display: 'grid', gridTemplateColumns: '1fr 1fr', gap: '18px'}}>
          <div style={{background: 'var(--color-surface)', borderRadius: 'var(--radius-lg)', boxShadow: 'var(--shadow-sm)', padding: '18px 20px'}}>
            <h5 style={{margin: '0 0 12px'}}>Violation policy</h5>
            {(policyRows).map((p, p_i) => (<Fragment key={p_i}>
              <div style={{display: 'flex', justifyContent: 'space-between', gap: '12px', padding: '9px 0', borderBottom: '1px solid rgba(233,233,237,.07)'}}>
                <div>
                  <div style={{fontSize: '13px'}}>{p.label}</div>
                  <div style={{fontSize: '11.5px', color: '#8b8f9f', textWrap: 'pretty'}}>{p.note}</div>
                </div>
                <div style={{fontFamily: 'ui-monospace,Menlo,monospace', fontSize: '12.5px', color: 'var(--color-accent-300)', flex: 'none'}}>{p.value}</div>
              </div>
            </Fragment>))}
          </div>
          <div style={{background: 'var(--color-surface)', borderRadius: 'var(--radius-lg)', boxShadow: 'var(--shadow-sm)', padding: '18px 20px'}}>
            <h5 style={{margin: '0 0 12px'}}>Exit-code contract</h5>
            <p style={{fontSize: '13px', color: '#b2b6ca', textWrap: 'pretty'}}>CI keys off these codes. They do not change between versions.</p>
            {(exitCodes).map((e, e_i) => (<Fragment key={e_i}>
              <div style={{display: 'flex', gap: '12px', padding: '8px 0', alignItems: 'baseline'}}>
                <div style={{fontFamily: 'ui-monospace,Menlo,monospace', fontSize: '13px', color: e.color, width: '22px'}}>{e.code}</div>
                <div style={{fontSize: '12.5px', color: '#cfd3e5'}}>{e.meaning}</div>
              </div>
            </Fragment>))}
          </div>
        </div>
        </Fragment>) : null}

        {(detailEnforceTab) ? (<Fragment>
        <div style={{display: 'grid', gridTemplateColumns: 'repeat(3,1fr)', gap: '18px'}}>
          {(enforcementPoints).map((p, p_i) => (<Fragment key={p_i}>
            <div style={{background: 'var(--color-surface)', borderRadius: 'var(--radius-lg)', boxShadow: 'var(--shadow-sm)', padding: '18px 20px', display: 'flex', flexDirection: 'column', gap: '9px'}}>
              <div style={{display: 'flex', alignItems: 'center', gap: '8px'}}>
                <div style={{width: '8px', height: '8px', borderRadius: '2px', background: p.dot}}></div>
                <div style={{fontFamily: 'var(--font-heading)', fontSize: '15px'}}>{p.title}</div>
              </div>
              <div style={{fontSize: '12.5px', color: '#b2b6ca', textWrap: 'pretty'}}>{p.body}</div>
              <div style={{fontFamily: 'ui-monospace,Menlo,monospace', fontSize: '11.5px', color: 'var(--color-accent-300)', background: '#12131f', borderRadius: '6px', padding: '9px 10px', overflowX: 'auto', whiteSpace: 'pre'}}>{p.cmd}</div>
              <div style={{fontSize: '11.5px', color: '#75798c', marginTop: 'auto'}}>{p.stat}</div>
            </div>
          </Fragment>))}
        </div>
        </Fragment>) : null}

        {(detailHistoryTab) ? (<Fragment>
        <div style={{background: 'var(--color-surface)', borderRadius: 'var(--radius-lg)', boxShadow: 'var(--shadow-sm)', padding: '18px 20px'}}>
          {(versionHistory).map((v, v_i) => (<Fragment key={v_i}>
            <div style={{display: 'flex', gap: '16px', padding: '12px 0', borderBottom: '1px solid rgba(233,233,237,.07)', alignItems: 'center'}}>
              <div style={{fontFamily: 'ui-monospace,Menlo,monospace', fontSize: '13px', color: 'var(--color-accent-300)', width: '70px', flex: 'none'}}>{v.version}</div>
              <div style={{flex: '1', minWidth: '0'}}>
                <div style={{fontSize: '13px'}}>{v.title}</div>
                <div style={{fontSize: '11.5px', color: '#8b8f9f'}}>{v.by}</div>
              </div>
              <div style={{fontSize: '11.5px', color: v.color, border: `1px solid ${v.color}`, borderRadius: '999px', padding: '2px 9px', flex: 'none'}}>{v.severity}</div>
              <div style={{fontSize: '11.5px', color: '#75798c', width: '90px', textAlign: 'right', flex: 'none'}}>{v.when}</div>
            </div>
          </Fragment>))}
        </div>
        </Fragment>) : null}
      </div>
      </Fragment>) : null}

      {(isEditor) ? (<Fragment>
      <div style={{display: 'grid', gridTemplateColumns: '1fr 320px', gap: '20px', alignItems: 'start'}}>
        <div style={{background: 'var(--color-surface)', borderRadius: 'var(--radius-lg)', boxShadow: 'var(--shadow-sm)', overflow: 'hidden'}}>
          <div style={{display: 'flex', alignItems: 'center', gap: '10px', padding: '12px 18px', borderBottom: '1px solid var(--color-divider)'}}>
            <div style={{display: 'flex', border: '1px solid var(--color-divider)', borderRadius: '8px', overflow: 'hidden'}}>
              {(authorTabs).map((t, t_i) => (<Fragment key={t_i}>
                <div onClick={t.onClick} style={{cursor: 'pointer', padding: '5px 14px', fontSize: '12.5px', color: t.color, background: t.bg}}>{t.label}</div>
              </Fragment>))}
            </div>
            <div style={{fontSize: '12px', color: '#75798c'}}>{editorHint}</div>
            <div style={{marginLeft: 'auto', display: 'flex', gap: '8px'}}>
              <div className="btn btn-secondary" style={{fontSize: '12.5px'}}>Validate</div>
              <div className="btn btn-primary" style={{fontSize: '12.5px'}}>Propose change</div>
            </div>
          </div>

          {(isYamlMode) ? (<Fragment>
          <div style={{background: '#12131f', padding: '16px 18px', fontFamily: 'ui-monospace,Menlo,monospace', fontSize: '12.5px', lineHeight: '1.75', overflowX: 'auto'}}>
            {(yamlLines).map((l, l_i) => (<Fragment key={l_i}>
              <div style={{whiteSpace: 'pre', color: l.c}}>{l.t}</div>
            </Fragment>))}
          </div>
          </Fragment>) : null}

          {(isFormMode) ? (<Fragment>
          <div style={{padding: '6px 0'}}>
            {(formFields).map((f, f_i) => (<Fragment key={f_i}>
              <div style={{display: 'grid', gridTemplateColumns: '1.1fr .8fr 2fr auto', gap: '12px', alignItems: 'center', padding: '12px 18px', borderBottom: '1px solid rgba(233,233,237,.07)'}}>
                <input value={f.name} onChange={noop} className="input" style={{fontFamily: 'ui-monospace,Menlo,monospace', fontSize: '12.5px', background: '#12131f', border: '1px solid var(--color-divider)', borderRadius: '6px', padding: '6px 9px', color: 'var(--color-accent-300)', outline: 'none'}} />
                <div style={{fontSize: '12.5px', color: '#b2b6ca', border: '1px solid var(--color-divider)', borderRadius: '6px', padding: '6px 9px'}}>{f.type} ▾</div>
                <div style={{display: 'flex', flexWrap: 'wrap', gap: '5px'}}>
                  {(f.rules).map((r, r_i) => (<Fragment key={r_i}>
                    <span style={{fontFamily: 'ui-monospace,Menlo,monospace', fontSize: '11.5px', color: '#cfd3e5', background: 'rgba(233,233,237,.07)', borderRadius: '4px', padding: '2px 7px'}}>{r}</span>
                  </Fragment>))}
                  <span style={{fontSize: '11.5px', color: 'var(--color-accent-300)', border: '1px dashed var(--color-accent-700)', borderRadius: '4px', padding: '2px 7px', cursor: 'pointer'}}>+ rule</span>
                </div>
                <div style={{fontSize: '12px', color: '#75798c', textAlign: 'right'}}>{f.coverage}</div>
              </div>
            </Fragment>))}
            <div style={{padding: '12px 18px'}}><span className="btn btn-ghost" style={{fontSize: '12.5px'}}>+ Add field</span></div>
          </div>
          </Fragment>) : null}
        </div>

        <div style={{display: 'flex', flexDirection: 'column', gap: '16px'}}>
          <div style={{background: 'var(--color-surface)', borderRadius: 'var(--radius-lg)', boxShadow: 'var(--shadow-sm)', padding: '16px 18px'}}>
            <h5 style={{margin: '0 0 10px'}}>Linter</h5>
            {(lintFindings).map((f, f_i) => (<Fragment key={f_i}>
              <div style={{display: 'flex', gap: '9px', padding: '8px 0', borderBottom: '1px solid rgba(233,233,237,.07)'}}>
                <div style={{fontSize: '11px', color: f.color, textTransform: 'uppercase', letterSpacing: '.06em', flex: 'none', width: '56px'}}>{f.level}</div>
                <div style={{minWidth: '0'}}>
                  <div style={{fontFamily: 'ui-monospace,Menlo,monospace', fontSize: '11.5px', color: '#b2b6ca'}}>{f.path}</div>
                  <div style={{fontSize: '12.5px', color: '#cfd3e5', textWrap: 'pretty'}}>{f.message}</div>
                </div>
              </div>
            </Fragment>))}
            <div style={{fontSize: '12px', color: '#75798c', marginTop: '10px'}}>A contract with an error-level finding will not compile — it never half-enforces.</div>
          </div>
          <div style={{background: 'rgba(145,132,217,.07)', border: '1px solid var(--color-accent-800)', borderRadius: 'var(--radius-lg)', padding: '16px 18px'}}>
            <div style={{display: 'flex', alignItems: 'center', gap: '8px', marginBottom: '6px'}}>
              <div style={{fontSize: '13px'}}>Infer from sample</div>
            </div>
            <div style={{fontSize: '12.5px', color: '#b2b6ca', textWrap: 'pretty'}}>Point Covenant at 10k live records and it proposes types, ranges and allowed-sets, with the observed frequency behind each one.</div>
          </div>
        </div>
      </div>
      </Fragment>) : null}

      {(isReport) ? (<Fragment>
      <div style={{display: 'flex', flexDirection: 'column', gap: '18px', maxWidth: '1120px'}}>
        <div style={{background: 'var(--color-surface)', borderRadius: 'var(--radius-lg)', boxShadow: 'var(--shadow-sm)', overflow: 'hidden'}}>
          <div style={{display: 'flex', alignItems: 'center', gap: '16px', padding: '16px 20px', background: 'rgba(217,138,138,.08)', borderBottom: '1px solid var(--color-divider)'}}>
            <div style={{fontFamily: 'var(--font-heading)', fontSize: '15px', color: '#d98a8a', border: '1px solid #8a4d4d', borderRadius: '6px', padding: '4px 12px'}}>FAIL</div>
            <div style={{minWidth: '0'}}>
              <div style={{fontFamily: 'ui-monospace,Menlo,monospace', fontSize: '13px'}}>exports/payments_2026-08-11.parquet</div>
              <div style={{fontSize: '12px', color: '#8b8f9f'}}>payments v3.1.0 · model payments · 880,142 rows checked · 412 violations · budget 0</div>
            </div>
            <div style={{marginLeft: 'auto', display: 'flex', gap: '8px', flex: 'none'}}>
              <div className="btn btn-secondary" style={{fontSize: '12.5px'}}>Download JSON</div>
              <div className="btn btn-primary" style={{fontSize: '12.5px'}}>Notify payments-eng</div>
            </div>
          </div>
          <div style={{display: 'flex', alignItems: 'center', gap: '12px', padding: '12px 20px', borderBottom: '1px solid var(--color-divider)'}}>
            <div style={{fontSize: '12.5px', color: '#8b8f9f'}}>Show violations as</div>
            <div style={{display: 'flex', border: '1px solid var(--color-divider)', borderRadius: '8px', overflow: 'hidden'}}>
              {(reportTabs).map((t, t_i) => (<Fragment key={t_i}>
                <div onClick={t.onClick} style={{cursor: 'pointer', padding: '5px 14px', fontSize: '12.5px', color: t.color, background: t.bg}}>{t.label}</div>
              </Fragment>))}
            </div>
            <div style={{marginLeft: 'auto', fontSize: '12px', color: '#75798c'}}>Counts are exact; samples are capped at 10 per rule.</div>
          </div>

          {(isByRule) ? (<Fragment>
          <div style={{padding: '6px 0'}}>
            {(ruleGroups).map((g, g_i) => (<Fragment key={g_i}>
              <div style={{padding: '14px 20px', borderBottom: '1px solid rgba(233,233,237,.07)'}}>
                <div style={{display: 'flex', alignItems: 'center', gap: '12px'}}>
                  <div style={{fontFamily: 'ui-monospace,Menlo,monospace', fontSize: '13px', color: 'var(--color-accent-300)', width: '180px', flex: 'none'}}>{g.field}</div>
                  <div style={{fontFamily: 'ui-monospace,Menlo,monospace', fontSize: '12.5px', color: '#cfd3e5', width: '140px', flex: 'none'}}>{g.rule}</div>
                  <div style={{flex: '1', height: '6px', background: 'rgba(233,233,237,.07)', borderRadius: '3px', overflow: 'hidden'}}>
                    <div style={{height: '100%', background: '#d98a8a', width: g.pct}}></div>
                  </div>
                  <div style={{fontSize: '13px', color: '#e9e9ed', width: '64px', textAlign: 'right', flex: 'none'}}>× {g.count}</div>
                </div>
                <div style={{marginTop: '8px', marginLeft: '192px', fontSize: '12.5px', color: '#8b8f9f', fontFamily: 'ui-monospace,Menlo,monospace', textWrap: 'pretty'}}>{g.sample}</div>
              </div>
            </Fragment>))}
          </div>
          </Fragment>) : null}

          {(isBySample) ? (<Fragment>
          <div>
            <div style={{display: 'grid', gridTemplateColumns: '70px 1fr 1fr 1.6fr', gap: '12px', padding: '10px 20px', borderBottom: '1px solid var(--color-divider)', fontSize: '11px', letterSpacing: '.07em', textTransform: 'uppercase', color: '#75798c'}}>
              <div>Row</div><div>Field</div><div>Rule</div><div>Offending value</div>
            </div>
            {(sampleRows).map((r, r_i) => (<Fragment key={r_i}>
              <div style={{display: 'grid', gridTemplateColumns: '70px 1fr 1fr 1.6fr', gap: '12px', padding: '11px 20px', borderBottom: '1px solid rgba(233,233,237,.07)', fontFamily: 'ui-monospace,Menlo,monospace', fontSize: '12.5px', alignItems: 'center'}}>
                <div style={{color: '#75798c'}}>{r.row}</div>
                <div style={{color: 'var(--color-accent-300)'}}>{r.field}</div>
                <div style={{color: '#cfd3e5'}}>{r.rule}</div>
                <div style={{color: '#d98a8a', whiteSpace: 'nowrap', overflow: 'hidden', textOverflow: 'ellipsis'}}>{r.value}</div>
              </div>
            </Fragment>))}
          </div>
          </Fragment>) : null}
        </div>

        <div style={{display: 'flex', gap: '12px', alignItems: 'center', background: 'var(--color-surface)', borderRadius: 'var(--radius-md)', padding: '13px 18px', boxShadow: 'var(--shadow-sm)'}}>
          <div style={{fontSize: '13px', color: '#b2b6ca', flex: '1', textWrap: 'pretty'}}>All 412 violations come from one deploy: <span style={{color: '#e9e9ed'}}>checkout-svc 4.18.0</span> started emitting <span style={{fontFamily: 'ui-monospace,Menlo,monospace', color: 'var(--color-accent-300)'}}>currency: "BTC"</span> and negative refunds.</div>
          <div onClick={goDlq} className="btn btn-secondary" style={{flex: 'none'}}>Open dead letters</div>
        </div>
      </div>
      </Fragment>) : null}

      {(isDiff) ? (<Fragment>
      <div style={{display: 'flex', flexDirection: 'column', gap: '18px', maxWidth: '1080px'}}>
        <div style={{background: 'var(--color-surface)', borderRadius: 'var(--radius-lg)', boxShadow: 'var(--shadow-sm)', padding: '18px 20px', display: 'flex', gap: '20px', alignItems: 'center'}}>
          <div style={{flex: '1', minWidth: '0'}}>
            <div style={{fontSize: '12px', color: '#75798c', fontFamily: 'ui-monospace,Menlo,monospace'}}>payments-eng/ledger · PR #482</div>
            <div style={{fontFamily: 'var(--font-heading)', fontSize: '18px', marginTop: '3px'}}>Drop legacy fields, tighten currency set</div>
            <div style={{fontSize: '12.5px', color: '#8b8f9f', marginTop: '4px'}}>contracts/payments.yaml · 3.1.0 → 3.2.0 · opened by r.okafor</div>
          </div>
          <div style={{textAlign: 'right', flex: 'none'}}>
            <div style={{display: 'inline-flex', alignItems: 'center', gap: '8px', border: '1px solid #8a4d4d', background: 'rgba(217,138,138,.10)', color: '#d98a8a', borderRadius: '999px', padding: '6px 13px', fontSize: '13px'}}>Merge blocked</div>
            <div style={{fontSize: '11.5px', color: '#75798c', marginTop: '7px'}}>covenant diff --fail-on breaking · exit 1</div>
          </div>
        </div>

        <div style={{background: 'rgba(217,138,138,.07)', border: '1px solid #6b3f3f', borderRadius: 'var(--radius-md)', padding: '14px 18px', display: 'flex', gap: '14px', alignItems: 'center'}}>
          <div style={{flex: '1'}}>
            <div style={{fontSize: '13.5px', color: '#e9e9ed'}}>A breaking change needs a major bump: 3.1.0 → <span style={{fontFamily: 'ui-monospace,Menlo,monospace', color: '#d98a8a'}}>4.0.0</span></div>
            <div style={{fontSize: '12.5px', color: '#b2b6ca', marginTop: '3px', textWrap: 'pretty'}}>The proposed bump is minor. Consumers pinned to ^3 would pick this up silently.</div>
          </div>
          <div className="btn btn-primary" style={{flex: 'none'}}>Apply 4.0.0</div>
        </div>

        <div style={{display: 'flex', gap: '10px'}}>
          {(diffLegend).map((l, l_i) => (<Fragment key={l_i}>
            <div style={{display: 'flex', alignItems: 'center', gap: '7px', fontSize: '12.5px', color: '#b2b6ca', border: '1px solid var(--color-divider)', borderRadius: '999px', padding: '4px 11px'}}>
              <div style={{width: '7px', height: '7px', borderRadius: '50%', background: l.color}}></div>{l.label}
            </div>
          </Fragment>))}
        </div>

        <div style={{background: 'var(--color-surface)', borderRadius: 'var(--radius-lg)', boxShadow: 'var(--shadow-sm)', overflow: 'hidden'}}>
          {(diffFindings).map((d, d_i) => (<Fragment key={d_i}>
            <div style={{display: 'flex', gap: '16px', padding: '15px 20px', borderBottom: '1px solid rgba(233,233,237,.07)', alignItems: 'flex-start'}}>
              <div style={{width: '74px', flex: 'none', fontSize: '11px', textTransform: 'uppercase', letterSpacing: '.06em', color: d.color, border: `1px solid ${d.color}`, borderRadius: '4px', padding: '2px 6px', textAlign: 'center'}}>{d.severity}</div>
              <div style={{flex: '1', minWidth: '0'}}>
                <div style={{fontFamily: 'ui-monospace,Menlo,monospace', fontSize: '13px', color: '#e9e9ed'}}>{d.path}</div>
                <div style={{fontSize: '13px', color: '#b2b6ca', marginTop: '3px', textWrap: 'pretty'}}>{d.message}</div>
                <div style={{fontSize: '12px', color: '#75798c', marginTop: '4px', textWrap: 'pretty'}}>{d.who}</div>
              </div>
              <div style={{flex: 'none', textAlign: 'right'}}>
                <div style={{fontSize: '11px', letterSpacing: '.06em', textTransform: 'uppercase', color: '#8b8f9f'}}>Impact</div>
                <div style={{fontSize: '12.5px', color: '#cfd3e5'}}>{d.impact}</div>
              </div>
            </div>
          </Fragment>))}
        </div>

        <div style={{background: 'var(--color-surface)', borderRadius: 'var(--radius-lg)', boxShadow: 'var(--shadow-sm)', padding: '16px 20px'}}>
          <div style={{display: 'flex', alignItems: 'center', gap: '8px', marginBottom: '10px'}}>
            <h5 style={{margin: '0'}}>Who actually breaks</h5>
          </div>
          <div style={{fontSize: '12.5px', color: '#8b8f9f', marginBottom: '10px'}}>From the registry's consumer graph — not guessed from the YAML.</div>
          {(consumers).map((c, c_i) => (<Fragment key={c_i}>
            <div style={{display: 'flex', alignItems: 'center', gap: '12px', padding: '9px 0', borderBottom: '1px solid rgba(233,233,237,.07)'}}>
              <div style={{fontSize: '13px', flex: '1'}}>{c.name}</div>
              <div style={{fontFamily: 'ui-monospace,Menlo,monospace', fontSize: '12px', color: '#8b8f9f'}}>{c.uses}</div>
              <div style={{fontSize: '12px', color: c.color, width: '110px', textAlign: 'right'}}>{c.impact}</div>
            </div>
          </Fragment>))}
        </div>
      </div>
      </Fragment>) : null}

      {(isGate) ? (<Fragment>
      <div style={{display: 'flex', flexDirection: 'column', gap: '18px'}}>
        <div style={{display: 'grid', gridTemplateColumns: 'repeat(4,1fr)', gap: '14px'}}>
          {(gateStats).map((s, s_i) => (<Fragment key={s_i}>
            <div style={{background: 'var(--color-surface)', borderRadius: 'var(--radius-lg)', boxShadow: 'var(--shadow-sm)', padding: '15px 18px'}}>
              <div style={{fontSize: '11px', letterSpacing: '.07em', textTransform: 'uppercase', color: '#75798c'}}>{s.label}</div>
              <div style={{fontFamily: 'var(--font-heading)', fontSize: '26px', marginTop: '5px', color: s.color}}>{s.value}</div>
              <div style={{fontSize: '11.5px', color: '#8b8f9f'}}>{s.note}</div>
            </div>
          </Fragment>))}
        </div>

        <div style={{display: 'grid', gridTemplateColumns: '1.4fr 1fr', gap: '18px', alignItems: 'start'}}>
          <div style={{background: 'var(--color-surface)', borderRadius: 'var(--radius-lg)', boxShadow: 'var(--shadow-sm)', padding: '18px 20px'}}>
            <div style={{display: 'flex', alignItems: 'center', gap: '10px', marginBottom: '14px'}}>
              <h5 style={{margin: '0'}}>Throughput · last 60 minutes</h5>
              <div style={{display: 'flex', alignItems: 'center', gap: '6px', marginLeft: 'auto', fontSize: '11.5px', color: '#84d9a8'}}>
                <div style={{width: '6px', height: '6px', borderRadius: '50%', background: '#84d9a8', animation: 'pulse 2s infinite'}}></div>live
              </div>
            </div>
            <div style={{display: 'flex', alignItems: 'flex-end', gap: '3px', height: '150px'}}>
              {(throughput).map((b, b_i) => (<Fragment key={b_i}>
                <div style={{flex: '1', display: 'flex', flexDirection: 'column', justifyContent: 'flex-end', gap: '1px', height: '100%'}}>
                  <div style={{background: '#d98a8a', height: b.bad}}></div>
                  <div style={{background: 'var(--color-accent-600)', height: b.good}}></div>
                </div>
              </Fragment>))}
            </div>
            <div style={{display: 'flex', justifyContent: 'space-between', fontSize: '11px', color: '#75798c', marginTop: '8px'}}>
              <span>18:00</span><span>18:30</span><span>19:00</span>
            </div>
          </div>

          <div style={{background: 'var(--color-surface)', borderRadius: 'var(--radius-lg)', boxShadow: 'var(--shadow-sm)', padding: '18px 20px'}}>
            <h5 style={{margin: '0 0 12px'}}>Blocking rules right now</h5>
            {(liveRules).map((r, r_i) => (<Fragment key={r_i}>
              <div style={{padding: '9px 0', borderBottom: '1px solid rgba(233,233,237,.07)'}}>
                <div style={{display: 'flex', gap: '10px', alignItems: 'baseline'}}>
                  <div style={{fontFamily: 'ui-monospace,Menlo,monospace', fontSize: '12.5px', color: 'var(--color-accent-300)', flex: '1'}}>{r.field}</div>
                  <div style={{fontFamily: 'ui-monospace,Menlo,monospace', fontSize: '12px', color: '#8b8f9f'}}>{r.rule}</div>
                  <div style={{fontSize: '12.5px'}}>{r.rate}</div>
                </div>
                <div style={{height: '4px', background: 'rgba(233,233,237,.07)', borderRadius: '2px', marginTop: '6px', overflow: 'hidden'}}>
                  <div style={{height: '100%', background: '#d98a8a', width: r.pct}}></div>
                </div>
              </div>
            </Fragment>))}
          </div>
        </div>

        <div style={{background: 'var(--color-surface)', borderRadius: 'var(--radius-lg)', boxShadow: 'var(--shadow-sm)', overflow: 'hidden'}}>
          <div style={{padding: '13px 20px', borderBottom: '1px solid var(--color-divider)', display: 'flex', alignItems: 'center', gap: '10px'}}>
            <h5 style={{margin: '0'}}>Records blocked in the last minute</h5>
            <div style={{marginLeft: 'auto', fontFamily: 'ui-monospace,Menlo,monospace', fontSize: '11.5px', color: '#75798c'}}>kcat -C -t payments_raw | covenant gate -c payments.yaml --dlq /var/log/payments.dlq.ndjson</div>
          </div>
          {(blockedStream).map((b, b_i) => (<Fragment key={b_i}>
            <div style={{display: 'grid', gridTemplateColumns: '88px 1.4fr 2fr', gap: '14px', padding: '10px 20px', borderBottom: '1px solid rgba(233,233,237,.07)', fontFamily: 'ui-monospace,Menlo,monospace', fontSize: '12px', alignItems: 'center'}}>
              <div style={{color: '#75798c'}}>{b.time}</div>
              <div style={{color: '#cfd3e5', whiteSpace: 'nowrap', overflow: 'hidden', textOverflow: 'ellipsis'}}>{b.key}</div>
              <div style={{color: '#d98a8a', whiteSpace: 'nowrap', overflow: 'hidden', textOverflow: 'ellipsis'}}>{b.why}</div>
            </div>
          </Fragment>))}
        </div>
      </div>
      </Fragment>) : null}

      {(isDlq) ? (<Fragment>
      <div style={{display: 'grid', gridTemplateColumns: '380px 1fr', gap: '18px', alignItems: 'start'}}>
        <div style={{background: 'var(--color-surface)', borderRadius: 'var(--radius-lg)', boxShadow: 'var(--shadow-sm)', overflow: 'hidden'}}>
          <div style={{padding: '12px 16px', borderBottom: '1px solid var(--color-divider)', display: 'flex', alignItems: 'center', gap: '8px'}}>
            <div style={{fontSize: '12.5px', color: '#b2b6ca'}}>1,284 envelopes</div>
            <div style={{marginLeft: 'auto', fontSize: '12px', color: '#75798c'}}>newest first</div>
          </div>
          {(dlqList).map((d, d_i) => (<Fragment key={d_i}>
            <div onClick={d.onClick} style={{padding: '12px 16px', borderBottom: '1px solid rgba(233,233,237,.07)', cursor: 'pointer', background: d.bg, borderLeft: `2px solid ${d.mark}`}} data-hv="a">
              <div style={{display: 'flex', gap: '8px', alignItems: 'baseline'}}>
                <div style={{fontFamily: 'ui-monospace,Menlo,monospace', fontSize: '12.5px', color: '#e9e9ed', flex: '1', whiteSpace: 'nowrap', overflow: 'hidden', textOverflow: 'ellipsis'}}>{d.id}</div>
                <div style={{fontSize: '11px', color: '#75798c'}}>{d.when}</div>
              </div>
              <div style={{fontSize: '12px', color: '#d98a8a', marginTop: '3px'}}>{d.rules}</div>
            </div>
          </Fragment>))}
        </div>

        <div style={{display: 'flex', flexDirection: 'column', gap: '16px'}}>
          <div style={{background: 'var(--color-surface)', borderRadius: 'var(--radius-lg)', boxShadow: 'var(--shadow-sm)', overflow: 'hidden'}}>
            <div style={{padding: '14px 20px', borderBottom: '1px solid var(--color-divider)', display: 'flex', alignItems: 'center', gap: '12px'}}>
              <div style={{minWidth: '0'}}>
                <div style={{fontFamily: 'ui-monospace,Menlo,monospace', fontSize: '13px'}}>{dlqDetail.id}</div>
                <div style={{fontSize: '12px', color: '#8b8f9f'}}>payments v3.1.0 · model payments · row {dlqDetail.row} · partition {dlqDetail.partition}</div>
              </div>
              <div style={{marginLeft: 'auto', display: 'flex', gap: '8px', flex: 'none'}}>
                <div className="btn btn-secondary" style={{fontSize: '12.5px'}}>Copy envelope</div>
                <div onClick={replay} className="btn btn-primary" style={{fontSize: '12.5px'}}>{replayLabel}</div>
              </div>
            </div>
            <div style={{display: 'grid', gridTemplateColumns: '1fr 1fr'}}>
              <div style={{padding: '16px 20px', borderRight: '1px solid var(--color-divider)'}}>
                <div style={{fontSize: '11px', letterSpacing: '.07em', textTransform: 'uppercase', color: '#75798c', marginBottom: '8px'}}>Rejected record</div>
                <div style={{fontFamily: 'ui-monospace,Menlo,monospace', fontSize: '12px', lineHeight: '1.7', background: '#12131f', borderRadius: '8px', padding: '12px 14px'}}>
                  {(dlqDetail.json).map((l, l_i) => (<Fragment key={l_i}>
                    <div style={{whiteSpace: 'pre', color: l.c}}>{l.t}</div>
                  </Fragment>))}
                </div>
              </div>
              <div style={{padding: '16px 20px'}}>
                <div style={{fontSize: '11px', letterSpacing: '.07em', textTransform: 'uppercase', color: '#75798c', marginBottom: '8px'}}>Why it was rejected</div>
                {(dlqDetail.violations).map((v, v_i) => (<Fragment key={v_i}>
                  <div style={{padding: '9px 0', borderBottom: '1px solid rgba(233,233,237,.07)'}}>
                    <div style={{display: 'flex', gap: '8px', alignItems: 'baseline'}}>
                      <div style={{fontFamily: 'ui-monospace,Menlo,monospace', fontSize: '12.5px', color: 'var(--color-accent-300)'}}>{v.field}</div>
                      <div style={{fontFamily: 'ui-monospace,Menlo,monospace', fontSize: '11.5px', color: '#8b8f9f'}}>{v.rule}</div>
                    </div>
                    <div style={{fontSize: '12.5px', color: '#cfd3e5', marginTop: '2px', textWrap: 'pretty'}}>{v.message}</div>
                  </div>
                </Fragment>))}
                <div style={{fontSize: '12px', color: '#75798c', marginTop: '12px', textWrap: 'pretty'}}>Fix the producer, then replay — the envelope keeps the record verbatim, so nothing is lost.</div>
              </div>
            </div>
          </div>
          <div style={{display: 'flex', gap: '12px', alignItems: 'center', background: 'rgba(145,132,217,.07)', border: '1px solid var(--color-accent-800)', borderRadius: 'var(--radius-md)', padding: '13px 16px'}}>
            <div style={{fontSize: '10px', letterSpacing: '.07em', textTransform: 'uppercase', color: 'var(--color-text-dim)', border: '1px solid var(--color-divider)', borderRadius: '4px', padding: '1px 5px', flex: 'none'}}>No endpoint</div>
            <div style={{fontSize: '13px', color: '#cfd3e5', flex: '1', textWrap: 'pretty'}}>Bulk replay: select by rule, dry-run against the current contract, then re-emit to the topic in original order.</div>
            <div className="btn btn-primary" style={{flex: 'none'}}>Replay 1,284</div>
          </div>
        </div>
      </div>
      </Fragment>) : null}

      {(isTeams) ? (<Fragment>
      <div style={{display: 'flex', flexDirection: 'column', gap: '18px'}}>
        <div style={{display: 'flex', alignItems: 'center', gap: '12px'}}>
          <div style={{display: 'flex', border: '1px solid var(--color-divider)', borderRadius: '8px', overflow: 'hidden'}}>
            {(healthTabs).map((t, t_i) => (<Fragment key={t_i}>
              <div onClick={t.onClick} style={{cursor: 'pointer', padding: '5px 14px', fontSize: '12.5px', color: t.color, background: t.bg}}>{t.label}</div>
            </Fragment>))}
          </div>
          <div style={{fontSize: '12.5px', color: '#8b8f9f'}}>Enforcement outcomes by owning team, last 14 days</div>
        </div>

        {(isHeatmap) ? (<Fragment>
        <div style={{background: 'var(--color-surface)', borderRadius: 'var(--radius-lg)', boxShadow: 'var(--shadow-sm)', padding: '18px 20px', overflowX: 'auto'}}>
          {(heatRows).map((row, row_i) => (<Fragment key={row_i}>
            <div style={{display: 'flex', alignItems: 'center', gap: '12px', padding: '7px 0'}}>
              <div style={{width: '150px', flex: 'none', fontSize: '12.5px', color: '#cfd3e5', whiteSpace: 'nowrap'}}>{row.team}</div>
              <div style={{display: 'flex', gap: '3px', flex: '1'}}>
                {(row.cells).map((c, c_i) => (<Fragment key={c_i}>
                  <div title={c.title} style={{flex: '1', height: '26px', borderRadius: '3px', background: c.color}}></div>
                </Fragment>))}
              </div>
              <div style={{width: '120px', flex: 'none', textAlign: 'right', fontSize: '12.5px', color: row.color}}>{row.summary}</div>
            </div>
          </Fragment>))}
          <div style={{display: 'flex', alignItems: 'center', gap: '8px', marginTop: '14px', fontSize: '11.5px', color: '#75798c'}}>
            <span>clean</span>
            <div style={{width: '22px', height: '10px', borderRadius: '2px', background: 'rgba(233,233,237,.07)'}}></div>
            <div style={{width: '22px', height: '10px', borderRadius: '2px', background: 'rgba(145,132,217,.45)'}}></div>
            <div style={{width: '22px', height: '10px', borderRadius: '2px', background: 'rgba(217,138,138,.5)'}}></div>
            <div style={{width: '22px', height: '10px', borderRadius: '2px', background: '#d98a8a'}}></div>
            <span>blocked deploys</span>
          </div>
        </div>
        </Fragment>) : null}

        {(isCards) ? (<Fragment>
        <div style={{display: 'grid', gridTemplateColumns: 'repeat(3,1fr)', gap: '16px'}}>
          {(teamCards).map((t, t_i) => (<Fragment key={t_i}>
            <div style={{background: 'var(--color-surface)', borderRadius: 'var(--radius-lg)', boxShadow: 'var(--shadow-sm)', padding: '18px 20px', display: 'flex', flexDirection: 'column', gap: '10px'}}>
              <div style={{display: 'flex', alignItems: 'center', gap: '9px'}}>
                <div style={{width: '8px', height: '8px', borderRadius: '50%', background: t.dot}}></div>
                <div style={{fontFamily: 'var(--font-heading)', fontSize: '15px'}}>{t.team}</div>
                <div style={{marginLeft: 'auto', fontSize: '12px', color: '#75798c'}}>{t.contracts}</div>
              </div>
              <div style={{display: 'flex', gap: '16px'}}>
                <div>
                  <div style={{fontFamily: 'var(--font-heading)', fontSize: '22px', color: t.color}}>{t.pass}</div>
                  <div style={{fontSize: '11px', color: '#75798c'}}>pass rate</div>
                </div>
                <div>
                  <div style={{fontFamily: 'var(--font-heading)', fontSize: '22px'}}>{t.blocked}</div>
                  <div style={{fontSize: '11px', color: '#75798c'}}>blocked merges</div>
                </div>
                <div>
                  <div style={{fontFamily: 'var(--font-heading)', fontSize: '22px'}}>{t.mttr}</div>
                  <div style={{fontSize: '11px', color: '#75798c'}}>median fix</div>
                </div>
              </div>
              <div style={{fontSize: '12.5px', color: '#8b8f9f', textWrap: 'pretty'}}>{t.note}</div>
            </div>
          </Fragment>))}
        </div>
        </Fragment>) : null}
      </div>
      </Fragment>) : null}

      {(isAlerts) ? (<Fragment>
      <div style={{display: 'grid', gridTemplateColumns: '1fr 320px', gap: '20px', alignItems: 'start', maxWidth: '1120px'}}>
        <div style={{display: 'flex', flexDirection: 'column', gap: '18px'}}>
          <div style={{background: 'var(--color-surface)', borderRadius: 'var(--radius-lg)', boxShadow: 'var(--shadow-sm)', padding: '18px 20px'}}>
            <h5 style={{margin: '0 0 4px'}}>Default policy</h5>
            <div style={{fontSize: '12.5px', color: '#8b8f9f', marginBottom: '14px'}}>Applies to every contract that doesn't override it.</div>
            {(settingRows).map((s, s_i) => (<Fragment key={s_i}>
              <div style={{display: 'flex', alignItems: 'center', gap: '16px', padding: '12px 0', borderBottom: '1px solid rgba(233,233,237,.07)'}}>
                <div style={{flex: '1', minWidth: '0'}}>
                  <div style={{fontSize: '13.5px'}}>{s.label}</div>
                  <div style={{fontSize: '12px', color: '#8b8f9f', textWrap: 'pretty'}}>{s.note}</div>
                </div>
                {(s.isToggle) ? (<Fragment>
                  <div onClick={s.onClick} style={{flex: 'none', width: '38px', height: '21px', borderRadius: '999px', padding: '2px', cursor: 'pointer', background: s.trackBg, display: 'flex', justifyContent: s.justify}}>
                    <div style={{width: '17px', height: '17px', borderRadius: '50%', background: s.knob}}></div>
                  </div>
                </Fragment>) : null}
                {(s.isChoice) ? (<Fragment>
                  <div style={{flex: 'none', display: 'flex', border: '1px solid var(--color-divider)', borderRadius: '8px', overflow: 'hidden'}}>
                    {(s.options).map((o, o_i) => (<Fragment key={o_i}>
                      <div onClick={o.onClick} style={{cursor: 'pointer', padding: '5px 12px', fontSize: '12.5px', color: o.color, background: o.bg}}>{o.label}</div>
                    </Fragment>))}
                  </div>
                </Fragment>) : null}
              </div>
            </Fragment>))}
          </div>

          <div style={{background: 'var(--color-surface)', borderRadius: 'var(--radius-lg)', boxShadow: 'var(--shadow-sm)', padding: '18px 20px'}}>
            <h5 style={{margin: '0 0 4px'}}>Where alerts go</h5>
            <div style={{fontSize: '12.5px', color: '#8b8f9f', marginBottom: '12px'}}>Routed by the contract's owner field — no separate on-call mapping to maintain.</div>
            {(routes).map((r, r_i) => (<Fragment key={r_i}>
              <div style={{display: 'flex', alignItems: 'center', gap: '12px', padding: '11px 0', borderBottom: '1px solid rgba(233,233,237,.07)'}}>
                <div style={{fontFamily: 'ui-monospace,Menlo,monospace', fontSize: '12.5px', color: 'var(--color-accent-300)', width: '180px', flex: 'none'}}>{r.owner}</div>
                <div style={{fontSize: '13px', color: '#cfd3e5', flex: '1'}}>{r.target}</div>
                <div style={{fontSize: '12px', color: '#75798c'}}>{r.when}</div>
              </div>
            </Fragment>))}
          </div>
        </div>

      </div>
      </Fragment>) : null}

      {(isInstall) ? (<Fragment>
      <div style={{maxWidth: '820px', display: 'flex', flexDirection: 'column', gap: '18px'}}>
        <div>
          <h3 style={{margin: '0 0 6px'}}>Enforce your first contract in about ten minutes</h3>
          <p style={{color: '#b2b6ca', maxWidth: '62ch', textWrap: 'pretty'}}>Covenant is one static binary. It has no server, no database and no agent — the console you're reading is a view over the reports the binary already writes.</p>
        </div>
        {(installSteps).map((s, s_i) => (<Fragment key={s_i}>
          <div style={{background: 'var(--color-surface)', borderRadius: 'var(--radius-lg)', boxShadow: 'var(--shadow-sm)', padding: '18px 20px', display: 'flex', gap: '16px'}}>
            <div style={{width: '26px', height: '26px', flex: 'none', borderRadius: '50%', border: '1px solid var(--color-accent-700)', color: 'var(--color-accent-300)', display: 'flex', alignItems: 'center', justifyContent: 'center', fontSize: '12.5px'}}>{s.n}</div>
            <div style={{flex: '1', minWidth: '0'}}>
              <div style={{fontFamily: 'var(--font-heading)', fontSize: '15px'}}>{s.title}</div>
              <div style={{fontSize: '12.5px', color: '#8b8f9f', margin: '4px 0 10px', textWrap: 'pretty'}}>{s.body}</div>
              <div style={{fontFamily: 'ui-monospace,Menlo,monospace', fontSize: '12.5px', color: 'var(--color-accent-300)', background: '#12131f', borderRadius: '8px', padding: '12px 14px', overflowX: 'auto', whiteSpace: 'pre'}}>{s.cmd}</div>
            </div>
          </div>
        </Fragment>))}
        <div style={{display: 'flex', gap: '12px', alignItems: 'center', background: 'var(--color-surface)', borderRadius: 'var(--radius-md)', padding: '14px 18px', boxShadow: 'var(--shadow-sm)'}}>
          <div style={{fontSize: '13px', color: '#b2b6ca', flex: '1', textWrap: 'pretty'}}>Exit codes are the whole integration surface: <span style={{fontFamily: 'ui-monospace,Menlo,monospace', color: '#84d9a8'}}>0</span> clean · <span style={{fontFamily: 'ui-monospace,Menlo,monospace', color: '#d98a8a'}}>1</span> violated · <span style={{fontFamily: 'ui-monospace,Menlo,monospace', color: 'var(--color-accent-300)'}}>2</span> run failed.</div>
          <div onClick={goRegistry} className="btn btn-primary" style={{flex: 'none'}}>Open the registry</div>
        </div>
      </div>
      </Fragment>) : null}

    </div>
  </main>
</div>


    </>
  );
}
