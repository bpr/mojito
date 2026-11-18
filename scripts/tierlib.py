"""Helpers shared by the test-tier scripts, `scripts/smoke` and `scripts/focused`.

Both build the CLI under the LLVM environment `scripts/check-pliron` uses,
run `target/debug/mojito` on fixtures and root test targets' executables, and
report failures in the same `FAIL <kind> <id>` shape that
`scripts/claude_loop.py` parses.
"""

from __future__ import annotations

import json
import os
import subprocess
import tomllib
from dataclasses import dataclass
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
BIN = ROOT / "target" / "debug" / "mojito"
TIMEOUT = 300


@dataclass
class Outcome:
    exit_code: int
    output: str


def mojito(argv: list[str], env: dict[str, str], stdin: bytes | None = None) -> Outcome:
    """Run the built CLI from the repository root; `stdin` defaults to none."""
    return run([str(BIN), *argv], env, stdin)


def run(argv: list[str], env: dict[str, str], stdin: bytes | None = None,
        extra_env: dict[str, str] | None = None) -> Outcome:
    """Run `argv` from the repository root with a timeout, merging its streams."""
    if extra_env:
        env = {**env, **extra_env}
    try:
        done = subprocess.run(argv, cwd=ROOT, env=env, capture_output=True, timeout=TIMEOUT,
                              input=stdin, stdin=None if stdin is not None else subprocess.DEVNULL)
    except subprocess.TimeoutExpired:
        return Outcome(124, f"timed out after {TIMEOUT}s\n")
    text = (done.stdout + done.stderr).decode("utf-8", errors="replace")
    return Outcome(done.returncode, text)


def build(steps: list[list[str]], env: dict[str, str], tool: str) -> bool:
    """Run each cargo step in order; report the first that fails."""
    for step in steps:
        done = subprocess.run(step, cwd=ROOT, env=env, capture_output=True, text=True)
        if done.returncode != 0:
            print(f"{tool}: `{' '.join(step)}` failed:\n{tail(done.stderr, 40)}")
            return False
    return True


def native_env() -> dict[str, str]:
    """The environment `scripts/check-pliron` builds and runs under."""
    env = dict(os.environ)
    prefix = llvm_prefix()
    env["LLVM_SYS_231_PREFIX"] = prefix
    env["PATH"] = f"{prefix}/bin:{env.get('PATH', '')}"
    env.setdefault("MOJITO_RUNTIME_LIB", str(ROOT / "target" / "debug" / "libmojito_runtime.a"))
    return env


def llvm_prefix() -> str:
    """The LLVM 23 install `scripts/llvm-prefix` finds, or the configured one when none is."""
    found = subprocess.run([str(ROOT / "scripts" / "llvm-prefix")], capture_output=True, text=True)
    return found.stdout.strip() or os.environ.get("LLVM_SYS_231_PREFIX", "/usr/lib/llvm-23")


def test_env() -> dict[str, str]:
    """`native_env` plus the `[env]` table of `.cargo/config.toml` and the
    root crate's `CARGO_MANIFEST_DIR`.

    Cargo sets both only for the processes it launches, and the script runs
    the test executables itself, so their trial threads would otherwise get
    Rust's default stack, and an `expect_file!` snapshot test could not
    resolve its path.
    """
    env = native_env()
    env.setdefault("CARGO_MANIFEST_DIR", str(ROOT))
    config = tomllib.loads((ROOT / ".cargo" / "config.toml").read_text())
    for key, value in config.get("env", {}).items():
        env.setdefault(key, value if isinstance(value, str) else value["value"])
    return env


def build_test_targets(targets: list[str], vm_only: bool, env: dict[str, str],
                       tool: str) -> dict[str, str] | None:
    """Build the test targets (and with them the CLI), returning each one's executable."""
    features = [] if vm_only else ["--features", "backend-pliron"]
    if not vm_only and not build(
            [["cargo", "build", "--quiet", "-p", "mojito-runtime"]], env, tool):
        return None
    command = ["cargo", "test", "--no-run", "--quiet", "--message-format=json", *features]
    for target in targets:
        command += ["--test", target]
    done = run(command, env)
    if done.exit_code != 0:
        print(f"{tool}: `{' '.join(command)}` failed:\n{tail(done.output, 40)}")
        return None
    executables = {}
    for line in done.output.splitlines():
        if not line.startswith("{"):
            continue
        message = json.loads(line)
        target = message.get("target", {})
        if (message.get("reason") == "compiler-artifact" and target.get("kind") == ["test"]
                and message.get("profile", {}).get("test") and message.get("executable")):
            executables[target["name"]] = message["executable"]
    return executables


def built_test_executables(targets: list[str]) -> dict[str, str]:
    """The newest built executable of each test target, for `--no-build`."""
    deps = ROOT / "target" / "debug" / "deps"
    executables = {}
    for target in targets:
        candidates = [p for p in deps.glob(f"{target}-*")
                      if p.suffix == "" and p.is_file() and p.stat().st_mode & 0o111]
        if candidates:
            executables[target] = str(max(candidates, key=lambda p: p.stat().st_mtime))
    return executables


def first_difference(expected: str, actual: str) -> str:
    want, got = expected.splitlines(), actual.splitlines()
    for line, (a, b) in enumerate(zip(want, got), 1):
        if a != b:
            return f"line {line}: expected `{a}`, got `{b}`"
    if len(want) != len(got):
        return f"expected {len(want)} lines, got {len(got)}"
    return "the outputs differ in trailing whitespace"


def tail(text: str, lines: int = 8) -> str:
    return "\n".join(text.rstrip().splitlines()[-lines:])


def indent(text: str) -> str:
    return "\n".join(f"    {line}" for line in text.splitlines())
