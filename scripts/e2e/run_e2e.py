#!/usr/bin/env python3
"""Manual end-to-end check of fleetify against real hosts and real agents.

Not part of CI. Run it by hand after a larger change. It creates small, private, throwaway
repositories on GitHub and GitLab (named `fleetify-e2e-<run>-...`), runs fleetify against them with
the selected agents, and compares each repo's result with the expected one. It never deletes a
repository by itself; `--cleanup <run>` prints or deletes what one run created.

Requires: a built fleetify binary, `gh` and `glab` logged in as the owner, `git`, and for the
agents: `opencode` with the llama.cpp provider reachable (default), `claude` and `copilot` when
selected. Claude Code and Copilot calls use their own quota: they are off unless named.

    python3 scripts/e2e/run_e2e.py --plan                       # what would be created
    python3 scripts/e2e/run_e2e.py --agents opencode-llamacpp   # create, run, compare
    python3 scripts/e2e/run_e2e.py --cleanup <run-id> --delete  # delete one run's repos

On success (every case matched the expected result) the run's repositories are deleted again.
On a mismatch they, and the work folder, are kept so the failure can be read; `--keep` keeps them
either way.
"""

from __future__ import annotations

import argparse
import datetime as dt
import json
import os
import shutil
import subprocess
import sys
import tempfile
import urllib.request
from dataclasses import dataclass, field
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
PREFIX = "fleetify-e2e"
LLAMA_MODELS_URL = os.environ.get("FLEETIFY_E2E_LLAMA_URL", "http://192.168.178.151:8080/v1/models")
MAX_REPOS = 14

# agent name -> (fleetify agent, model or None). Models are passed through unchanged.
AGENTS = {
    "opencode-llamacpp": ("opencode", "llamacpp/qwen3.8-flash-next"),
    "copilot": ("copilot", None),
    "claude-code": ("claude-code", None),
}
PAID_OR_QUOTA = {"copilot", "claude-code"}

RUBY_FROM = "3.2"
RUBY_TO = "3.4"


@dataclass
class Host:
    name: str  # "github" or "gitlab"
    spec: str  # repos-file prefix: github: / gitlab:
    ci: str  # human name of the CI system


HOSTS = {
    "github": Host("github", "github", "GitHub Actions"),
    "gitlab": Host("gitlab", "gitlab", "GitLab CI"),
}


@dataclass
class Case:
    """One fleetify run against one fresh repository, with the result it must produce."""

    key: str
    host: str
    agent: str | None  # key of AGENTS, or None for a deterministic recipe
    scenario: str
    expected: str
    wait: bool = False
    merge: bool = False
    repo: str = ""  # filled in when the repository is created
    result: str = ""


def run(args: list[str], cwd: Path | None = None, check: bool = True, capture: bool = True) -> str:
    proc = subprocess.run(args, cwd=cwd, text=True, capture_output=capture)
    if check and proc.returncode != 0:
        raise SystemExit(f"command failed ({proc.returncode}): {' '.join(args[:4])}...\n{proc.stderr}")
    return proc.stdout


def owner_of(host: str) -> str:
    if host == "github":
        return run(["gh", "api", "user", "--jq", ".login"]).strip()
    return json.loads(run(["glab", "api", "user"]))["username"]


def plan_cases(agents: list[str], hosts: list[str]) -> list[Case]:
    cases: list[Case] = []
    for host in hosts:
        # Deterministic recipes: no agent. One repo each.
        cases.append(Case(f"{host}-plain", host, None, "plain", "pr_open"))
        cases.append(Case(f"{host}-verify-failed", host, None, "verify-failed", "verify_failed"))
        for agent in agents:
            cases.append(Case(f"{host}-{agent}", host, agent, "agent", "pr_open"))
            cases.append(Case(f"{host}-{agent}-ci", host, agent, "agent-ci", "ci_passed", wait=True))
            cases.append(
                Case(f"{host}-{agent}-merge", host, agent, "agent-merge", "merged", wait=True, merge=True)
            )
    return cases


