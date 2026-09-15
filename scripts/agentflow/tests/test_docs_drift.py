"""The two documents of record must not name strings the executor does not have.

WHY THIS FILE EXISTS. Three hardening rounds have now each ended with a doc
sentence that had gone false: post-merge described as touching nothing in the
shared checkout while it checks out, fetches and fast-forwards there; the dirt
"left exactly as found" while a `finally` deleted the tree holding it; an
eligibility parenthetical pointing at `manual-only`/`epic`/`meta` instead of the
incident labels. Every one was found by a human re-reading prose, and the suite
was green through all of them. Nothing executable tied either document to the
code.

WHAT THIS CAN AND CANNOT CATCH, stated so it is not read as more than it is. It
catches STRING DRIFT: a label renamed, a halt reason renamed, a payload field
renamed, with the docs left behind. It cannot check a sentence -- "the tree is
retained" is a claim about control flow, and the substitute for that is still a
human reading `cli.py` with the doc open. This pins the half that a rename
breaks silently, which is the half no reviewer reliably re-checks.

DIRECTION MATTERS: each test reads the DOC and asks the CODE, never the reverse.
A test that grepped the code for doc-shaped strings would pass a doc that had
said nothing at all.
"""
import dataclasses
import re
from pathlib import Path

import pytest

from agentflow import cli, eligibility, recovery

REPO = Path(__file__).resolve().parents[3]
ORCHESTRATE = REPO / ".claude" / "commands" / "orchestrate.md"
SPEC = REPO / "docs" / "superpowers" / "specs" / "2026-09-13-issue-orchestrator-design.md"
DOCS = (ORCHESTRATE, SPEC)


def _text(p: Path) -> str:
    # BYTES, then decode: this checkout is `core.autocrlf` and both documents
    # are CRLF in the working tree. Reading in text mode is fine for matching
    # but the explicit decode keeps the failure message honest about what was
    # read.
    return p.read_bytes().decode("utf-8")


@pytest.mark.parametrize("doc", DOCS, ids=lambda p: p.name)
def test_docs_name_only_labels_the_executor_defines(doc):
    """Every `agent-…` label a document names must be one the executor has.

    `eligibility.EXCLUDE_LABELS` decides which issues the loop skips forever
    and `cli.LABELS` is what `ensure_labels` creates on the repo, so a rename
    on either side that misses these two files leaves a human reading an
    instruction about a label that no longer exists.

    `[agent-discovery]` is EXCLUDED BY SHAPE, and the shape is the reason: it
    is a discovery issue's TITLE prefix (`discoveries.py`'s `title`), never a
    label, and it appears only inside square brackets. The lookbehind is what
    keeps that distinction rather than a hand-maintained allow-list that would
    quietly swallow a real miss.
    """
    named = set(re.findall(r"(?<!\[)\bagent-[a-z-]+[a-z]", _text(doc)))
    known = set(cli.LABELS) | set(eligibility.EXCLUDE_LABELS)
    assert named, f"{doc.name} names no agent label at all -- the regex has stopped matching"
    assert named <= known, f"{doc.name} names labels the executor does not define: {sorted(named - known)}"


@pytest.mark.parametrize("doc", DOCS, ids=lambda p: p.name)
def test_docs_name_only_halt_fields_the_payload_carries(doc):
    """`halt.<field>` in a document must be a real `HaltOutcome` field.

    The conductor BRANCHES on this payload (step 9.1 prints what `halt`
    carries), so a field named in prose and absent from the dataclass is an
    instruction that reads fine and does nothing. `asdict(HaltOutcome)` is
    exactly what reaches the JSON, so the dataclass is the authority.
    """
    named = set(re.findall(r"halt\.([a-z_]+)", _text(doc)))
    fields = {f.name for f in dataclasses.fields(recovery.HaltOutcome)}
    # `named <= fields` passes trivially when `named` is EMPTY, so a regex that
    # stopped matching -- or a doc that stopped naming any halt field -- would
    # read as a pass. The sibling label test above carries this same guard for
    # the same reason; this one was missing it.
    assert named, f"{doc.name} names no halt field at all -- the regex has stopped matching"
    assert named <= fields, f"{doc.name} names halt fields that do not exist: {sorted(named - fields)}"


@pytest.mark.parametrize("doc", DOCS, ids=lambda p: p.name)
def test_docs_spell_both_halt_reason_codes_as_the_code_does(doc):
    """Both halt reason codes must appear VERBATIM in both documents.

    Not "a reason code is mentioned" -- the two specific strings, taken from
    the constants. These are the machine-readable codes a human triaging a
    HALT matches against `.agent/incidents.json`, and they are the one part of
    the three-stop taxonomy that a reader can act on mechanically. Driving the
    assertion from `recovery.TREE_DIRTY` / `recovery.GATE_TIMED_OUT` rather
    than from literals is the point: rename either constant and this fails
    until the documents follow.

    MATCHED AS A TOKEN, not with `in`, and the difference is not cosmetic: the
    first version of this test used `code in text` and a break that renamed
    `TREE_DIRTY` to `"tree-dirty"` came back GREEN, because the documents still
    contained `tree-dirty-after-gates` and the shorter string is a substring of
    it. A substring check cannot see a rename to any PREFIX of the old value.
    The boundaries reject a `-` or a word character on either side, so
    `tree-dirty` no longer matches inside `tree-dirty-after-gates`.
    """
    text = _text(doc)
    for code in (recovery.TREE_DIRTY, recovery.GATE_TIMED_OUT):
        token = re.compile(rf"(?<![\w-]){re.escape(code)}(?![\w-])")
        assert token.search(text), f"{doc.name} does not spell the halt reason {code!r}"


def test_the_incident_vocabulary_and_the_exclusion_list_are_the_same_strings():
    """`recovery.INCIDENT_LABELS` must be a subset of `eligibility.EXCLUDE_LABELS`.

    NOT A DOC TEST, and it is here because trying to prove the doc test above
    could fail is what found the hole. Renaming `recovery.UNVERIFIED` and
    running the doc test came back GREEN: `cli.LABELS` splices
    `INCIDENT_LABELS`, so it followed the rename, but `EXCLUDE_LABELS` spells
    all ten labels as LITERALS, so the union the doc test asks still contained
    the old string. Two independent spellings of one vocabulary, and nothing
    compared them.

    The CHANGELOG states the invariant this pins -- "all three new names are in
    `eligibility.EXCLUDE_LABELS`, so a reopened issue carrying one is never
    re-picked by the loop" -- and it was carried by nothing executable. Break
    it and the loop re-picks an issue a human reopened while still carrying
    `agent-revert-blocked`, which means MASTER MAY STILL BE RED: the exact
    fail-closed property the spec's eligibility bullet promises.

    Subset, not equality: `EXCLUDE_LABELS` also carries `agent-working`,
    `manual-only`, `epic` and `meta`, which have nothing to do with incidents.
    """
    missing = set(recovery.INCIDENT_LABELS) - set(eligibility.EXCLUDE_LABELS)
    assert not missing, f"incident labels the loop would re-pick: {sorted(missing)}"
