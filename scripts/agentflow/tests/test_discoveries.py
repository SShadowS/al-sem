import json

import pytest

from agentflow import budget, discoveries, lock
from agentflow.gh import Gh
from agentflow.state import read_json
from agentflow.tests.conftest import FakeRunner

REPO = "SShadowS/al-sem"


def disc(sym="test_x fails with 'unknown edge 42'", loc="tests/gap/x.rs", sub="resolve"):
    return discoveries.Discovery(subsystem=sub, locator=loc, symptom=sym, kind="bug", origin_issue=8,
                                 reproducer="cargo test --test gap x", pre_existing=False,
                                 capability="x resolves", acceptance="test_x passes")


def search_hit(fp, number):
    return json.dumps([{"number": number, "title": "t", "body": f"x {discoveries.MARKER_FMT.format(fp=fp)}",
                        "labels": [], "author": {"login": "SShadowS"}, "createdAt": "2026-09-01T00:00:00Z"}])


def setup(ctx):
    lock.acquire(ctx, 8, "s", 1)
    budget.init(ctx, claimed_at=ctx.now())


def test_fingerprint_normalizes_numbers_case_and_space():
    a = discoveries.fingerprint("resolve", "tests/gap/x.rs", "Unknown edge   42 at line 7")
    b = discoveries.fingerprint("Resolve", "tests\\gap\\x.rs", "unknown edge 99 at line 8")
    assert a == b and len(a) == 16


def test_file_all_writes_pending_before_create_then_filed(ctx):
    setup(ctx)
    d = disc()
    fp = discoveries.fingerprint(d.subsystem, d.locator, d.symptom)
    r = FakeRunner({f"issue list --repo {REPO} --state all --search {discoveries.MARKER_FMT.format(fp=fp)} --limit 100 --json *": "[]",
                    "issue create *": "https://github.com/SShadowS/al-sem/issues/50\n"})
    out = discoveries.file_all(ctx, Gh(ctx, REPO, run=r), [d], "https://s")
    assert out == [{"fp": fp, "status": "filed", "number": 50}]
    idx = read_json(ctx.paths.discoveries_index)
    assert idx[fp] == {"status": "filed", "number": 50, "origin": 8}
    create = next(c for c in r.calls if c.startswith("issue create"))
    assert "--label agent-filed --label bug" in create and "[agent-discovery][resolve]" in create


def test_crash_between_create_and_index_leaves_pending_and_reconcile_finds_marker(ctx):
    setup(ctx)
    d = disc()
    fp = discoveries.fingerprint(d.subsystem, d.locator, d.symptom)
    r = FakeRunner({f"issue list --repo {REPO} --state all --search {discoveries.MARKER_FMT.format(fp=fp)} --limit 100 --json *": "[]",
                    "issue create *": "garbage without a number"})
    out = discoveries.file_all(ctx, Gh(ctx, REPO, run=r), [d], "https://s")
    assert out[0]["status"] == "pending"
    assert read_json(ctx.paths.discoveries_index)[fp]["status"] == "pending"
    r2 = FakeRunner({f"issue list --repo {REPO} --state all --search {discoveries.MARKER_FMT.format(fp=fp)} --limit 100 --json *": search_hit(fp, 51)})
    rep = discoveries.reconcile_pending(ctx, Gh(ctx, REPO, run=r2))
    assert rep == [{"fp": fp, "status": "filed", "number": 51}]


def test_ambiguous_reconcile_stays_pending_and_never_recreates(ctx):
    setup(ctx)
    d = disc()
    fp = discoveries.fingerprint(d.subsystem, d.locator, d.symptom)
    discoveries._save_index(ctx, {fp: {"status": "pending", "number": None, "origin": 8}})
    two = json.loads(search_hit(fp, 1)) + json.loads(search_hit(fp, 2))
    r = FakeRunner({f"issue list --repo {REPO} --state all --search {discoveries.MARKER_FMT.format(fp=fp)} --limit 100 --json *": json.dumps(two)})
    rep = discoveries.reconcile_pending(ctx, Gh(ctx, REPO, run=r))
    assert rep[0]["status"] == "pending-ambiguous"
    out = discoveries.file_all(ctx, Gh(ctx, REPO, run=r), [d], "https://s")
    assert out[0]["status"] == "skipped-index" and not any("issue create" in c for c in r.calls)


def test_cap_five_per_issue(ctx):
    setup(ctx)
    ds = [disc(sym=f"symptom number {'x' * i}") for i in range(7)]
    r = FakeRunner({"issue list *": "[]", "issue create *": "https://github.com/SShadowS/al-sem/issues/60\n"})
    out = discoveries.file_all(ctx, Gh(ctx, REPO, run=r), ds, "https://s")
    assert [o["status"] for o in out].count("filed") == 5 and out[-1]["status"] == "over-cap"