# --- fixtures ------------------------------------------------------------------------------------


def fixture_files(host: str) -> dict[str, str]:
    """A small Ruby project. The CI file pins Ruby 3.2 and runs one command."""
    files = {"README.md": "# fleetify e2e throwaway\n\nSafe to delete.\n", ".ruby-version": f"{RUBY_FROM}\n"}
    if host == "github":
        files[".github/workflows/ci.yml"] = (
            "name: ci\non: [push, pull_request]\njobs:\n  test:\n    runs-on: ubuntu-latest\n"
            "    steps:\n      - uses: actions/checkout@v4\n      - uses: ruby/setup-ruby@v1\n"
            f'        with:\n          ruby-version: "{RUBY_FROM}"\n      - run: ruby -v\n'
        )
    else:
        files[".gitlab-ci.yml"] = (
            f"image: ruby:{RUBY_FROM}\n\ntest:\n  script:\n    - ruby -v\n"
        )
    return files


def make_repo_dir(host: str, path: Path) -> None:
    path.mkdir(parents=True)
    for name, text in fixture_files(host).items():
        target = path / name
        target.parent.mkdir(parents=True, exist_ok=True)
        target.write_text(text)
    run(["git", "init", "-q", "-b", "main"], cwd=path)
    run(["git", "add", "-A"], cwd=path)
    run(
        ["git", "-c", "user.name=fleetify-e2e", "-c", "user.email=e2e@example.invalid",
         "commit", "-q", "-m", "initial"],
        cwd=path,
    )


# --- recipes -------------------------------------------------------------------------------------

PREPARE_SH = "#!/bin/sh\nset -eu\ncd \"$1\"\necho 'written by fleetify e2e' > e2e.txt\n"
VERIFY_E2E_SH = "#!/bin/sh\nset -eu\ncd \"$1\"\n[ -f e2e.txt ] || { echo 'error: e2e.txt missing'; exit 1; }\necho ok\n"
VERIFY_FAIL_SH = "#!/bin/sh\necho 'error: always fails on purpose'\nexit 1\n"


def agent_verify_sh(host: str) -> str:
    if host == "github":
        check = f'grep -q \'ruby-version: "{RUBY_TO}"\' .github/workflows/ci.yml || {{ echo "error: ci.yml not {RUBY_TO}"; exit 1; }}'
    else:
        check = f"grep -q '^image: ruby:{RUBY_TO}$' .gitlab-ci.yml || {{ echo 'error: .gitlab-ci.yml not {RUBY_TO}'; exit 1; }}"
    return (
        "#!/bin/sh\nset -eu\ncd \"$1\"\n"
        f'[ "$(cat .ruby-version)" = "{RUBY_TO}" ] || {{ echo "error: .ruby-version not {RUBY_TO}"; exit 1; }}\n'
        f"{check}\necho 'ok: Ruby {RUBY_TO} pinned'\n"
    )


def ci_file(host: str) -> str:
    return ".github/workflows/ci.yml" if host == "github" else ".gitlab-ci.yml"


