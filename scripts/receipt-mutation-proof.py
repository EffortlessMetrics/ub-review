"""Disposable, exact-source mutation evidence for ub-review#1333; no branch writes."""
import difflib
import hashlib
import json
import os
from pathlib import Path
import re
import selectors
import signal
import subprocess
import time

HEAD = '9fa5ac2353194209b26d89672fb7c66bf4810a93'
TREE = '642f05dd2b868076183211678d25440b9170e6c2'
P = 'src/tools/gate_receipt.rs'
T = 'src/tools.rs'
# Every replacement is performed once on the original bytes, never cumulatively.
MUTATIONS = [
 ('present-absent', P, 'T::deserialize(deserializer).map(Some)', 'T::deserialize(deserializer).map(|_| None)'),
 ('counts-discard', P, 'BadgeCounts::deserialize(MapAccessDeserializer::new(map))', 'BadgeCounts::deserialize(MapAccessDeserializer::new(map)).map(|_| BadgeCounts { unsuppressed_exposure_gaps: 0 })'),
 ('counts-absent', P, 'deserializer.deserialize_map(CountsVisitor).map(Some)', 'deserializer.deserialize_map(CountsVisitor).map(|_| None)'),
 ('native-not-admitted', P, 'receipt.new_unsuppressed,', 'None,'),
 ('native-count-changed', P, 'Ok(CountEvidence::Complete(count)),', 'Ok(CountEvidence::Complete(count.saturating_add(1))),'),
 ('badge-not-admitted', P, '(None, Some(version), Some(counts), preview_skipped) => {', '(None, Some(version), Some(counts), preview_skipped) if false => {'),
 ('unknown-version-admitted', P, 'if !matches!(version.as_str(), "0.5" | "0.6") {', 'if false {'),
 ('missing-preview-admitted', P, 'if version == "0.6" && preview_skipped.is_none() {', 'if false {'),
 ('partial-marked-complete', P, 'Ok(CountEvidence::Incomplete(counts.unsuppressed_exposure_gaps))', 'Ok(CountEvidence::Complete(counts.unsuppressed_exposure_gaps))'),
 ('complete-marked-partial', P, 'Ok(CountEvidence::Complete(counts.unsuppressed_exposure_gaps))', 'Ok(CountEvidence::Incomplete(counts.unsuppressed_exposure_gaps))'),
 ('unknown-diagnostic-empty', P, '"unsupported RIPR badge schema_version {version:?}; expected 0.5 or 0.6"', '""'),
 ('hybrid-diagnostic-empty', P, '"gate-decision receipt must contain either native new_unsuppressed or a versioned \\\n             RIPR badge with schema_version and counts, never mixed authority fields"', '""'),
 ('nested-object-diagnostic-empty', P, 'formatter.write_str("a counts object")', 'formatter.write_str("")'),
 ('missing-preview-diagnostic-empty', P, '"RIPR badge schema_version 0.6 requires preview_skipped"', '""'),
 ('reader-complete-marked-partial', T, 'ToolGateDecisionState::Present(ToolGateDecision { new_unsuppressed })', 'ToolGateDecisionState::Incomplete(ToolGateDecision { new_unsuppressed })'),
 ('reader-partial-marked-complete', T, 'ToolGateDecisionState::Incomplete(ToolGateDecision { new_unsuppressed })', 'ToolGateDecisionState::Present(ToolGateDecision { new_unsuppressed })'),
 ('partial-violation-erased', T, 'if decision.new_unsuppressed > maximum {', 'if false {'),
 ('partial-reason-empty', T, '"RIPR badge preview_skipped reports incomplete coverage".to_owned()', '"".to_owned()'),
 ('partial-metric-promoted', T, '"RIPR badge preview_skipped reports incomplete coverage".to_owned(),\n                            None,', '"RIPR badge preview_skipped reports incomplete coverage".to_owned(),\n                            Some(decision.new_unsuppressed),'),
 ('present-state-discarded', T, 'ToolGateDecisionState::Present(decision) | ToolGateDecisionState::Incomplete(decision) => {\n            Some(decision)', 'ToolGateDecisionState::Present(decision) | ToolGateDecisionState::Incomplete(decision) => {\n            None'),
]
SUMMARY = re.compile(r'^test result: (ok|FAILED)\. (\d+) passed; (\d+) failed; (\d+) ignored; (\d+) measured; (\d+) filtered out;', re.M)

def classify(code, text, bounded=True):
    summary = SUMMARY.findall(text)
    failed = re.findall(r'^test (\S+) \.\.\. FAILED$', text, re.M)
    if not bounded or len(summary) != 1 or 'could not compile' in text:
        return 'not_proven', failed
    state, passed, failures, ignored, measured, _ = summary[0]
    if int(passed) + int(failures) != 16 or int(ignored) or int(measured):
        return 'not_proven', failed
    if code == 0 and state == 'ok' and int(failures) == 0:
        return 'survived', failed
    if code == 101 and state == 'FAILED' and int(failures) == len(failed) and failed:
        return 'killed', failed
    return 'not_proven', failed

