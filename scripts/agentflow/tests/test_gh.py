import json

import pytest

from agentflow import lock
from agentflow.gh import Gh, GhError, Issue
from agentflow.state import Ctx, DryRunViolation
from agentflow.tests.conftest import FakeRunner

REPO = "SShadowS/al-sem"


def issues_page(*nums):
    return json.dumps([[{
        "number": n, "title": f"t{n}", "body": "## Acceptance\nx", "user": {"login": "SShadowS"},
        "labels": [{"name": "enhancement"}], "created_at": f"2026-09-{n:02d}T00:00:00Z",
    } for n in nums]])


def test_list_open_issues_paginates_and_drops_prs(ctx):
    page = json.loads(issues_page(1, 2))
    page[0].append({"number": 3, "title": "pr", "body": "", "user": {"login": "x"}, "labels": [],
                    "created_at": "2026-09-03T00:00:00Z", "pull_request": {}})
    r = FakeRunner({f"api repos/{REPO}/issues?state=open&per_page=100 --paginate --slurp": json.dumps(page)})
    gh = Gh(ctx, REPO, run=r, sleep=lambda s: None)
    got = gh.list_open_issues()
    assert [i.number for i in got] == [1, 2]
    assert got[0].labels == frozenset({"enhancement"}) and got[0].author == "SShadowS"


def test_retries_on_5xx_then_succeeds(ctx):
    r = FakeRunner({f"api repos/{REPO}/collaborators?permission=push&per_page=100 --paginate --slurp":
                    json.dumps([[{"login": "SShadowS"}]])}, fail_at=1)
    slept = []
    gh = Gh(ctx, REPO, run=r, sleep=slept.append)
    assert gh.collaborators() == {"SShadowS"}
    assert slept == [1]
    assert len(r.calls) == 2


def test_gives_up_after_three_attempts(ctx):
    r = FakeRunner({})
    r.responses["auth status"] = (1, "", "HTTP 503: down")
    gh = Gh(ctx, REPO, run=r, sleep=lambda s: None)
    with pytest.raises(GhError):
        gh._raw(["auth", "status"])
    assert len(r.calls) == 3


def test_mutations_need_lock_fence_and_refuse_dry_run(ctx, dry_ctx):
    r = FakeRunner({f"issue edit 8 --add-label agent-working": ""})
    with pytest.raises(lock.FenceError):
        Gh(ctx, REPO, run=r).add_labels(8, ["agent-working"])
    lock.acquire(ctx, 8, "s", 1)
    Gh(ctx, REPO, run=r).add_labels(8, ["agent-working"])
    assert r.calls[-1] == "issue edit 8 --add-label agent-working"
    with pytest.raises(DryRunViolation):
        Gh(dry_ctx, REPO, run=FakeRunner(readonly=True)).comment(8, "hi")


def test_create_issue_parses_number_from_url(ctx):
    lock.acquire(ctx, 8, "s", 1)
    r = FakeRunner({"issue create --title T --body-file * ": "https://github.com/SShadowS/al-sem/issues/42\n",
                    "issue create *": "https://github.com/SShadowS/al-sem/issues/42\n"})
    n = Gh(ctx, REPO, run=r).create_issue("T", "body", ["agent-filed", "bug"])
    assert n == 42


def test_ensure_labels_creates_only_missing(ctx):
    lock.acquire(ctx, 8, "s", 1)
    r = FakeRunner({f"api repos/{REPO}/labels?per_page=100 --paginate --slurp": json.dumps([[{"name": "agent-done"}]]),
                    f"api repos/{REPO}/labels -X POST -f name=agent-working -f color=ededed": "{}"})
    Gh(ctx, REPO, run=r).ensure_labels(["agent-done", "agent-working"])
    assert sum("-X POST" in c for c in r.calls) == 1
