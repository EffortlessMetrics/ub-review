#!/usr/bin/env python3
"""Protected-source read-only native PR admission. No candidate checkout.

Policy owner: release/ci (#1465). Not an approval, check writer or merge service.
"""
import argparse
import base64
import datetime
import hashlib
import json
import os
from pathlib import Path
import re
import signal
import sys
import urllib.parse
import urllib.request

REPO = "EffortlessMetrics/ub-review"
SCHEMA = "ub-review.premerge.v2"
ASSOCIATIONS = {"OWNER", "MEMBER", "COLLABORATOR"}
ANCHORS = {
    "gate": (289600864, 39015965204, 1858, ".github/workflows/ub-review-gate.yml"),
    "baseline": (342497115, 39015965202, 678, ".github/workflows/independent-baseline.yml"),
}
LABEL = re.compile(r"ci:[A-Za-z0-9_-]{43}\Z")
TITLE = re.compile(r"premerge-v1 ([1-9][0-9]*) ([0-9a-f]{40}) ([0-9a-f]{40}) (\S+) (\S+) ([0-9]+) ([\s\S]+)\Z")
KEYS = {"schema", "repository", "pr", "head_repository", "head_sha", "base_ref", "base_sha",
        "merge_sha", "merge_tree", "policy_sha256", "owner_id", "objective", "review_ids", "review_generation", "retry"}


class Refusal(Exception):
    pass


def require(condition, reason):
    if not condition:
        raise Refusal(reason)


def pairs(items):
    value = {}
    for key, item in items:
        require(key not in value, "duplicate JSON field")
        value[key] = item
    return value


def decode(data):
    return json.loads(data, object_pairs_hook=pairs,
                      parse_constant=lambda _: require(False, "nonfinite JSON"))


def native_time(value):
    require(isinstance(value, str), "missing native event timestamp")
    stamp = datetime.datetime.fromisoformat(value)
    require(stamp.tzinfo is not None, "native event timestamp has no timezone")
    return stamp


def canonical(value):
    return json.dumps(value, sort_keys=True, separators=(",", ":"), ensure_ascii=True)


def digest(value):
    return hashlib.sha256(canonical(value).encode()).hexdigest()


def label_for(body):
    return "ci:" + base64.urlsafe_b64encode(hashlib.sha256(body.encode()).digest()).decode().rstrip("=")


class PublicGitHub:
    """Public reads; existing job token only for the fixed current-authority read.

    Run solely from digest-verified protected base, before any candidate checkout.
    Never load candidate modules, credential stores, redirects or retry transports.
    """
    def __init__(self):
        class NoRedirect(urllib.request.HTTPRedirectHandler):
            def redirect_request(self, req, fp, code, msg, headers, newurl):
                raise Refusal("public GitHub redirect refused")
        self.opener = urllib.request.build_opener(NoRedirect)
        self.calls = 0
        self.histories = {}

    def get(self, suffix):
        self.calls += 1
        require(self.calls <= 40, "public metadata request bound exceeded")
        require(suffix.startswith("/") and ".." not in suffix and "#" not in suffix,
                "invalid metadata path")
        url = "https://api.github.com/repos/" + REPO + suffix
        headers = {"Accept": "application/vnd.github+json", "User-Agent": "ub-review-premerge-v1",
                   "X-GitHub-Api-Version": "2022-11-28"}
        if re.fullmatch(r"/collaborators/[A-Za-z0-9-]{1,39}/permission", suffix):
            token = os.environ.get("ADMISSION_METADATA_TOKEN", "")
            require(token and not any(char.isspace() for char in token), "existing job metadata token unavailable")
            headers["Authorization"] = "Bearer " + token
        request = urllib.request.Request(url, headers=headers)
        with self.opener.open(request, timeout=10) as response:
            data = response.read(2_000_001)
        require(len(data) <= 2_000_000, "public metadata byte bound exceeded")
        return decode(data)

    def pages(self, suffix, field=None, limit=5):
        result = []
        for page in range(1, limit + 1):
            separator = "&" if "?" in suffix else "?"
            payload = self.get(f"{suffix}{separator}per_page=100&page={page}")
            values = payload[field] if field else payload
            require(isinstance(values, list) and len(values) <= 100, "invalid metadata page")
            result.extend(values)
            if len(values) < 100:
                return result
        raise Refusal("incomplete metadata pagination; no qualification allocated")


