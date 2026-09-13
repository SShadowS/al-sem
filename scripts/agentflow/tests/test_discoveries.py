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
    r = FakeRunner({"issue list *": "[]",
                    "issue create *": "https://github.com/SShadowS/al-sem/issues/50\n"})
    out = discoveries.file_all(ctx, Gh(ctx, REPO, run=r), [d], "https://s")
    assert out == [{"fp": fp, "status": "filed", "number": 50, "error": None}]
    idx = read_json(ctx.paths.discoveries_index)
    assert idx[fp] == {"status": "filed", "number": 50, "origin": 8}
    create = next(c for c in r.calls if c.startswith("issue create"))
    assert "--label agent-filed --label bug" in create and "[agent-discovery][resolve]" in create
    list_call = next(c for c in r.calls if c.startswith("issue list"))
    assert f"--repo {REPO} --label agent-filed --state all --limit 500 --json" in list_call


def test_crash_between_create_and_index_leaves_pending_and_reconcile_finds_marker(ctx):
    setup(ctx)
    d = disc()
    fp = discoveries.fingerprint(d.subsystem, d.locator, d.symptom)
    r = FakeRunner({"issue list *": "[]", "issue create *": "garbage without a number"})
    out = discoveries.file_all(ctx, Gh(ctx, REPO, run=r), [d], "https://s")
    assert out[0]["status"] == "pending"
    assert read_json(ctx.paths.discoveries_index)[fp]["status"] == "pending"
    r2 = FakeRunner({"issue list *": search_hit(fp, 51)})
    rep = discoveries.reconcile_pending(ctx, Gh(ctx, REPO, run=r2))
    assert rep == [{"fp": fp, "status": "filed", "number": 51}]
    assert read_json(ctx.paths.discoveries_index)[fp]["status"] == "filed"


def test_reconcile_zero_hits_is_pending_not_found_and_index_unchanged(ctx):
    setup(ctx)
    d = disc()
    fp = discoveries.fingerprint(d.subsystem, d.locator, d.symptom)
    discoveries._save_index(ctx, {fp: {"status": "pending", "number": None, "origin": 8}})
    r = FakeRunner({"issue list *": "[]"})
    rep = discoveries.reconcile_pending(ctx, Gh(ctx, REPO, run=r))
    assert rep == [{"fp": fp, "status": "pending-not-found"}]
    assert read_json(ctx.paths.discoveries_index)[fp] == {"status": "pending", "number": None, "origin": 8}


def test_pending_not_found_then_later_file_all_retries_and_creates(ctx):
    setup(ctx)
    d = disc()
    fp = discoveries.fingerprint(d.subsystem, d.locator, d.symptom)
    discoveries._save_index(ctx, {fp: {"status": "pending", "number": None, "origin": 8}})
    r = FakeRunner({"issue list *": "[]"})
    rep = discoveries.reconcile_pending(ctx, Gh(ctx, REPO, run=r))
    assert rep == [{"fp": fp, "status": "pending-not-found"}]
    r2 = FakeRunner({"issue list *": "[]", "issue create *": "https://github.com/SShadowS/al-sem/issues/70\n"})
    out = discoveries.file_all(ctx, Gh(ctx, REPO, run=r2), [d], "https://s")
    assert out == [{"fp": fp, "status": "filed", "number": 70, "error": None}]
    assert any(c.startswith("issue create") for c in r2.calls)


def test_charge_after_success_failed_create_leaves_budget_unchanged(ctx):
    setup(ctx)
    d = disc()
    r = FakeRunner({"issue list *": "[]", "issue create *": "garbage without a number"})
    discoveries.file_all(ctx, Gh(ctx, REPO, run=r), [d], "https://s")
    assert budget.snapshot(ctx)["counts"].get("discoveries", 0) == 0


def test_ambiguous_reconcile_stays_pending_and_never_recreates(ctx):
    setup(ctx)
    d = disc()
    fp = discoveries.fingerprint(d.subsystem, d.locator, d.symptom)
    discoveries._save_index(ctx, {fp: {"status": "pending", "number": None, "origin": 8}})
    two = json.loads(search_hit(fp, 1)) + json.loads(search_hit(fp, 2))
    r = FakeRunner({"issue list *": json.dumps(two)})
    rep = discoveries.reconcile_pending(ctx, Gh(ctx, REPO, run=r))
    assert rep[0]["status"] == "pending-ambiguous"
    assert read_json(ctx.paths.discoveries_index)[fp]["status"] == "pending"  # unchanged (M16)
    out = discoveries.file_all(ctx, Gh(ctx, REPO, run=r), [d], "https://s")
    assert out[0]["status"] == "ambiguous" and not any("issue create" in c for c in r.calls)


def test_search_failure_in_file_all_reports_search_failed_and_continues(ctx):
    setup(ctx)
    d = disc()
    fp = discoveries.fingerprint(d.subsystem, d.locator, d.symptom)
    r = FakeRunner({})
    r.responses["issue list *"] = (1, "", "HTTP 404: gone")
    out = discoveries.file_all(ctx, Gh(ctx, REPO, run=r, sleep=lambda s: None), [d], "https://s")
    assert out[0]["fp"] == fp and out[0]["status"] == "search-failed" and out[0]["number"] is None
    assert "HTTP 404" in out[0]["error"]


def test_cap_five_per_issue(ctx):
    setup(ctx)
    ds = [disc(sym=f"symptom number {'x' * i}") for i in range(7)]
    r = FakeRunner({"issue list *": "[]", "issue create *": "https://github.com/SShadowS/al-sem/issues/60\n"})
    out = discoveries.file_all(ctx, Gh(ctx, REPO, run=r), ds, "https://s")
    assert [o["status"] for o in out].count("filed") == 5 and out[-1]["status"] == "over-cap"


def test_deadline_exceeded_raises_before_any_create(ctx):
    lock.acquire(ctx, 8, "s", 1)
    budget.init(ctx, claimed_at=ctx.now() - budget.WALL_CLOCK_SECONDS - 1)
    d = disc()
    r = FakeRunner({"issue list *": "[]", "issue create *": "https://github.com/SShadowS/al-sem/issues/99\n"})
    with pytest.raises(budget.DeadlineExceeded):
        discoveries.file_all(ctx, Gh(ctx, REPO, run=r), [d], "https://s")
    assert not any(c.startswith("issue create") for c in r.calls)


def test_ambiguous_marker_hits_in_file_all_are_reported_not_resolved(ctx):
    setup(ctx)
    d = disc()
    fp = discoveries.fingerprint(d.subsystem, d.locator, d.symptom)
    two = json.loads(search_hit(fp, 1)) + json.loads(search_hit(fp, 2))
    r = FakeRunner({"issue list *": json.dumps(two)})
    out = discoveries.file_all(ctx, Gh(ctx, REPO, run=r), [d], "https://s")
    assert out == [{"fp": fp, "status": "ambiguous", "number": None, "error": "2 hits"}]
    assert fp not in read_json(ctx.paths.discoveries_index, default={})
    assert not any("issue create" in c for c in r.calls)