def recipe_text(name: str, host: str, agent: str | None, scenario: str, wait: bool, merge: bool,
                agent_name: str | None) -> str:
    h = HOSTS[host]
    lines = [
        "schema = 1",
        f'name = "{name}"',
        f'branch = "e2e/{name}"',
        f'pr_title = "e2e: {name}"',
        'tools = "tools"',
    ]
    if agent:
        lines.append(f'agent = "{agent}"')
    lines += ["", "[applies_to]", f'host_type = ["{h.name}"]', 'has_file = [".ruby-version"]', ""]
    if scenario == "plain" or scenario == "verify-failed":
        lines += ["[prepare]", 'argv = ["sh", "{tools}/prepare.sh", "{repo}"]', "timeout_s = 60", ""]
        verify = "verify-fail.sh" if scenario == "verify-failed" else "verify.sh"
        lines += ["[verify]", f'argv = ["sh", "{{tools}}/{verify}", "{{repo}}"]', "timeout_s = 60",
                  "baseline = false", ""]
        lines += ["[guards]", 'allow_paths = ["e2e.txt"]', "max_files_changed = 2", ""]
    else:
        lines += [
            "[[instructions]]",
            f'text = "This repository runs on Ruby {RUBY_FROM}. Update it to Ruby {RUBY_TO}: set '
            f'.ruby-version to {RUBY_TO} and the Ruby version used in the CI configuration to '
            f'{RUBY_TO}. Change nothing else."',
            "",
            "[needs]",
            'commands = ["sh .agent/tools/verify.sh"]',
            "",
            "[verify]",
            'argv = ["sh", "{tools}/verify.sh", "{repo}"]',
            "timeout_s = 60",
            "baseline = false",
            "",
            "[guards]",
            f'allow_paths = [".ruby-version", "{ci_file(host)}"]',
            "max_files_changed = 2",
            "max_lines_changed = 20",
            "",
        ]
    lines += ["[ci]", f"wait = {'true' if wait else 'false'}", "timeout_min = 20"]
    if merge:
        lines += ['merge = "when_green"']
    lines += ["", "[limits]", "agent_timeout_min = 15", ""]
    return "\n".join(lines)


def write_recipe(base: Path, name: str, host: str, case: Case) -> Path:
    directory = base / "recipes" / name
    (directory / "tools").mkdir(parents=True)
    (directory / "recipe.toml").write_text(
        recipe_text(name, host, AGENTS[case.agent][0] if case.agent else None, case.scenario,
                    case.wait, case.merge, case.agent)
    )
    if case.agent is None:
        (directory / "tools" / "prepare.sh").write_text(PREPARE_SH)
        (directory / "tools" / "verify.sh").write_text(VERIFY_E2E_SH)
        (directory / "tools" / "verify-fail.sh").write_text(VERIFY_FAIL_SH)
    else:
        (directory / "tools" / "verify.sh").write_text(agent_verify_sh(host))
    return directory


# --- hosts ---------------------------------------------------------------------------------------


def create_remote_repo(host: str, owner: str, name: str, local: Path) -> None:
    if host == "github":
        run(["gh", "repo", "create", f"{owner}/{name}", "--private", "--source", str(local), "--push",
             "--description", "fleetify e2e throwaway (safe to delete)"])
    else:
        run(["glab", "repo", "create", "--name", name, "--private", "--defaultBranch", "main",
             "--description", "fleetify e2e throwaway (safe to delete)"], cwd=local)
        url = f"https://gitlab.com/{owner}/{name}.git"
        run(["git", "remote", "add", "origin", url], cwd=local, check=False)
        run(["git", "push", "-q", "origin", "main"], cwd=local)


def delete_remote_repo(host: str, owner: str, name: str) -> None:
    if host == "github":
        run(["gh", "repo", "delete", f"{owner}/{name}", "--yes"])
    else:
        run(["glab", "repo", "delete", f"{owner}/{name}", "--yes"])


# --- main flow ----------------------------------------------------------------------------------


# Scopes the test needs. GitHub: classic token scopes (fine-grained tokens expose no scope list,
# they are reported as unchecked). GitLab: the `api` scope covers creating, pushing, pipelines,
# merge requests and deleting projects.
REQUIRED_GITHUB = {"repo", "workflow"}
DELETE_GITHUB = {"delete_repo"}
REQUIRED_GITLAB = {"api"}


def credential_token(host: str) -> str | None:
    """The first token in ~/.fleetify/credentials.toml for this host (fleetify's own token)."""
    import tomllib

    path = Path.home() / ".fleetify" / "credentials.toml"
    if not path.is_file():
        return None
    host_name = "github.com" if host == "github" else "gitlab.com"
    for rule in tomllib.loads(path.read_text()).get("credential", []):
        if rule.get("match", "").startswith(host_name + "/"):
            return rule.get("token")
    return None