def generation(reviews, review_ids=None):
    selected = [review for review in reviews if review_ids is None or review["id"] in review_ids]
    return digest(sorted(({**{key: review[key] for key in
                            ("id", "state", "commit_id", "submitted_at", "body", "author_association")},
                           "reviewer_id": review["user"]["id"]}
                          for review in selected), key=lambda review: review["id"]))


def classify(event_name, event):
    action = event.get("action", "")
    label = event.get("label", {}).get("name", "")
    if event_name in {"pull_request", "pull_request_target"}:
        if action == "labeled" and LABEL.fullmatch(label):
            return "request"
        if action in {"synchronize", "converted_to_draft", "closed"}:
            return "invalidate"
        if action == "unlabeled" and LABEL.fullmatch(label):
            return "invalidate"
        if action == "edited" and "base" in event.get("changes", {}):
            return "invalidate"
    if event_name == "pull_request_review":
        if action == "submitted":
            state = event["review"]["state"].lower()
            require(state in {"approved", "commented", "changes_requested"}, "unknown submitted review state")
            return "invalidate" if state == "changes_requested" else "observation"
        if action in {"dismissed", "edited"}:
            return "review"
    return "observation"


def request_for(comments, label, owner_id=None):
    matches = [comment for comment in comments if label_for(comment.get("body", "")) == label
               and (owner_id is None or comment.get("user", {}).get("id") == owner_id)]
    require(matches, "request bytes absent, edited, deleted or owned by another actor")
    comment = min(matches, key=lambda item: item["id"])
    body = decode(comment["body"])
    require(isinstance(body, dict) and set(body) == KEYS and body["schema"] == SCHEMA,
            "invalid request schema or unexpected field")
    require(comment["user"]["type"] == "User" and comment["author_association"] in ASSOCIATIONS
            and type(body["owner_id"]) is int and body["owner_id"] == comment["user"]["id"],
            "native request owner identity is ineligible; App request effects are not admitted")
    for key in ("head_sha", "base_sha", "merge_sha", "merge_tree"):
        require(isinstance(body[key], str) and re.fullmatch("[0-9a-f]{40}", body[key]), "invalid full source identity")
    require(re.fullmatch("[0-9a-f]{64}", body["policy_sha256"]) and
            re.fullmatch("[0-9a-f]{64}", body["review_generation"]), "invalid policy/review digest")
    require(type(body["pr"]) is int and body["pr"] > 0 and isinstance(body["review_ids"], list)
            and body["review_ids"] and all(type(item) is int and item > 0 for item in body["review_ids"])
            and body["review_ids"] == sorted(set(body["review_ids"])), "invalid native review identities")
    require(isinstance(body["objective"], str) and re.fullmatch(
        r"https://github.com/EffortlessMetrics/ub-review/issues/[1-9][0-9]*", body["objective"]),
        "missing existing objective evidence")
    retry = body["retry"]
    require(retry is None or (isinstance(retry, dict) and set(retry) == {"generation", "prior_request", "runs", "cause"}
            and retry["cause"] in {"terminal", "withdrawal"}
            and type(retry["generation"]) is int and 1 <= retry["generation"] <= 3
            and isinstance(retry["prior_request"], str) and re.fullmatch("[0-9a-f]{64}", retry["prior_request"])
            and isinstance(retry["runs"], dict) and set(retry["runs"]) == set(ANCHORS)
            and all(type(value) is int and value > 0 for value in retry["runs"].values())),
            "invalid bounded retry transaction")
    return body


