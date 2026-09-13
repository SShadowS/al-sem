from agentflow.eligibility import eligible, has_acceptance, oldest, parse_dependencies, truncate_body
from agentflow.gh import Issue


def mk(n, body="## Acceptance\nok", labels=(), author="SShadowS", created="2026-09-01T00:00:00Z", title="t"):
    return Issue(n, title, body, frozenset(labels), author, created)


def test_parse_dependencies_from_line_and_section():
    body = "Depends-on: #3, #4\n\n## Dependencies\n- needs #7 first\n\n## Other\n#9 unrelated"
    assert parse_dependencies(body) == {3, 4, 7}


def test_acceptance_heading_required_unless_question():
    assert has_acceptance("## Acceptance\nx")
    assert has_acceptance("### acceptance criteria")
    assert not has_acceptance("## Accept\nx")
    ok, ex = eligible([mk(1, body="no section"), mk(2, body="?", labels={"question"})], {"SShadowS"})
    assert [i.number for i in ok] == [2] and ex[0].reason == "no-acceptance"


def test_author_and_labels_exclude():
    ok, ex = eligible([mk(1, author="stranger"), mk(2, labels={"agent-blocked"}), mk(3, labels={"epic"})], {"SShadowS"})
    assert ok == [] and {e.reason for e in ex} == {"author", "label:agent-blocked", "label:epic"}


def test_open_dependency_excludes_but_closed_one_does_not():
    ok, ex = eligible([mk(1, body="## Acceptance\nx\nDepends-on: #2"), mk(2), mk(3, body="## Acceptance\nDepends-on: #99")],
                      {"SShadowS"})
    assert [i.number for i in ok] == [2, 3]
    assert ex[0].issue.number == 1 and ex[0].reason == "depends-on-open:#2"


def test_dependency_cycle_excludes_all_members():
    a = mk(1, body="## Acceptance\nDepends-on: #2")
    b = mk(2, body="## Acceptance\nDepends-on: #1")
    ok, ex = eligible([a, b, mk(3)], {"SShadowS"})
    assert [i.number for i in ok] == [3]
    assert {e.reason for e in ex} == {"dependency-cycle"}


def test_oldest_and_truncate():
    xs = [mk(1, created="2026-09-03T00:00:00Z"), mk(2, created="2026-09-01T00:00:00Z"), mk(3, created="2026-09-02T00:00:00Z")]
    assert [i.number for i in oldest(xs, 2)] == [2, 3]
    assert truncate_body("a" * 9000).endswith("…[truncated]") and len(truncate_body("a" * 9000)) < 8100