def scopes_github(token: str) -> set[str] | None:
    req = urllib.request.Request("https://api.github.com/user", headers={"Authorization": f"Bearer {token}"})
    with urllib.request.urlopen(req, timeout=20) as reply:
        raw = reply.headers.get("X-OAuth-Scopes")
    if raw is None:
        return None  # fine-grained token: no scope header
    return {scope.strip() for scope in raw.split(",") if scope.strip()}


def scopes_gitlab(token: str) -> set[str]:
    req = urllib.request.Request("https://gitlab.com/api/v4/personal_access_tokens/self",
                                 headers={"PRIVATE-TOKEN": token})
    with urllib.request.urlopen(req, timeout=20) as reply:
        return set(json.loads(reply.read()).get("scopes", []))


def check_tokens(hosts: list[str], delete: bool) -> list[str]:
    """Returns problems; an empty list means the tokens have what the test needs."""
    problems: list[str] = []
    for host in hosts:
        if host == "github":
            needed = REQUIRED_GITHUB | (DELETE_GITHUB if delete else set())
            cli_scopes = gh_cli_scopes()
            for label, token in (("gh login", None), ("fleetify credential", credential_token(host))):
                try:
                    scopes = cli_scopes if token is None else scopes_github(token)
                except OSError as err:
                    problems.append(f"github {label}: cannot read scopes ({err})")
                    continue
                if scopes is None:
                    print(f"github {label}: fine-grained token, scopes not visible; check by hand: {sorted(needed)}")
                    continue
                missing = needed - scopes
                if missing:
                    problems.append(f"github {label}: missing scope(s) {sorted(missing)}")
                else:
                    print(f"github {label}: scopes ok ({', '.join(sorted(needed))})")
        else:
            token = credential_token(host)
            glab_token = glab_cli_token()
            for label, value in (("glab login", glab_token), ("fleetify credential", token)):
                if value is None:
                    problems.append(f"gitlab {label}: no token found")
                    continue
                try:
                    scopes = scopes_gitlab(value)
                except OSError as err:
                    problems.append(f"gitlab {label}: cannot read scopes ({err})")
                    continue
                missing = REQUIRED_GITLAB - scopes
                if missing:
                    problems.append(f"gitlab {label}: missing scope(s) {sorted(missing)}")
                else:
                    print(f"gitlab {label}: scopes ok ({', '.join(sorted(scopes))})")
    return problems


def gh_cli_scopes() -> set[str] | None:
    out = run(["gh", "auth", "token"]).strip()
    return scopes_github(out)


def glab_cli_token() -> str | None:
    out = run(["glab", "config", "get", "token", "--host", "gitlab.com"], check=False).strip()
    return out or None


def preflight(fleetify: Path, agents: list[str], hosts: list[str]) -> None:
    if not fleetify.is_file():
        raise SystemExit(f"fleetify binary not found: {fleetify} (build it or pass --fleetify)")
    if "github" in hosts:
        run(["gh", "auth", "status"])
    if "gitlab" in hosts:
        run(["glab", "api", "user"])
    problems = check_tokens(hosts, delete=False)
    if problems:
        raise SystemExit("token check failed:\n  " + "\n  ".join(problems))
    if "opencode-llamacpp" in agents:
        try:
            urllib.request.urlopen(LLAMA_MODELS_URL, timeout=20).read(64)
        except OSError as err:
            raise SystemExit(f"llama.cpp server not reachable at {LLAMA_MODELS_URL}: {err}")
    for name in agents:
        if name in PAID_OR_QUOTA:
            print(f"note: {name} uses its own quota for each run", file=sys.stderr)