def current(api, event, label, policy_hash):
    pr = event["pull_request"]
    number = pr["number"]
    require(event["repository"]["full_name"] == REPO and event["repository"]["private"] is False,
            "public repository only; no credential fallback")
    live = api.get(f"/pulls/{number}")
    comments = api.pages(f"/issues/{number}/comments")
    request = request_for(comments, label, event["sender"]["id"])
    require(event["sender"]["type"] == "User", "App requester effect is unqualified")
    require(live["state"] == "open" and live["draft"] is False and live["mergeable"] is True,
            "PR closed, draft, conflicting or merge state unavailable")
    require(request["repository"] == REPO and request["pr"] == number
            and request["head_repository"] == pr["head"]["repo"]["full_name"]
            == live["head"]["repo"]["full_name"]
            and request["head_sha"] == pr["head"]["sha"] == live["head"]["sha"]
            and request["base_ref"] == pr["base"]["ref"] == live["base"]["ref"] == "main"
            and pr["base"]["repo"]["full_name"] == live["base"]["repo"]["full_name"] == REPO
            and request["base_sha"] == pr["base"]["sha"] == live["base"]["sha"],
            "request repository/PR/head/base/target is stale or mismatched")
    owners = live["assignees"]
    require(len(owners) == 1 and owners[0]["id"] == request["owner_id"],
            "exactly one accountable native assignee must own the selected candidate")
    login = owners[0]["login"]
    require(isinstance(login, str) and re.fullmatch("[A-Za-z0-9-]{1,39}", login), "invalid native owner login")
    # Current assignment/old association are not a current permission grant.
    # Only this protected-source helper receives the existing metadata-read token.
    permission = api.get(f"/collaborators/{login}/permission")
    require(permission["user"]["id"] == request["owner_id"] and permission["permission"] in {"write", "admin"},
            "current native owner permission lost, read-only, unknown or mismatched")
    require(request["policy_sha256"] == policy_hash, "request policy generation is stale")
    require(label in [item["name"] for item in live["labels"]], "request withdrawn")
    # Other actors' labels are not selection authority and cannot withdraw this request.
    active = []
    for item in live["labels"]:
        if LABEL.fullmatch(item["name"]):
            try:
                candidate = request_for(comments, item["name"], request["owner_id"])
                if candidate["head_sha"] == request["head_sha"] and candidate["base_sha"] == request["base_sha"]:
                    active.append(item["name"])
            except (Refusal, KeyError, TypeError, ValueError):
                continue
    require(active == [label], "ambiguous selected request; retain one candidate request label")
    objective = api.get("/issues/" + request["objective"].rsplit("/", 1)[1])
    require(objective["state"] == "open" and "pull_request" not in objective, "objective unavailable or no longer open")
    reviews = api.pages(f"/pulls/{number}/reviews")
    require(generation(reviews, request["review_ids"]) == request["review_generation"], "review/evidence generation changed")
    by_id = {review["id"]: review for review in reviews}
    for review_id in request["review_ids"]:
        review = by_id[review_id]
        require(review["commit_id"] == request["head_sha"] and review["state"] in {"APPROVED", "COMMENTED"}
                and review["body"].strip() and review["author_association"] in ASSOCIATIONS,
                "referenced review is absent, stale, dismissed or not a substantive native report")
    decisive = {}
    for review in reviews:
        require(review["state"] in {"PENDING", "COMMENTED", "APPROVED", "CHANGES_REQUESTED", "DISMISSED"},
                "unknown native review state")
        # Draft review IDs precede submission. COMMENTED does not dismiss a block.
        if review["state"] in {"APPROVED", "CHANGES_REQUESTED"}:
            submitted = native_time(review["submitted_at"])
            decisive.setdefault(review["user"]["id"], []).append((submitted, review["state"]))
    for submissions in decisive.values():
        latest_time = max(item[0] for item in submissions)
        require(all(state != "CHANGES_REQUESTED" for time, state in submissions if time == latest_time),
                "native blocking review (ambiguous same-time decisions remain blocking)")
    require(live["merge_commit_sha"] == request["merge_sha"], "integration subject changed")
    merge = api.get("/git/commits/" + request["merge_sha"])
    require(merge["sha"] == request["merge_sha"] and merge["tree"]["sha"] == request["merge_tree"]
            and [parent["sha"] for parent in merge["parents"]] == [request["base_sha"], request["head_sha"]],
            "integration commit/tree/ordered parents mismatch")
    events = api.pages(f"/issues/{number}/events")
    edges = [edge for edge in events if edge.get("label", {}).get("name") == label
             and edge["event"] in {"labeled", "unlabeled"}]
    require(edges and all(edge["event"] == "labeled" and edge["actor"]["id"] == request["owner_id"] for edge in edges),
            "request edge is absent, replayed after withdrawal, or has another owner")
    requested_at = min(native_time(edge["created_at"]) for edge in edges)
    # Native review records, not candidate-controlled gate titles, retire BOTH populations.
    require(not any(review["state"] == "CHANGES_REQUESTED" and native_time(review["submitted_at"]) >= requested_at
                    for review in reviews), "blocking review after request permanently retired its generation")
    for event_record in events:
        if event_record["event"] != "review_dismissed":
            continue
        dismissed = event_record["dismissed_review"]
        review_id = dismissed["review_id"]
        require((type(review_id) is int and review_id > 0) or
                (isinstance(review_id, str) and re.fullmatch("[1-9][0-9]*", review_id)), "unknown dismissed native review ID")
        state = dismissed["state"].lower()
        require(state in {"commented", "approved", "changes_requested"}, "unknown dismissed native review state")
        if state == "changes_requested" and native_time(event_record["created_at"]) >= requested_at:
            review = by_id.get(int(review_id))
            require(review is not None and native_time(review["submitted_at"]) < requested_at,
                    "post-request blocking review was dismissed; prior request remains retired")
    return request, comments


