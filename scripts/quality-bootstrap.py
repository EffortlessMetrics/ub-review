#!/usr/bin/env python3
"""Read two bounded GitHub bootstrap datasets; never publish partial results."""
from __future__ import annotations

import argparse
from datetime import datetime, timezone
import json
import os
from pathlib import Path
import re
import selectors
import shutil
import signal
import subprocess
import tempfile
import time

QUERIES = (
    ('actions-runs.json', ['gh', 'run', 'list', '--workflow', 'ub-review-gate.yml',
     '--event', 'pull_request', '--limit', '100', '--json',
     'databaseId,headSha,conclusion,createdAt,displayTitle,url']),
    ('pr-state.json', ['gh', 'pr', 'list', '--state', 'all', '--limit', '100', '--json',
     'number,title,state,isDraft,mergedAt,reviewDecision,headRefOid,comments,reviews,url']),
)
MAX_ATTEMPTS = 4
TOTAL_SECONDS = 90
CALL_SECONDS = 20
BACKOFF = (1, 3, 7)

class ReadFailure(Exception):
    """A bounded classification, deliberately excluding command output/secrets."""
    def __init__(self, reason, transient=False):
        super().__init__(reason)
        self.reason, self.transient = reason, transient


def transport(argv, timeout):
    """Capture stdout/stderr separately with hard memory and process deadlines."""
    try:
        child = subprocess.Popen(argv, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                                 start_new_session=True)
    except OSError as error:
        raise ReadFailure('client_unavailable') from error
    streams = {'stdout': bytearray(), 'stderr': bytearray()}
    selector = selectors.DefaultSelector()
    selector.register(child.stdout, selectors.EVENT_READ, 'stdout')
    selector.register(child.stderr, selectors.EVENT_READ, 'stderr')
    deadline = time.monotonic() + timeout
    try:
        while selector.get_map():
            remaining = deadline - time.monotonic()
            if remaining <= 0:
                raise ReadFailure('transport_timeout', True)
            for key, _ in selector.select(min(.2, remaining)):
                data = os.read(key.fd, 65536)
                if not data:
                    selector.unregister(key.fileobj)
                    continue
                limit = 16 * 1024 * 1024 if key.data == 'stdout' else 64 * 1024
                if len(streams[key.data]) + len(data) > limit:
                    raise ReadFailure('response_too_large')
                streams[key.data].extend(data)
        try:
            code = child.wait(timeout=max(.001, deadline-time.monotonic()))
        except subprocess.TimeoutExpired as error:
            raise ReadFailure('transport_timeout', True) from error
        return code, bytes(streams['stdout']), bytes(streams['stderr'])
    finally:
        if child.poll() is None:
            os.killpg(child.pid, signal.SIGKILL)
            child.wait(timeout=5)
        selector.close()
        child.stdout.close()
        child.stderr.close()


def failure(stderr):
    """Retry only the observed transient HTTP family, not arbitrary CLI errors."""
    text = stderr.decode('utf-8', errors='replace').lower()
    if 'rate limit' in text or 'rate_limit' in text:
        return ReadFailure('rate_limit_exhausted')
    status = re.search(r'\bhttp\s+(\d{3})\b', text)
    code = int(status.group(1)) if status else None
    if code in (500, 502, 503, 504):
        return ReadFailure('upstream_unavailable', True)
    if code in (401, 403) or 'authentication' in text or 'gh auth login' in text:
        return ReadFailure('authorization_failed')
    if 'graphql' in text:
        return ReadFailure('graphql_error')
    return ReadFailure('client_read_failed')


def validate(data, name):
    """Validate the bounded query shape before any final source file changes."""
    def unique(pairs):
        result = {}
        for key, value in pairs:
            if key in result:
                raise ValueError('duplicate key')
            result[key] = value
        return result
    try:
        value = json.loads(data, object_pairs_hook=unique,
                           parse_constant=lambda _: (_ for _ in ()).throw(ValueError('nonfinite')))
        if isinstance(value, dict) and value.get('errors'):
            raise ReadFailure('graphql_error')
        if not isinstance(value, list) or len(value) > 100:
            raise ValueError('expected bounded list')
        identity = 'databaseId' if name == 'actions-runs.json' else 'number'
        required = set(QUERIES[0 if identity == 'databaseId' else 1][1][-1].split(','))
        seen = set()
        for row in value:
            if not isinstance(row, dict) or type(row.get(identity)) is not int or row[identity] <= 0:
                raise ValueError('missing identity')
            if not required.issubset(row):
                raise ValueError('incomplete query row')
            if row[identity] in seen:
                raise ValueError('duplicate identity')
            seen.add(row[identity])
            if identity == 'number' and (not isinstance(row.get('comments'), list) or not isinstance(row.get('reviews'), list)):
                raise ValueError('missing reviewer data')
        return value
    except (ValueError, UnicodeError, TypeError) as error:
        raise ReadFailure('malformed_response') from error


