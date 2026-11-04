#!/usr/bin/env python3
"""mdlc benchmark suite -- entry point.

Usage:

```bash
python benchmarks/bench.py                    # run everything in benchmarks/config.toml
python benchmarks/bench.py --list             # show what the config contains
python benchmarks/bench.py -f dmx-112 -c mdlc
python benchmarks/bench.py --repeat 7 --json out.json
```

Design choices:

- **Standard library only** (`subprocess` / `statistics` / `tomllib`, 3.11+).
  A machine used for benchmarking should not need a dependency install before it
  can measure one number.
- **Fixtures are generated on demand** and cached: the parameters are part of the
  directory name, so the second run reuses them.
- **The command line is entirely configuration-driven**: to measure your own
  compiler, add one template to `[[compilers]]` -- no code changes.
"""

from __future__ import annotations

import argparse
import sys
import time
from pathlib import Path

try:
    from benchlib import config as config_mod
    from benchlib import fixture as fixture_mod
    from benchlib import measure as measure_mod
    from benchlib import report as report_mod
except ImportError:  # invoked from some other directory
    sys.path.insert(0, str(Path(__file__).resolve().parent))
    from benchlib import config as config_mod
    from benchlib import fixture as fixture_mod
    from benchlib import measure as measure_mod
    from benchlib import report as report_mod


def _split(value: str) -> list[str]:
    return [part.strip() for part in value.split(",") if part.strip()]


def parse_args(argv: list[str]) -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        prog="bench.py",
        description=(
            "mdlc benchmark suite: compare the compile speed of different "
            "compilers on the same set of generated fixtures."
        ),
        formatter_class=argparse.RawDescriptionHelpFormatter,
        epilog="""examples:
  python benchmarks/bench.py --list
  python benchmarks/bench.py -f dmx-112 -c mdlc --repeat 3
  python benchmarks/bench.py --no-generate --json results.json
""",
    )
    parser.add_argument(
        "-c", "--config", default=None,
        help="configuration file (default: benchmarks/config.toml)",
    )
    parser.add_argument(
        "-f", "--fixtures", default="all",
        help="only run these fixtures (comma separated; default: all)",
    )
    parser.add_argument(
        "--compilers", default="all",
        help="only run these compilers (comma separated; default: all)",
    )
    parser.add_argument("--repeat", type=int, default=None, help="override [general] repeat")
    parser.add_argument("--warmup", type=int, default=None, help="override [general] warmup")
    parser.add_argument(
        "--no-generate", action="store_true",
        help="do not run Blender; the fixtures must already exist",
    )
    parser.add_argument("--json", default=None, help="write the results to this JSON file")
    parser.add_argument("--csv", default=None, help="write the results to this CSV file")
    parser.add_argument("--list", action="store_true", help="list the fixtures and compilers, then exit")
    parser.add_argument(
        "--keep", action="store_true",
        help="keep the output directories (the default; only meaningful together with --clean)",
    )
    parser.add_argument(
        "--clean", action="store_true",
        help="delete this run's output directories afterwards",
    )
    return parser.parse_args(argv)