def history(api, lane, run_id):
    workflow_id, anchor_id, floor, path = ANCHORS[lane]
    if lane in api.histories:
        rows = api.histories[lane]
        own = [row for row in rows if row["id"] == run_id]
        require(len(own) == 1, "referenced run absent from complete native history")
        return own[0], rows
    anchor = api.get(f"/actions/runs/{anchor_id}")
    require((anchor["id"], anchor["workflow_id"], anchor["run_number"], anchor["path"])
            == (anchor_id, workflow_id, floor, path), "native history anchor unavailable or changed")
    rows = []
    seen = set()
    for page in range(1, 13):
        payload = api.get(f"/actions/workflows/{workflow_id}/runs?per_page=100&page={page}")
        batch = payload["workflow_runs"]
        require(isinstance(batch, list) and 0 < len(batch) <= 100, "incomplete run history")
        for row in batch:
            require(row["workflow_id"] == workflow_id and row["path"] == path
                    and row["repository"]["full_name"] == REPO and row["id"] not in seen,
                    "unverifiable/duplicate native history row")
            seen.add(row["id"])
            if row["run_number"] >= floor:
                rows.append(row)
        if any(row["id"] == anchor_id for row in rows):
            break
    else:
        raise Refusal("run history bound exceeded; owner must refresh reviewed protocol/anchor")
    numbers = [row["run_number"] for row in rows]
    require(numbers and all(type(number) is int and floor <= number < floor + 1200 for number in numbers),
            "native run sequence bound exceeded")
    require(len(numbers) == len(set(numbers)) and set(numbers) == set(range(floor, max(numbers) + 1)),
            "deleted, missing or incomplete native run history")
    own = [row for row in rows if row["id"] == run_id]
    require(len(own) == 1, "current run absent from native history")
    api.histories[lane] = rows
    return own[0], rows


