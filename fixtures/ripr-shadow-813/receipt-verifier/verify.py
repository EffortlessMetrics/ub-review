# Owner: ub-review/tests, #813. Read-only comparison receipt verifier.
from pathlib import Path
import json,collections,hashlib,os
G={'weakly_exposed','reachable_unrevealed','no_static_path'}
U={'infection_unknown','propagation_unknown','static_unknown'}
def load(p):
 def pairs(items):
  d={}
  for k,v in items:
   if k in d:raise ValueError('duplicate JSON key: '+k)
   d[k]=v
  return d
 return json.loads(p.read_bytes(),object_pairs_hook=pairs)
def walk(o):
 if isinstance(o,dict):
  yield o
  for v in o.values():yield from walk(v)
 elif isinstance(o,list):
  for v in o:yield from walk(v)
def verify_comparison_contract(contract):
 if contract.get('schema') != 'ub-review.ripr_frozen_receipt.v1':
  raise ValueError('unsupported comparison receipt schema')
 if contract.get('authority') != 'shadow-only':
  raise ValueError('comparison cannot claim required authority')
 if contract.get('decision') != 'comparison_only_no_promotion':
  raise ValueError('unsupported promotion decision')
 for key in ('qualified', 'promotion_approved', 'required_gate_passed'):
  if key in contract and contract[key] is not False:
   raise ValueError('unsupported qualification claim: '+key)

def verify_unknowns(raw, badge, side):
 counts=collections.Counter(x['classification'] for x in raw['findings'])
 for kind in sorted(U):
  value=raw['summary'].get(kind)
  if type(value) is not int or value != counts[kind]:
   raise ValueError('unknown classification count must remain explicit: '+kind)
 current=sum(x['classification'] in U and
             (side=='stable' or x.get('source_currentness')!='base_deleted')
             for x in raw['findings'])
 value=badge['counts'].get('unknowns')
 if type(value) is not int or value != current:
  raise ValueError('native unknown count disagrees with retained rows')
 return {kind: counts[kind] for kind in sorted(U)}

def main(root):
 E=Path(root)
 contract=E/'manifest.json'
 if not contract.is_file():contract=E/'sealed/manifest.json'
 verify_comparison_contract(load(contract))
 summary=[]
 for corpus in ['packets','fixture-packets']:
  for packet in sorted((E/corpus).iterdir()):
   for side in ['stable','candidate']:
    out=packet/side
    if not (out/'badge.stdout').exists():continue
    raw=load(out/'raw-1.stdout');badge=load(out/'badge.stdout');findings=raw['findings'];counts=collections.Counter(x['classification'] for x in findings)
    unknown_breakdown=verify_unknowns(raw,badge,side)
    assert len({x['id'] for x in findings})==len(findings),'duplicate finding id'
    assert raw['summary']['findings']==len(findings),'incomplete findings array'
    assert raw['summary']['probes']==len(findings),'probe denominator mismatch'
    for k,v in counts.items():assert raw['summary'][k]==v,(k,v,raw['summary'])
    for obj in [*walk(raw),*walk(badge)]:
     if 'downstream_consumable' in obj:assert obj['downstream_consumable'] is True,'non-consumable raw'
     if 'analysis_complete' in obj:assert obj['analysis_complete'] is True,'incomplete analysis'
     if 'run_limitations' in obj:assert not obj['run_limitations'],'run limitations'
    if side=='candidate':
     outcome=raw['analysis_outcome']['outcome']
     assert outcome==badge['analysis_outcome'],'raw/badge outcome disagreement'
     assert not outcome['limitations'],'typed outcome limitations'
     assert outcome['kind'] in ['complete_with_findings','no_behavioral_candidates'],'incomplete typed outcome'
     assert outcome['counts']['finding_count']==len(findings),'typed finding denominator mismatch'
     assert outcome['counts']['probe_count']==raw['summary']['probes'],'typed probe denominator mismatch'
     frozen=load(packet/'frozen-input.json')
     expected=frozen['diff_sha256'] if corpus=='packets' else frozen['diff.patch']
     assert outcome['identity']['input_identity']=='sha256:'+expected,'input identity mismatch'
    bc=badge['counts'];assert bc['analyzed_findings']==len(findings),'badge/full raw denominator mismatch'
    rawg=sum(counts[k] for k in G)
    if side=='candidate':
     removed=sum(x['classification'] in G and x.get('source_currentness')=='base_deleted' for x in findings)
     assert rawg==bc['unsuppressed_exposure_gaps']+bc['suppressed_exposure_gaps']+removed,'raw/current/base/suppression disagreement'
    else:
     assert rawg==bc['unsuppressed_exposure_gaps']+bc['suppressed_exposure_gaps'],'legacy raw/suppression disagreement'
    sides=collections.Counter(x.get('source_currentness','legacy_unspecified') if isinstance(x.get('source_currentness',''),str) else str(x.get('source_currentness')) for x in findings if x['classification'] in G)
    row={'corpus':corpus,'name':packet.name,'side':side,'raw_schema':raw.get('schema_version'),'badge_schema':badge.get('schema_version'),'findings':len(findings),'classes':dict(counts),'raw_gap_rows':rawg,'native_unsuppressed':bc['unsuppressed_exposure_gaps'],'native_suppressed':bc['suppressed_exposure_gaps'],'unknowns':bc['unknowns'],'unknown_breakdown':unknown_breakdown,'comparison_decision':'comparison_only_no_promotion','source_currentness_gap_counts':dict(sides),'raw_deterministic':(out/'raw-1.stdout').read_bytes()==(out/'raw-2.stdout').read_bytes(),'native_count_below_unchanged_max0':bc['unsuppressed_exposure_gaps']==0,'preview_skipped':badge.get('preview_skipped'),'limitations':raw.get('run_limitations'),'outcome':raw.get('analysis_outcome'),'durations':load(out/'invocations.json')}
    assert row['raw_deterministic'],'nondeterministic repeat'
    summary.append(row)
 for r in summary:print(r['name'],r['side'],'findings',r['findings'],'rawg',r['raw_gap_rows'],'badge',r['native_unsuppressed'],'unknown',r['unknowns'],'schema',r['raw_schema'],r['badge_schema'],'unknown_breakdown',r['unknown_breakdown'],'decision',r['comparison_decision'])

if __name__ == "__main__":
 import argparse
 parser=argparse.ArgumentParser(description="Validate comparison-only RIPR receipts without altering evidence")
 parser.add_argument("evidence_root", type=Path)
 args=parser.parse_args()
 main(args.evidence_root)