def main(argv: list[str]) -> int:
    args = parse_args(argv)

    config_path = Path(args.config) if args.config else config_mod.BENCH_DIR / "config.toml"
    try:
        config = config_mod.load(config_path)
    except config_mod.ConfigError as exc:
        print(f"configuration error: {exc}", file=sys.stderr)
        return 2

    if args.repeat is not None:
        config.general.repeat = max(1, args.repeat)
    if args.warmup is not None:
        config.general.warmup = max(0, args.warmup)

    if args.list:
        print(f"config: {config.source}")
        print(f"\nfixtures ({len(config.fixtures)}):")
        for fixture in config.fixtures:
            print(f"  {fixture.name:<14} {fixture.describe()}")
        print(f"\ncompilers ({len(config.compilers)}):")
        for compiler in config.compilers:
            print(f"  {compiler.id:<14} {' '.join(compiler.command)}")
        print(f"\nwork directory: {config.general.work_dir}")
        return 0

    # -- pick the combinations to run -------------------------------------

    if args.fixtures == "all":
        fixtures = list(config.fixtures)
    else:
        wanted = _split(args.fixtures)
        unknown = [w for w in wanted if w not in {f.name for f in config.fixtures}]
        if unknown:
            print(
                f"unknown fixture(s): {', '.join(unknown)} (available: {config.fixture_names()})",
                file=sys.stderr,
            )
            return 2
        fixtures = [config.fixture(name) for name in wanted]

    if args.compilers == "all":
        compilers = list(config.compilers)
    else:
        wanted = _split(args.compilers)
        unknown = [w for w in wanted if w not in {c.id for c in config.compilers}]
        if unknown:
            print(
                f"unknown compiler(s): {', '.join(unknown)} (available: {config.compiler_ids()})",
                file=sys.stderr,
            )
            return 2
        compilers = [config.compiler(ident) for ident in wanted]

    # -- preflight: the compiler binaries should exist before anything runs --
    #
    # Otherwise the suite spends minutes generating fixtures only to discover a
    # typo in a path.

    problems: list[str] = []
    for compiler in compilers:
        if not compiler.bin:
            continue
        try:
            expanded = config.executable(
                compiler.bin, {"qc": "", "fixture": "", "out": ""}
            )
        except config_mod.ConfigError as exc:
            problems.append(f"{compiler.id}: {exc}")
            continue
        if not Path(expanded).exists():
            problems.append(f"{compiler.id}: executable does not exist -> {expanded}")
    if problems:
        print("preflight failed:", file=sys.stderr)
        for problem in problems:
            print(f"  - {problem}", file=sys.stderr)
        print("  (fix `bin` in config.toml, or build the binary first)", file=sys.stderr)
        return 2

    if not args.no_generate:
        try:
            blender = config.blender.resolve()
            print(f"Blender: {blender}")
        except config_mod.ConfigError as exc:
            print(f"configuration error: {exc}", file=sys.stderr)
            return 2

    print(f"config: {config.source}")
    print(f"work directory: {config.general.work_dir}")
    print(
        f"combinations: {len(fixtures)} fixture(s) x {len(compilers)} compiler(s), "
        f"{config.general.repeat} run(s) each (+{config.general.warmup} warmup)"
    )

    # -- generate the fixtures --------------------------------------------

    paths_by_name = {}
    for fixture in fixtures:
        try:
            paths_by_name[fixture.name] = fixture_mod.ensure(
                config, fixture, generate=not args.no_generate
            )
        except config_mod.ConfigError as exc:
            print(f"\nfixture {fixture.name!r} is unavailable: {exc}", file=sys.stderr)
            return 2

    # -- measure ----------------------------------------------------------

    results: list[measure_mod.Result] = []
    started_all = time.perf_counter()
    for fixture in fixtures:
        paths = paths_by_name[fixture.name]
        for compiler in compilers:
            out_dir = config.general.work_dir / "out" / fixture.name / compiler.id
            print(f"\n> {fixture.name} x {compiler.id}", end="", flush=True)

            def on_run(index: int, total: int, run) -> None:
                if run is None:
                    print(f"\n  warmup {index}/{total} ...", end="", flush=True)
                    return
                marker = "" if run.ok else " x"
                print(f"\n  {index}/{total} {run.wall:.3f}s{marker}", end="", flush=True)

            result = measure_mod.measure(
                config,
                compiler,
                paths,
                repeat=config.general.repeat,
                warmup=config.general.warmup,
                out_dir=out_dir,
                on_run=on_run,
            )
            print()
            results.append(result)
            if not result.ok:
                print(f"  x {compiler.id} failed on {fixture.name}", file=sys.stderr)
                if result.runs and result.runs[0].stderr_tail:
                    for line in result.runs[0].stderr_tail.splitlines():
                        print(f"    {line}", file=sys.stderr)

            if args.clean and out_dir.exists():
                import shutil

                shutil.rmtree(out_dir, ignore_errors=True)

    elapsed = time.perf_counter() - started_all
    print(report_mod.render(config, fixtures, results))
    print(f"total {elapsed:.1f}s")

    if args.json:
        report_mod.write_json(Path(args.json), config, fixtures, results)
        print(f"wrote {args.json}")
    if args.csv:
        report_mod.write_csv(Path(args.csv), fixtures, results)
        print(f"wrote {args.csv}")

    failed = [r for r in results if not r.ok]
    if failed:
        names = ", ".join(f"{r.compiler}@{r.fixture}" for r in failed)
        print(f"\n{len(failed)} combination(s) failed: {names}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