def elect(api, event, lane, run_id, request, comments):
    own, rows = history(api, lane, run_id)
    require(own["event"] == ("pull_request_target" if lane == "baseline" else "pull_request"),
            "wrong native request event")
    require(own["head_sha"] == request["head_sha"] and own["actor"]["id"] == request["owner_id"],
            "native run/head/requester mismatch")
    candidates = []
    for row in rows:
        if row["event"] == "workflow_dispatch":
            continue  # This protocol never qualifies dispatch; no PR check association is assumed.
        title = row["display_title"]
        match = TITLE.fullmatch(title)
        if not title.startswith("premerge-v1 "):
            continue  # Legacy, before this protocol; never an admission receipt.
        prefix = re.match(r"premerge-v1 ([1-9][0-9]*) ", title)
        associated = any(pr["number"] == request["pr"] for pr in row["pull_requests"])
        if prefix and int(prefix.group(1)) != request["pr"]:
            require(not associated, "native PR/run title mismatch")
            continue
        require(match is not None, "malformed protocol run identity")
        number, head, base, base_ref, action, actor, label = match.groups()
        if int(number) != request["pr"]:
            continue
        require(row["head_sha"] == head and row["actor"]["id"] == int(actor), "forged native request display identity")
        if row["run_number"] <= own["run_number"] and action == "labeled" and LABEL.fullmatch(label) and int(actor) == request["owner_id"]:
            require(row["event"] == own["event"], "wrong native predecessor event")
            previous = request_for(comments, label, int(actor))
            require(previous["pr"] == int(number) and previous["head_sha"] == head
                    and previous["base_sha"] == base and previous["base_ref"] == base_ref,
                    "request/history source mismatch")
            require(any(pr["number"] == int(number) and pr["head"]["sha"] == head
                        and pr["base"]["sha"] == base and pr["base"]["ref"] == base_ref
                        for pr in row["pull_requests"]), "native PR association does not match request")
            if canonical(previous) == canonical(request):
                candidates.append(row)
        review_event = re.fullmatch(r"review:([1-9][0-9]*):([a-zA-Z_]+)", label)
        review_invalidated = row["event"] == "pull_request_review" and review_event and (
            (action == "submitted" and review_event[2].lower() == "changes_requested") or
            (action in {"edited", "dismissed"} and int(review_event[1]) in request["review_ids"]))
        if row["run_number"] > own["run_number"] and (review_invalidated or
                action in {"synchronize", "converted_to_draft", "closed"}
                or (action == "edited" and (head != request["head_sha"] or base != request["base_sha"] or base_ref != "main"))
                or (action == "unlabeled" and label == event["label"]["name"])):
            raise Refusal("native material transition retired this request")
    require(any(row["id"] == run_id for row in candidates), "current request not verifiable in native history")
    return min(candidates, key=lambda row: row["run_number"])


def prior_request(comments, identity, owner_id):
    matches = []
    for comment in comments:
        if comment.get("user", {}).get("id") != owner_id:
            continue
        try:
            request = request_for(comments, label_for(comment["body"]), owner_id)
            if digest(request) == identity:
                matches.append(request)
        except (Refusal, KeyError, TypeError, ValueError):
            continue
    require(matches, "retry predecessor request absent or not owned by the same native lead")
    return matches[0]