def self_test():
    good = 'test result: ok. 16 passed; 0 failed; 0 ignored; 0 measured; 1114 filtered out;\n'
    bad = 'test tools::x ... FAILED\ntest result: FAILED. 15 passed; 1 failed; 0 ignored; 0 measured; 1114 filtered out;\n'
    assert classify(0, good)[0] == 'survived'
    assert classify(101, bad)[0] == 'killed'
    for code, text, bounded in [(101, 'could not compile', True), (101, bad, False),
        (0, good.replace('16 passed','0 passed'), True), (101, bad.replace('tools::x',''), True),
        (101, 'could not compile\n'+bad, True), (0, bad, True)]:
        assert classify(code, text, bounded)[0] == 'not_proven'

def run(root, packet, name, timeout):
    args = ['cargo','test','--locked','--bin','ub-review','tools::','--','--test-threads=2']
    env = {k: os.environ[k] for k in ('PATH','HOME','CARGO_HOME','RUSTUP_HOME') if k in os.environ}
    env.update(CARGO_TARGET_DIR=str(Path(os.environ['RUNNER_TEMP'])/'mutation-target'),
               CARGO_BUILD_JOBS='2', CARGO_TERM_COLOR='never', CARGO_INCREMENTAL='0')
    proc = subprocess.Popen(args, cwd=root, env=env, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, start_new_session=True)
    selector = selectors.DefaultSelector()
    selector.register(proc.stdout, selectors.EVENT_READ)
    captured = bytearray()
    start = time.monotonic()
    bounded = True
    try:
        while selector.get_map():
            if time.monotonic()-start > timeout:
                bounded = False
                break
            for key, _ in selector.select(timeout=.2):
                chunk = os.read(key.fd, 65536)
                if not chunk:
                    selector.unregister(key.fileobj)
                elif len(captured)+len(chunk) > 4*1024*1024:
                    bounded = False
                    break
                else:
                    captured.extend(chunk)
            if not bounded:
                break
        if not bounded:
            os.killpg(proc.pid, signal.SIGKILL)
        code = proc.wait(timeout=30)
    finally:
        if proc.poll() is None:
            os.killpg(proc.pid, signal.SIGKILL)
            proc.wait(timeout=30)
        selector.close()
        proc.stdout.close()
    text = captured.decode('utf-8',errors='replace')
    status, failed = classify(code, text, bounded)
    (packet/(name+'.log')).write_bytes(captured)
    return dict(name=name, command=args, status=status, failed_tests=failed,
                exit_code=code, within_bounds=bounded, seconds=round(time.monotonic()-start,3),
                log_bytes=len(captured), log_sha256=hashlib.sha256(captured).hexdigest())

def main():
    self_test()
    root, packet = Path(os.environ['SUBJECT_ROOT']), Path(os.environ['PACKET_ROOT'])
    packet.mkdir(exist_ok=False)
    def git(*args):
        return subprocess.check_output(['git','-C',str(root),*args],text=True,timeout=30).strip()
    assert git('rev-parse','HEAD') == HEAD
    assert git('rev-parse','HEAD^{tree}') == TREE
    assert not git('status','--porcelain','--untracked-files=no')
    originals = {p:(root/p).read_text() for p in (P,T)}
    tests = {p:hashlib.sha256((root/p).read_bytes()).hexdigest() for p in
             ('src/tests/tool_gate_receipt_tests.rs','src/tools/gate_receipt/tests.rs')}
    for name,p,old,new in MUTATIONS:
        assert originals[p].count(old)==1, (name, originals[p].count(old))
        assert old != new
    shard = int(os.environ['SHARD'])
    selected = [m for i,m in enumerate(MUTATIONS) if i%4==shard]
    receipt = dict(schema='ub-review.targeted_mutation.v1', authority='evidence-only',
        subject_head=HEAD, subject_tree=TREE, shard=shard,
        harness_commit=os.environ['HARNESS_SHA'], run_id=os.environ['GITHUB_RUN_ID'],
        attempt=os.environ['GITHUB_RUN_ATTEMPT'], test_sha256=tests, rows=[])
    def save():
        (packet/'receipt.json').write_text(json.dumps(receipt,indent=2)+'\n')
    try:
        receipt['baseline'] = run(root,packet,'baseline',900)
        save()
        assert receipt['baseline']['status']=='survived', 'initial control must pass all 16 tests'
        for name,p,old,new in selected:
            changed = originals[p].replace(old,new,1)
            (root/p).write_text(changed)
            (packet/(name+'.patch')).write_text(''.join(difflib.unified_diff(originals[p].splitlines(True),changed.splitlines(True),fromfile=p,tofile=p)))
            try:
                row = run(root,packet,name,240)
                row.update(path=p, before_sha256=hashlib.sha256(originals[p].encode()).hexdigest(),
                           mutated_sha256=hashlib.sha256(changed.encode()).hexdigest())
                receipt['rows'].append(row)
                print(json.dumps(row),flush=True)
                save()
            finally:
                (root/p).write_text(originals[p])
        receipt['restored_control'] = run(root,packet,'restored',240)
        assert receipt['restored_control']['status']=='survived'
        assert not git('status','--porcelain','--untracked-files=no')
        assert all(hashlib.sha256((root/p).read_bytes()).hexdigest()==sha for p,sha in tests.items())
        receipt['tracked_source_restored'] = True
        save()
    finally:
        for p,text in originals.items():
            (root/p).write_text(text)
    assert len(receipt['rows'])==len(selected)
    assert all(r['status'] != 'not_proven' for r in receipt['rows']), 'retain unproven experiment instead of claiming kill'

if __name__ == '__main__':
    main()