def write_config(workdir: Path, owner: str, run_id: str) -> None:
    (workdir / "config.toml").write_text(
        "[permissions]\n"
        'commands = ["sh .agent/tools/verify.sh"]\n\n'
        "[policy]\nrequire_checked_agent = true\n"
    )
    creds = Path.home() / ".fleetify" / "credentials.toml"
    shutil.copy(creds, workdir / "credentials.toml")


def check_agents(fleetify: Path, workdir: Path, agents: list[str]) -> None:
    for name in agents:
        agent, model = AGENTS[name]
        args = [str(fleetify), "--workdir", str(workdir), "check-agent", agent]
        if model:
            args += ["--model", model]
        run(args)


def execute(args: argparse.Namespace) -> int:
    fleetify = Path(args.fleetify).resolve()
    agents = [a for a in args.agents.split(",") if a]
    hosts = [h for h in args.hosts.split(",") if h]
    for name in agents:
        if name not in AGENTS:
            raise SystemExit(f"unknown agent {name!r}; choose from {', '.join(AGENTS)}")
    if args.check_tokens:
        problems = check_tokens(hosts, delete=args.delete)
        print("\n".join(problems) if problems else "all tokens have the scopes the test needs")
        return 1 if problems else 0
    cases = plan_cases(agents, hosts)
    if len(cases) > MAX_REPOS:
        raise SystemExit(f"{len(cases)} repositories planned; the limit is {MAX_REPOS}. Select fewer.")
    run_id = args.run or dt.datetime.now().strftime("%m%d%H%M")
    print(f"run {run_id}: {len(cases)} repositories, all private, prefix {PREFIX}-{run_id}-")
    for case in cases:
        print(f"  {case.host:7} {case.key:32} expect {case.expected}")
    if args.plan:
        return 0
    preflight(fleetify, agents, hosts)
    if not args.yes:
        if input("create these repositories and run them? [y/N] ").strip().lower() != "y":
            print("stopped; nothing created")
            return 1

    workdir = Path(args.workdir) if args.workdir else Path(tempfile.mkdtemp(prefix="fleetify-e2e-"))
    workdir.mkdir(parents=True, exist_ok=True)
    owners = {host: owner_of(host) for host in hosts}
    write_config(workdir, owners.get("github", ""), run_id)
    check_agents(fleetify, workdir, agents)

    scratch = workdir / "scratch"
    for case in cases:
        name = f"{PREFIX}-{run_id}-{case.key}"
        case.repo = name
        owner = owners[case.host]
        local = scratch / name
        make_repo_dir(case.host, local)
        create_remote_repo(case.host, owner, name, local)
        recipe = write_recipe(workdir, name, case.host, case)
        repos_file = workdir / f"{name}.repos"
        repos_file.write_text(f"{HOSTS[case.host].spec}:{owner}/{name}\n")
        command = [str(fleetify), "--workdir", str(workdir), "run", str(recipe), str(repos_file),
                   "--run", f"e2e-{run_id}-{case.key}", "--plain"]
        if case.agent:
            command += ["--agent", AGENTS[case.agent][0]]
            if AGENTS[case.agent][1]:
                command += ["--model", AGENTS[case.agent][1]]
        if case.merge:
            command += ["--merge"]
        print(f"running {case.key} ...", flush=True)
        subprocess.run(command, stdout=subprocess.DEVNULL, stderr=subprocess.STDOUT)
        report = workdir / "runs" / f"e2e-{run_id}-{case.key}" / "report.json"
        try:
            data = json.loads(report.read_text())
            case.result = data["repos"][0]["result"]
        except (OSError, KeyError, IndexError, json.JSONDecodeError):
            case.result = "no-report"

    width = max(len(c.key) for c in cases)
    failed = 0
    lines = ["| case | host | expected | got | repo |", "|---|---|---|---|---|"]
    for case in cases:
        ok = case.result == case.expected
        failed += 0 if ok else 1
        mark = "" if ok else " **MISMATCH**"
        print(f"{case.key:{width}}  expected {case.expected:14} got {case.result}{mark}")
        lines.append(f"| {case.key} | {case.host} | {case.expected} | {case.result}{mark} | {case.repo} |")
    summary = workdir / "e2e-summary.md"
    summary.write_text("\n".join(lines) + "\n")
    print(f"summary: {summary}")
    if failed:
        print(f"{failed} mismatch(es): repositories and {workdir} are kept for debugging.")
        print(f"remove them later: python3 scripts/e2e/run_e2e.py --cleanup {run_id} --delete")
        return 1
    if args.keep:
        print(f"--keep: repositories kept; remove later: python3 scripts/e2e/run_e2e.py --cleanup {run_id} --delete")
        return 0
    return remove_run(run_id, cases, owners)