def validate_retry(api, request, comments, events):
    """At most three explicit successor transactions; no nonce or automatic rerun.

    Both prior populations must be terminal, with at least one failed/cancelled.
    An explicit successor spends both populations again, preserving old receipts.
    """
    child = request
    while child["retry"] is not None:
        retry = child["retry"]
        parent = prior_request(comments, retry["prior_request"], child["owner_id"])
        require({key: value for key, value in parent.items() if key != "retry"}
                == {key: value for key, value in child.items() if key != "retry"},
                "retry changed source, evidence, owner, objective or policy")
        previous_generation = parent["retry"]["generation"] if parent["retry"] is not None else 0
        require(retry["generation"] == previous_generation + 1, "retry generation is not consecutive")
        conclusions = []
        terminal_times = []
        winner_labels = set()
        for lane, run_id in retry["runs"].items():
            row, rows = history(api, lane, run_id)
            expected_event = "pull_request_target" if lane == "baseline" else "pull_request"
            candidates = []
            for other in rows:
                match = TITLE.fullmatch(other["display_title"])
                if not match:
                    require(not other["display_title"].startswith("premerge-v1 "), "unknown predecessor protocol row")
                    continue
                number, head, base, base_ref, action, actor, label = match.groups()
                if int(number) != parent["pr"] or action != "labeled" or not LABEL.fullmatch(label):
                    continue
                if int(actor) != parent["owner_id"]:
                    continue
                predecessor = request_for(comments, label, parent["owner_id"])
                if canonical(predecessor) != canonical(parent):
                    continue
                require(other["event"] == expected_event and other["actor"]["id"] == parent["owner_id"]
                        and other["head_sha"] == head == parent["head_sha"]
                        and base == parent["base_sha"] and base_ref == parent["base_ref"]
                        and any(pr["number"] == parent["pr"] and pr["head"]["sha"] == head
                                and pr["base"]["sha"] == base and pr["base"]["ref"] == base_ref
                                for pr in other["pull_requests"]), "retry predecessor native identity mismatch")
                candidates.append(other)
            require(candidates and min(candidates, key=lambda item: item["run_number"])["id"] == run_id,
                    "retry does not reference the original elected predecessor")
            require(row["run_attempt"] == 1 and row["status"] == "completed"
                    and row["conclusion"] in {"success", "failure", "cancelled", "timed_out"},
                    "retry predecessor is nonterminal, rerun, unknown or ineligible")
            conclusions.append(row["conclusion"])
            terminal_times.append(native_time(row["updated_at"]))
            winner_labels.add(TITLE.fullmatch(row["display_title"])[7])
        if retry["cause"] == "terminal":
            require(any(value != "success" for value in conclusions), "successful transaction needs verified withdrawal")
            retired_at = max(terminal_times)
        else:
            require(all(value == "success" for value in conclusions), "failed transaction must use terminal retry")
            retirements = []
            for label in winner_labels:
                withdrawals = [native_time(edge["created_at"]) for edge in events
                               if edge.get("label", {}).get("name") == label and edge["event"] == "unlabeled"
                               and edge["actor"]["id"] == parent["owner_id"]
                               and native_time(edge["created_at"]) > max(terminal_times)]
                require(withdrawals, "successful predecessor has no verified owner withdrawal of its elected label")
                retirements.append(min(withdrawals))
            retired_at = max(retirements)
        # Native comment and application must both follow terminal completion.
        matching = []
        for comment in comments:
            if comment.get("user", {}).get("id") != child["owner_id"]:
                continue
            try:
                if digest(request_for(comments, label_for(comment.get("body", "")), child["owner_id"])) == digest(child):
                    matching.append(comment)
            except (Refusal, KeyError, TypeError, ValueError):
                continue
        labels = {label_for(comment["body"]) for comment in matching}
        edges = [edge for edge in events if edge.get("label", {}).get("name") in labels and edge["event"] == "labeled"]
        require(matching and edges and all(edge["actor"]["id"] == child["owner_id"] for edge in edges)
                and all(native_time(comment["created_at"]) > retired_at for comment in matching)
                and all(native_time(edge["created_at"]) > retired_at for edge in edges),
                "retry intent predates terminal predecessor settlement")
        child = parent


def review_transition(api, event):
    """Only mutation/withdrawal of selected evidence invalidates an otherwise harmless review event."""
    number = event["pull_request"]["number"]
    live = api.get(f"/pulls/{number}")
    comments = api.pages(f"/issues/{number}/comments")
    owners = [owner["id"] for owner in live["assignees"]]
    for label in live["labels"]:
        if not LABEL.fullmatch(label["name"]):
            continue
        try:
            request = request_for(comments, label["name"])
        except (Refusal, KeyError, TypeError, ValueError):
            continue
        if request["owner_id"] in owners and request["head_sha"] == live["head"]["sha"] \
                and request["base_sha"] == live["base"]["sha"] and request["base_ref"] == live["base"]["ref"] \
                and event["review"]["id"] in request["review_ids"]:
            return {"mode": "invalidated", "required": "true"}
    return {"mode": "observation", "required": "false"}