def promote(out, values):
    """Single-writer staged replacement; retain the prior snapshot, not reader isolation."""
    out.parent.mkdir(parents=True, exist_ok=True)
    if out.is_symlink() or (out.exists() and not out.is_dir()):
        raise ReadFailure('invalid_source_destination')
    stage = Path(tempfile.mkdtemp(prefix='.quality-bootstrap-', dir=out.parent))
    previous = None
    try:
        for name, rows in values.items():
            (stage/name).write_text(json.dumps(rows, ensure_ascii=False)+'\n', encoding='utf-8')
        (stage/'pr-numbers.txt').write_text(''.join(f"{r['number']}\n" for r in values['pr-state.json']))
        if out.exists():
            backup = Path(tempfile.mkdtemp(prefix='.quality-previous-', dir=out.parent))
            previous = backup/'github'
            os.replace(out, previous)
        try:
            os.replace(stage, out)
        except OSError:
            if previous is not None:
                os.replace(previous, out)
            raise
        return str(previous) if previous is not None else None
    finally:
        if stage.exists():
            shutil.rmtree(stage)


def collect(out, diagnostic, runner=transport, clock=time.monotonic, sleep=time.sleep):
    """Collect within one total deadline; failure receipts contain no response bodies."""
    # A crash cannot leave a previous success receipt looking current.
    diagnostic.unlink(missing_ok=True)
    started = clock()
    receipt = dict(schema='ub-review.quality_bootstrap.v1', collection_complete=False,
                   status='unavailable', started_at=datetime.now(timezone.utc).isoformat(),
                   repository=os.environ.get('GH_REPO'), run_id=os.environ.get('GITHUB_RUN_ID'),
                   run_attempt=os.environ.get('GITHUB_RUN_ATTEMPT'), query_limit=100,
                   coverage='two bounded bootstrap queries only', attempts=[])
    values = {}
    try:
        for name, argv in QUERIES:
            for attempt in range(1, MAX_ATTEMPTS+1):
                remaining = TOTAL_SECONDS-(clock()-started)
                if remaining <= 0:
                    raise ReadFailure('deadline_exhausted')
                try:
                    code, stdout, stderr = runner(argv, min(CALL_SECONDS, remaining))
                    if code:
                        raise failure(stderr)
                    rows = validate(stdout, name)
                    receipt['attempts'].append(dict(query=name, attempt=attempt, status='complete', rows=len(rows)))
                    values[name] = rows
                    break
                except ReadFailure as error:
                    receipt['attempts'].append(dict(query=name, attempt=attempt, status=error.reason))
                    if not error.transient or attempt == MAX_ATTEMPTS:
                        raise
                    delay = BACKOFF[attempt-1]
                    if clock()-started+delay >= TOTAL_SECONDS:
                        raise ReadFailure('deadline_exhausted') from error
                    sleep(delay)
        if clock()-started >= TOTAL_SECONDS:
            raise ReadFailure('deadline_exhausted')
        receipt['previous_snapshot'] = promote(out, values)
        receipt.update(status='complete', collection_complete=True)
    except ReadFailure as error:
        receipt['status'] = error.reason
    except OSError:
        receipt['status'] = 'local_io_failed'
    receipt['elapsed_seconds'] = round(clock()-started, 3)
    receipt['completed_at'] = datetime.now(timezone.utc).isoformat()
    diagnostic.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.NamedTemporaryFile(mode='w', encoding='utf-8', dir=diagnostic.parent, delete=False) as stream:
        json.dump(receipt, stream, indent=2)
        temporary = Path(stream.name)
    try:
        os.replace(temporary, diagnostic)
    finally:
        temporary.unlink(missing_ok=True)
    return receipt


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--out', required=True, type=Path)
    parser.add_argument('--diagnostic', required=True, type=Path)
    args = parser.parse_args()
    receipt = collect(args.out, args.diagnostic)
    print(f"GitHub bootstrap: {receipt['status']}; {len(receipt['attempts'])} attempts")
    return 0 if receipt['collection_complete'] else 1

if __name__ == '__main__':
    raise SystemExit(main())
