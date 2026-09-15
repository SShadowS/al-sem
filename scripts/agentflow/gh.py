"""Thin `gh` wrapper. All GitHub reads and writes go through here.

Reads never touch the lock. Writes call the dry-run guard and the run-id fence
first, so a superseded run cannot label, comment, file, or merge. Every call
retries on HTTP 403/429/5xx with 1 s then 2 s backoff, then raises GhError.
"""
from __future__ import annotations

import json
import re
import subprocess
import tempfile
import time
from dataclasses import dataclass
from pathlib import Path
from typing import Any, Callable

from . import lock
from .state import Ctx

RETRY_MARKERS = ("HTTP 403", "HTTP 429", "HTTP 500", "HTTP 502", "HTTP 503", "HTTP 504")
ISSUE_FIELDS = "number,title,body,labels,author,createdAt"


class GhError(RuntimeError):
    pass


@dataclass(frozen=True)
class Issue:
    number: int
    title: str
    body: str
    labels: frozenset[str]
    author: str
    created_at: str


def _issue_from_api(d: dict) -> Issue:
    return Issue(
        number=d["number"], title=d["title"], body=d.get("body") or "",
        labels=frozenset(l["name"] for l in d.get("labels", [])),
        author=d["user"]["login"], created_at=d["created_at"],
    )


def _issue_from_cli(d: dict) -> Issue:
    return Issue(
        number=d["number"], title=d["title"], body=d.get("body") or "",
        labels=frozenset(l["name"] for l in d.get("labels", [])),
        author=d["author"]["login"], created_at=d["createdAt"],
    )