def execute(api, event_name, event, lane, run_id, attempt, phase, policy_hash,
            mode="", product="", cancelled=False, admission_result="success", native_sha=""):
    route = classify(event_name, event)
    if route == "observation":
        return {"mode": "observation", "required": "false"}
    if phase == "finalize" and mode in {"observation", "duplicate", "replay"}:
        return {"mode": mode, "required": "false"}
    if route == "review":
        return review_transition(api, event)
    if route == "invalidate":
        # Material events emit failure only, never a replacement green context.
        return {"mode": "invalidated", "required": "true"}
    if attempt != 1:
        return {"mode": "replay", "required": "false", "winner": str(run_id), "attempt": "1"}
    request, comments = current(api, event, event["label"]["name"], policy_hash)
    if lane == "gate":
        require(native_sha == request["merge_sha"], "native merge checkout differs from requested integration subject")
    winner = elect(api, event, lane, run_id, request, comments)
    if winner["id"] != run_id:
        return {"mode": "duplicate", "required": "false", "winner": str(winner["id"]),
                "attempt": "1"}
    if request["retry"] is not None:
        edges = api.pages(f"/issues/{request['pr']}/events")
        validate_retry(api, request, comments, edges)
    require(not cancelled, "own workflow cancelled")
    if phase == "finalize":
        require(admission_result == "success" and mode == "admitted" and product == "success",
                "missing, failed, skipped or cancelled product/admission")
    if phase == "finalize" and lane == "gate":
        artifacts = api.pages(f"/actions/runs/{run_id}/artifacts", field="artifacts")
        packets = [item for item in artifacts if item["name"] == "ub-review-gate"]
        require(len(packets) == 1 and packets[0]["expired"] is False and packets[0]["size_in_bytes"] > 0
                and re.fullmatch("sha256:[0-9a-f]{64}", packets[0]["digest"])
                and packets[0]["workflow_run"]["id"] == run_id
                and packets[0]["workflow_run"]["head_sha"] == request["head_sha"],
                "required gate packet metadata missing, expired, empty or bound to another run/head")
    return {"mode": "admitted", "required": "true", "request": digest(request)}


def prepare(api, number, objective, owner_id, review_ids, policy_hash, retry_of=None, gate_run=None, baseline_run=None,
            retry_cause="terminal"):
    """Prepare bytes only. The existing external caller posts/labels explicitly."""
    pr = api.get(f"/pulls/{number}")
    require(pr["state"] == "open" and pr["draft"] is False and pr["mergeable"] is True,
            "prepare requires the selected, non-draft conflict-free candidate")
    require(pr["base"]["repo"]["full_name"] == REPO and pr["base"]["ref"] == "main",
            "prepare requires the canonical protected target")
    require(len(pr["assignees"]) == 1 and pr["assignees"][0]["id"] == owner_id,
            "assign exactly one accountable candidate lead first")
    reviews = api.pages(f"/pulls/{number}/reviews")
    merge = api.get("/git/commits/" + pr["merge_commit_sha"])
    request = {"schema": SCHEMA, "repository": REPO, "pr": number,
               "head_repository": pr["head"]["repo"]["full_name"], "head_sha": pr["head"]["sha"],
               "base_ref": "main", "base_sha": pr["base"]["sha"], "merge_sha": pr["merge_commit_sha"],
               "merge_tree": merge["tree"]["sha"], "policy_sha256": policy_hash,
               "owner_id": owner_id, "objective": f"https://github.com/{REPO}/issues/{objective}",
               "review_ids": sorted(set(review_ids)), "review_generation": generation(reviews, review_ids), "retry": None}
    if retry_of is not None:
        comments = api.pages(f"/issues/{number}/comments")
        previous = prior_request(comments, retry_of, owner_id)
        request["retry"] = {"generation": (previous["retry"]["generation"] if previous["retry"] else 0) + 1,
                            "prior_request": retry_of, "runs": {"gate": gate_run, "baseline": baseline_run}, "cause": retry_cause}
        require(1 <= request["retry"]["generation"] <= 3 and type(gate_run) is int and gate_run > 0
                and type(baseline_run) is int and baseline_run > 0, "retry needs both native winners and generation <=3")
        require({key: value for key, value in previous.items() if key != "retry"}
                == {key: value for key, value in request.items() if key != "retry"}, "retry subject changed")
    else:
        require(gate_run is None and baseline_run is None and retry_cause == "terminal", "successor options require --retry-of")
    body = canonical(request)
    return {"body": body, "label": label_for(body)}


