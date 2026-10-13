#!/usr/bin/env python3
"""Audit frozen scaling trials; keep strict presentation scores and content checks separate."""
import argparse
import importlib.util
import json
from pathlib import Path
import re

HERE=Path(__file__).resolve().parent
SPEC=importlib.util.spec_from_file_location('projects',HERE/'evaluate-projects.py')
EVAL=importlib.util.module_from_spec(SPEC);SPEC.loader.exec_module(EVAL)


def content_claims(summary, commands, changed_files):
    # A raw JSON document is a standard alternative encoding, not a guessed substring.
    try:
        raw=json.loads(summary)
    except ValueError:
        return 'fenced_or_invalid', EVAL.evidence_claims(summary,commands,changed_files)
    canonical='```json\n'+json.dumps(raw)+'\n```'
    return 'raw_json', EVAL.evidence_claims(canonical,commands,changed_files)


def audit(root):
    report=json.loads((root/'report.json').read_text())
    cases={c['id']:c for c in json.loads((root/'source-snapshot/cases.json').read_text())}
    records=[];answers=[]
    for r in report['records']:
        trial=root/f"{r['case']}-{r['attempt']}-{r['variant']}"
        databases=list(trial.glob('state/**/run.sqlite3'))
        if len(databases)!=1:raise ValueError('one journal per trial required')
        _,events,summaries,_,outputs=EVAL.inspect(databases[0]);metrics=EVAL.safety_metrics(events)
        if any(r[k]!=v for k,v in metrics.items()):raise ValueError('journal safety metrics differ')
        commands=EVAL.observed_commands(databases[0],outputs)
        if commands!=r.get('observed_commands'):raise ValueError('command recipe join differs')
        originals=cases[r['case']]['files'];workspace=trial/'workspace'
        actual={str(p.relative_to(workspace)):p.read_bytes() for p in workspace.rglob('*') if p.is_file() and '__pycache__' not in p.parts}
        changed=sorted(n for n in set(actual)|set(originals) if actual.get(n)!=(originals[n].encode() if n in originals else None))
        if changed!=r['observed_changed_files']:raise ValueError('changed files differ')
        summary=summaries[-1] if summaries else ''
        encoding,claims=content_claims(summary,commands,changed)
        source_quotes={name:bool(summaries and EVAL.quote_matches(summary,(workspace/name).read_text())) for name in cases[r['case']]['sources']}
        if source_quotes!=r['source_quotes']:raise ValueError('quote scores differ')
        sources_omitted=r['source_blocks_omitted']
        item={k:r[k] for k in ('case','family','size','response_mode','variant','attempt','outcome','task_completed','diagnostic_codes')}
        item.update(metrics,encoding=encoding,**claims,source_quotes=source_quotes,source_blocks_omitted=sources_omitted,
                    strict_evidence_contract_valid=r['evidence_contract_valid'])
        item['content_response_pass']=bool(r['outcome']=='pass' and claims['command_claims_match'] and claims['changed_file_claims_match']
                                          and (all(source_quotes.values()) if r['response_mode']=='quote' else sources_omitted))
        if not summaries:item['classification']='no_completion_output'
        elif r['response_mode']=='quote' and not all(source_quotes.values()):
            item['classification']='requested_source_omitted' if sources_omitted else 'source_quote_mismatch'
        elif encoding=='raw_json':item['classification']='content_present_but_fence_contract_violated'
        else:item['classification']='completion_present'
        records.append(item)
        answers.append({'case':r['case'],'variant':r['variant'],'attempt':r['attempt'],'summary':summary or None,
                        'observed_commands':commands,'observed_changed_files':changed,
                        'final_source_sha256':{n:EVAL.digest(workspace/n) for n in cases[r['case']]['sources']}})
    return {'scope':'Post-execution content audit accepting raw JSON; not the registered presentation/adoption gate. No rescoring or default change.',
            'records':records},answers


if __name__=='__main__':
    p=argparse.ArgumentParser(description=__doc__);p.add_argument('root',type=Path);args=p.parse_args()
    review,answers=audit(args.root)
    (args.root/'content-audit.json').write_text(json.dumps(review,ensure_ascii=False,indent=2)+'\n')
    (args.root/'answers.json').write_text(json.dumps(answers,ensure_ascii=False,indent=2)+'\n')
    print('Audited '+str(len(review['records']))+' frozen trials; raw report unchanged')