class Gh:
    def __init__(self, ctx: Ctx, repo: str, run: Callable = subprocess.run, sleep: Callable = time.sleep):
        self.ctx, self.repo, self.run, self.sleep = ctx, repo, run, sleep

    # ---- transport -------------------------------------------------------
    def _raw(self, args: list[str], mutating: bool = False) -> str:
        if mutating:
            self.ctx.write_guard("gh " + " ".join(args))
            lock.check_fence(self.ctx)
        last = ""
        for attempt in range(3):
            r = self.run(["gh", *args], capture_output=True, text=True)
            if r.returncode == 0:
                return r.stdout
            last = r.stderr or r.stdout
            if attempt < 2 and any(m in last for m in RETRY_MARKERS):
                self.sleep(2 ** attempt)
                continue
            break
        raise GhError(last.strip() or f"gh {' '.join(args)} failed")

    def _api(self, path: str, *, paginate: bool = False, method: str | None = None, fields: dict | None = None) -> Any:
        args = ["api", path]
        if paginate:
            args += ["--paginate", "--slurp"]
        if method:
            args += ["-X", method]
        for k, v in (fields or {}).items():
            args += ["-f", f"{k}={v}"]
        out = self._raw(args, mutating=method in ("POST", "PATCH", "DELETE"))
        data = json.loads(out) if out.strip() else None
        if paginate:
            return [item for page in data for item in page]
        return data

    # ---- reads -----------------------------------------------------------
    def auth_ok(self) -> bool:
        try:
            self._raw(["auth", "status"])
            return True
        except GhError:
            return False

    def list_open_issues(self) -> list[Issue]:
        items = self._api(f"repos/{self.repo}/issues?state=open&per_page=100", paginate=True)
        return [_issue_from_api(d) for d in items if "pull_request" not in d]

    def collaborators(self) -> set[str]:
        items = self._api(f"repos/{self.repo}/collaborators?permission=push&per_page=100", paginate=True)
        return {d["login"] for d in items}

    def issue(self, n: int) -> Issue:
        return _issue_from_api(self._api(f"repos/{self.repo}/issues/{n}"))

    def search_issues(self, query: str) -> list[Issue]:
        out = self._raw(["issue", "list", "--repo", self.repo, "--state", "all", "--search", query,
                         "--limit", "100", "--json", ISSUE_FIELDS])
        return [_issue_from_cli(d) for d in json.loads(out or "[]")]

    def list_labeled(self, label: str) -> list[Issue]:
        out = self._raw(["issue", "list", "--repo", self.repo, "--label", label, "--state", "all",
                         "--limit", "500", "--json", ISSUE_FIELDS])
        return [_issue_from_cli(d) for d in json.loads(out or "[]")]

    def pr_for_branch_prefix(self, prefix: str, suffix: str | None = None) -> dict | None:
        """The best-ranked PR whose `headRefName` starts with `prefix` and --
        when given -- ends with `suffix`. MERGED outranks OPEN.

        `suffix` is not a convenience. The branch names this flow mints are
        `issue/<n>-<slug>-a<attempt>`, so a PREFIX alone matches every attempt
        the issue ever had, and the MERGED-first ranking then prefers a
        previous attempt's long-merged PR over THIS attempt's open one.
        `recovery.recover_stale` turns that answer into a trusted merge record
        (see `cli.cmd_post_merge`'s provenance check), so the wrong answer here
        aims a full gate re-run -- and, if master has not moved, a REVERT --
        at a months-old commit this run established nothing about.

        The filter belongs here rather than in the caller because this method
        returns ONE PR: post-filtering a single ranked answer would silently
        drop the right PR whenever a wrong one outranked it."""
        out = self._raw(["pr", "list", "--repo", self.repo, "--state", "all", "--limit", "50",
                         "--json", "number,state,headRefName,headRefOid,mergeCommit,mergedAt"])
        matches = [pr for pr in json.loads(out or "[]")
                   if pr["headRefName"].startswith(prefix)
                   and (suffix is None or pr["headRefName"].endswith(suffix))]
        if not matches:
            return None
        rank = {"MERGED": 0, "OPEN": 1}
        return min(matches, key=lambda pr: rank.get(pr.get("state"), 2))

    def pr_checks(self, n: int) -> list[dict]:
        out = self._raw(["pr", "view", str(n), "--repo", self.repo, "--json", "statusCheckRollup"])
        return json.loads(out or "{}").get("statusCheckRollup", [])

    def pr_view(self, n: int, fields: str) -> dict:
        out = self._raw(["pr", "view", str(n), "--repo", self.repo, "--json", fields])
        return json.loads(out or "{}")

    # ---- writes ----------------------------------------------------------
    def ensure_labels(self, names: list[str]) -> None:
        existing = {d["name"] for d in self._api(f"repos/{self.repo}/labels?per_page=100", paginate=True)}
        for name in names:
            if name not in existing:
                self._api(f"repos/{self.repo}/labels", method="POST", fields={"name": name, "color": "ededed"})

    def add_labels(self, n: int, labels: list[str]) -> None:
        args = ["issue", "edit", str(n)]
        for l in labels:
            args += ["--add-label", l]
        self._raw(args, mutating=True)

    def remove_label(self, n: int, label: str) -> None:
        self._raw(["issue", "edit", str(n), "--remove-label", label], mutating=True)

    def _body_file(self, body: str) -> str:
        f = tempfile.NamedTemporaryFile("w", suffix=".md", delete=False, encoding="utf-8")
        f.write(body)
        f.close()
        return f.name

    def comment(self, n: int, body: str) -> None:
        self._raw(["issue", "comment", str(n), "--body-file", self._body_file(body)], mutating=True)

    def create_issue(self, title: str, body: str, labels: list[str]) -> int:
        args = ["issue", "create", "--title", title, "--body-file", self._body_file(body)]
        for l in labels:
            args += ["--label", l]
        out = self._raw(args, mutating=True)
        m = re.search(r"/issues/(\d+)", out)
        if not m:
            raise GhError(f"could not parse issue number from: {out!r}")
        return int(m.group(1))

    def pr_comment(self, n: int, body: str) -> None:
        self._raw(["pr", "comment", str(n), "--repo", self.repo, "--body-file", self._body_file(body)], mutating=True)

    def create_pr(self, title: str, body: str, head: str, base: str) -> int:
        out = self._raw(["pr", "create", "--title", title, "--body-file", self._body_file(body),
                         "--head", head, "--base", base], mutating=True)
        m = re.search(r"/pull/(\d+)", out)
        if not m:
            raise GhError(f"could not parse PR number from: {out!r}")
        return int(m.group(1))

    def merge_pr(self, n: int, head_sha: str) -> None:
        self._raw(["pr", "merge", str(n), "--squash", "--match-head-commit", head_sha], mutating=True)

    def reopen_issue(self, n: int) -> None:
        self._raw(["issue", "reopen", str(n)], mutating=True)