def main():
    def deadline(_signum, _frame):
        raise Refusal("metadata wall deadline exceeded")
    signal.signal(signal.SIGALRM, deadline)
    signal.alarm(120)
    if len(sys.argv) > 1:
        parser = argparse.ArgumentParser(description=__doc__)
        parser.add_argument("operation", choices=["prepare"])
        parser.add_argument("--pr", type=int, required=True)
        parser.add_argument("--objective", type=int, required=True)
        parser.add_argument("--owner-id", type=int, required=True)
        parser.add_argument("--review-id", type=int, action="append", required=True)
        parser.add_argument("--retry-of")
        parser.add_argument("--gate-run", type=int)
        parser.add_argument("--baseline-run", type=int)
        parser.add_argument("--retry-cause", choices=["terminal", "withdrawal"], default="terminal")
        args = parser.parse_args()
        try:
            print(canonical(prepare(PublicGitHub(), args.pr, args.objective, args.owner_id,
                                    args.review_id, hashlib.sha256(Path(__file__).read_bytes()).hexdigest(),
                                    args.retry_of, args.gate_run, args.baseline_run, args.retry_cause)))
            return 0
        except (Refusal, KeyError, TypeError, ValueError, OSError) as error:
            print("request preparation refused:", error, file=sys.stderr)
            return 1
    event = decode(Path(os.environ["GITHUB_EVENT_PATH"]).read_text())
    result = {"mode": "rejected", "required": "true"}
    code = 0
    try:
        result = execute(PublicGitHub(), os.environ["GITHUB_EVENT_NAME"], event,
                         os.environ["ADMISSION_LANE"], int(os.environ["GITHUB_RUN_ID"]),
                         int(os.environ["GITHUB_RUN_ATTEMPT"]), os.environ["ADMISSION_PHASE"],
                         hashlib.sha256(Path(__file__).read_bytes()).hexdigest(),
                         os.environ.get("ADMISSION_MODE", ""), os.environ.get("PRODUCT_RESULT", ""),
                         os.environ.get("WORKFLOW_CANCELLED") == "true",
                         os.environ.get("ADMISSION_RESULT", "success"), os.environ.get("NATIVE_SHA", ""))
        if os.environ["ADMISSION_PHASE"] in {"recheck", "finalize"} and os.environ.get("ADMISSION_MODE") == "admitted":
            require(result["mode"] == "admitted", "qualification lost admission before execution/finalization")
        if result["mode"] == "invalidated":
            code = 1
    except (Refusal, KeyError, TypeError, ValueError, OSError) as error:
        # No public failure is absence/clean evidence. No retry or credential fallback.
        print("admission refused:", str(error), file=sys.stderr)
        code = 1
    print(canonical(result))
    if result.get("winner") and os.environ.get("GITHUB_STEP_SUMMARY"):
        with open(os.environ["GITHUB_STEP_SUMMARY"], "a", encoding="utf-8") as summary:
            summary.write("Existing request transaction: https://github.com/" + REPO + "/actions/runs/"
                          + result["winner"] + "/attempts/1 . Inspect that attempt; no new proof was allocated.\n")
    with open(os.environ["GITHUB_OUTPUT"], "a", encoding="utf-8") as output:
        for key, value in result.items():
            require("\n" not in value and "\r" not in value, "invalid output bytes")
            output.write(key + "=" + value + "\n")
    return code


if __name__ == "__main__":
    sys.exit(main())
