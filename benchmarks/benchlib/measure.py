"""Measurement: run a command a number of times, time it, and collect output info.

Only **wall-clock time** is measured (`time.perf_counter`). It is the one number
users actually feel, and it is consistent across platforms; peak memory would
need process accounting that the standard library does not expose portably
(`getrusage` on POSIX is cumulative, Windows needs `GetProcessMemoryInfo`), and
adding it would make the suite "only runs on one OS". If you need memory data,
pair the suite with `time -v` or a process monitor -- this suite reports time
and outputs only.

The output directory is **cleared before every run**: otherwise a later run can
just pick up the previous run's artifacts and measure "the compiler doing
nothing".
"""

from __future__ import annotations

import hashlib
import os
import shutil
import statistics
import subprocess
import time
from dataclasses import dataclass, field
from pathlib import Path

from .config import Compiler, Config, ConfigError
from .fixture import FixturePaths


@dataclass
class Product:
    """One output file."""

    path: Path
    size: int
    digest: str = ""


@dataclass
class Run:
    """A single run."""

    wall: float
    exit_code: int
    ok: bool
    products: list[Product] = field(default_factory=list)
    stdout_tail: str = ""
    stderr_tail: str = ""

    @property
    def total_bytes(self) -> int:
        return sum(p.size for p in self.products)


@dataclass
class Result:
    """All runs of one (compiler x fixture) combination."""

    compiler: str
    fixture: str
    runs: list[Run]
    warmups: int

    @property
    def ok(self) -> bool:
        return bool(self.runs) and all(r.ok for r in self.runs)

    def _walls(self) -> list[float]:
        return [r.wall for r in self.runs]

    @property
    def median(self) -> float:
        return statistics.median(self._walls()) if self.runs else float("nan")

    @property
    def best(self) -> float:
        return min(self._walls()) if self.runs else float("nan")

    @property
    def worst(self) -> float:
        return max(self._walls()) if self.runs else float("nan")

    @property
    def spread(self) -> float:
        """Range (slowest - fastest), a quick read on how stable the machine is."""
        return (self.worst - self.best) if self.runs else float("nan")

    @property
    def mean(self) -> float:
        return statistics.fmean(self._walls()) if self.runs else float("nan")

    @property
    def stdev(self) -> float:
        return statistics.stdev(self._walls()) if len(self.runs) > 1 else 0.0

    @property
    def bytes_per_run(self) -> int:
        if not self.runs:
            return 0
        return int(statistics.median([r.total_bytes for r in self.runs]))

    @property
    def digest(self) -> str:
        """Fingerprint of the outputs: a hash over every (size, content hash) pair.

        This only answers "do repeated runs of the same compiler produce the same
        files", and is **not** a comparison yardstick against another compiler --
        two different compilers will never be byte-identical even when they are
        semantically equivalent.
        """
        parts = []
        for run in self.runs:
            for product in sorted(run.products, key=lambda p: p.path.name):
                parts.append(f"{product.path.name}:{product.size}:{product.digest}")
            break
        return hashlib.sha256("\n".join(parts).encode()).hexdigest()[:16] if parts else ""


def _collect(out_dir: Path, expect: list[str], hash_outputs: bool) -> list[Product]:
    """Collect outputs from the output directory, matching the `expect` globs."""
    products: dict[str, Product] = {}
    for pattern in expect:
        for path in sorted(out_dir.rglob(pattern)):
            if not path.is_file():
                continue
            digest = ""
            if hash_outputs:
                digest = hashlib.sha256(path.read_bytes()).hexdigest()[:16]
            products[str(path)] = Product(path=path, size=path.stat().st_size, digest=digest)
    return list(products.values())


def _tail(text: str, lines: int = 12) -> str:
    return "\n".join(text.splitlines()[-lines:])


def run_once(
    config: Config,
    compiler: Compiler,
    paths: FixturePaths,
    out_dir: Path,
    *,
    hash_outputs: bool,
) -> Run:
    """Run the compiler once."""
    if out_dir.exists():
        shutil.rmtree(out_dir)
    out_dir.mkdir(parents=True, exist_ok=True)

    extra = {
        "qc": str(paths.qc),
        "fixture": str(paths.dir),
        "out": str(out_dir),
    }
    argv = config.command_line(compiler, extra)
    if not argv or not argv[0]:
        raise ConfigError(
            f"compiler {compiler.id!r} expands to an empty command: {compiler.command}"
        )
    if compiler.bin and not Path(argv[0]).exists():
        raise ConfigError(
            f"the executable for compiler {compiler.id!r} does not exist: {argv[0]}\n"
            "  Fix `bin` in config.toml, or build the project first "
            "(for example `cargo build --release`)."
        )

    cwd = Path(config.expand(compiler.cwd, extra))
    if not cwd.is_dir():
        raise ConfigError(f"the cwd for compiler {compiler.id!r} does not exist: {cwd}")

    env = dict(os.environ)
    for key, value in compiler.env.items():
        env[key] = config.expand(value, extra)

    started = time.perf_counter()
    try:
        proc = subprocess.run(
            argv,
            capture_output=True,
            text=True,
            encoding="utf-8",
            errors="replace",
            cwd=str(cwd),
            env=env,
            timeout=config.general.timeout,
        )
        wall = time.perf_counter() - started
    except subprocess.TimeoutExpired:
        wall = time.perf_counter() - started
        return Run(
            wall=wall,
            exit_code=-1,
            ok=False,
            stdout_tail="",
            stderr_tail=f"timed out (> {config.general.timeout:.0f} s)",
        )
    except OSError as exc:
        raise ConfigError(f"could not start {compiler.id!r}: {exc}") from None

    products = _collect(out_dir, config.expand_list(compiler.expect, extra), hash_outputs)
    ok = proc.returncode == 0
    stderr_tail = _tail(proc.stderr)
    if ok and not products:
        ok = False
        stderr_tail = (
            (stderr_tail + "\n" if stderr_tail else "")
            + f"exit code was 0, but no output matched {compiler.expect} -- "
            "the search directory or the expected file name is probably wrong."
        )
    return Run(
        wall=wall,
        exit_code=proc.returncode,
        ok=ok,
        products=products,
        stdout_tail=_tail(proc.stdout),
        stderr_tail=stderr_tail,
    )


def measure(
    config: Config,
    compiler: Compiler,
    paths: FixturePaths,
    *,
    repeat: int,
    warmup: int,
    out_dir: Path,
    on_run=None,
) -> Result:
    """Run `warmup` discarded runs, then `repeat` measured runs."""
    for i in range(warmup):
        if on_run:
            on_run(i + 1, warmup, None)
        run_once(config, compiler, paths, out_dir, hash_outputs=False)

    runs: list[Run] = []
    for i in range(repeat):
        run = run_once(config, compiler, paths, out_dir, hash_outputs=config.general.hash_outputs)
        runs.append(run)
        if on_run:
            on_run(i + 1, repeat, run)
        if not run.ok:
            # Failing immediately means the rest would just repeat the same error,
            # so stop here instead of flooding the output.
            break
    return Result(compiler=compiler.id, fixture=paths.fixture.name, runs=runs, warmups=warmup)