def remove_run(run_id: str, cases: list[Case], owners: dict[str, str]) -> int:
    """All cases matched: delete this run's repositories. Needs the delete scope on the tokens."""
    failed = []
    for case in cases:
        try:
            delete_remote_repo(case.host, owners[case.host], case.repo)
        except SystemExit as err:
            failed.append(f"{case.host}:{owners[case.host]}/{case.repo} ({err})")
    if failed:
        print("all cases matched, but these repositories could not be deleted (token scope?):")
        print("\n".join(f"  {entry}" for entry in failed))
        print(f"remove them by hand, or run: python3 scripts/e2e/run_e2e.py --cleanup {run_id} --delete")
        return 1
    print(f"all cases matched; {len(cases)} repositories removed")
    return 0


def cleanup(args: argparse.Namespace) -> int:
    run_id = args.cleanup
    names = []
    for host in ["github", "gitlab"]:
        owner = owner_of(host)
        listing = run(["gh", "repo", "list", owner, "--limit", "200", "--json", "name", "--jq",
                       ".[].name"]) if host == "github" else run(
            ["glab", "api", "projects?owned=true&search=" + f"{PREFIX}-{run_id}-", "--paginate"])
        if host == "github":
            found = [n for n in listing.split() if n.startswith(f"{PREFIX}-{run_id}-")]
            names += [("github", owner, n) for n in found]
        else:
            for project in json.loads(listing or "[]"):
                if project["name"].startswith(f"{PREFIX}-{run_id}-"):
                    names.append(("gitlab", owner, project["name"]))
    if not names:
        print(f"no repositories for run {run_id}")
        return 0
    for host, owner, name in names:
        print(f"{host}: {owner}/{name}")
    if not args.delete:
        print("nothing deleted; repeat with --delete to remove them (needs the delete scope)")
        return 0
    for host, owner, name in names:
        delete_remote_repo(host, owner, name)
        print(f"deleted {host}: {owner}/{name}")
    return 0


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--agents", default="opencode-llamacpp",
                        help="comma list of: " + ", ".join(AGENTS) + " (default: opencode-llamacpp)")
    parser.add_argument("--hosts", default="github,gitlab", help="github, gitlab or both")
    parser.add_argument("--fleetify", default=str(ROOT / "target" / "debug" / "fleetify"))
    parser.add_argument("--workdir", help="keep runs and config here (default: a temp folder)")
    parser.add_argument("--run", help="run id used in repository names (default: time based)")
    parser.add_argument("--plan", action="store_true", help="print the plan and stop")
    parser.add_argument("--yes", action="store_true", help="do not ask before creating repositories")
    parser.add_argument("--check-tokens", action="store_true",
                        help="only check that the tokens have the scopes the test needs")
    parser.add_argument("--keep", action="store_true", help="keep the repositories even when all cases match")
    parser.add_argument("--cleanup", metavar="RUN", help="list (or with --delete remove) one run's repos")
    parser.add_argument("--delete", action="store_true", help="with --cleanup: really delete")
    args = parser.parse_args()
    return cleanup(args) if args.cleanup else execute(args)


if __name__ == "__main__":
    raise SystemExit(main())
