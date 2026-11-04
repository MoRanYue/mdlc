"""Output: a terminal table plus JSON / CSV export.

The terminal table is grouped by fixture, with one block per fixture and one row
per compiler. Everything in one wide table would be unreadable: fixture scales
differ a lot (25k vs 252k triangles), so a horizontal comparison would not be
meaningful.
"""

from __future__ import annotations

import csv
import json
from pathlib import Path

from .config import Config, Fixture
from .measure import Result


def _fmt_seconds(value: float) -> str:
    if value != value:  # NaN
        return "-"
    if value >= 10.0:
        return f"{value:.2f}s"
    if value >= 1.0:
        return f"{value:.3f}s"
    return f"{value * 1000:.0f}ms"


def _fmt_bytes(value: int) -> str:
    if value <= 0:
        return "-"
    for unit, scale in (("GB", 1 << 30), ("MB", 1 << 20), ("KB", 1 << 10)):
        if value >= scale:
            return f"{value / scale:.1f}{unit}"
    return f"{value}B"


def _fmt_rate(seconds: float, triangles: int) -> str:
    if seconds != seconds or seconds <= 0 or triangles <= 0:
        return "-"
    return f"{triangles / seconds / 1e3:.0f}k t/s"


def render(
    config: Config,
    fixtures: list[Fixture],
    results: list[Result],
    *,
    colors: bool = True,
) -> str:
    """Render the results as a human-readable table."""
    lines: list[str] = []
    by_fixture: dict[str, list[Result]] = {}
    for result in results:
        by_fixture.setdefault(result.fixture, []).append(result)

    for fixture in fixtures:
        runs = by_fixture.get(fixture.name, [])
        if not runs:
            continue
        lines.append("")
        lines.append(f"-- {fixture.name}  .  {fixture.describe()}")
        header = (
            f"  {'compiler':<16}{'median':>10}{'best':>10}{'worst':>10}"
            f"{'spread':>9}{'throughput':>13}{'outputs':>10}"
        )
        lines.append(header)
        lines.append("  " + "-" * (len(header) - 2))

        fastest = min((r.median for r in runs if r.ok), default=float("nan"))
        for result in runs:
            status = "" if result.ok else "  x"
            note = (
                ""
                if result.ok
                else (f" (exit {result.runs[0].exit_code})" if result.runs else " (not run)")
            )
            if not result.ok and not result.runs:
                lines.append(f"  {result.compiler:<16}{'not run':>10}{note}")
                continue
            star = "*" if result.ok and result.median == fastest else " "
            lines.append(
                f"  {result.compiler:<15}{star}"
                f"{_fmt_seconds(result.median):>9}"
                f"{_fmt_seconds(result.best):>10}"
                f"{_fmt_seconds(result.worst):>10}"
                f"{_fmt_seconds(result.spread):>9}"
                f"{_fmt_rate(result.median, fixture.triangles):>13}"
                f"{_fmt_bytes(result.bytes_per_run):>10}"
                f"{status}{note}"
            )
            if not result.ok and result.runs and result.runs[0].stderr_tail:
                for line in result.runs[0].stderr_tail.splitlines()[-4:]:
                    lines.append(f"      {line}")

        if len(runs) > 1 and all(r.ok for r in runs):
            base = min(runs, key=lambda r: r.median)
            others = [r for r in runs if r is not base]
            parts = [
                f"{r.compiler} is {r.median / base.median:.2f}x that"
                for r in others
                if base.median > 0
            ]
            if parts:
                lines.append(f"  -> fastest: {base.compiler}; " + "; ".join(parts))

    lines.append("")
    runs = config.general.repeat
    warmup = config.general.warmup
    run_word = "run" if runs == 1 else "runs"
    if warmup:
        warm_word = "run" if warmup == 1 else "runs"
        lines.append(
            f"({runs} measured {run_word} per cell, "
            f"plus {warmup} discarded warmup {warm_word})"
        )
    else:
        lines.append(f"({runs} measured {run_word} per cell)")
    return "\n".join(lines)


def to_json(config: Config, fixtures: list[Fixture], results: list[Result]) -> dict:
    """Machine-readable results."""
    meta = {
        "config": str(config.source),
        "repeat": config.general.repeat,
        "warmup": config.general.warmup,
        "hash_outputs": config.general.hash_outputs,
    }
    rows = []
    for result in results:
        fixture = next((f for f in fixtures if f.name == result.fixture), None)
        rows.append(
            {
                "fixture": result.fixture,
                "format": fixture.format if fixture else "",
                "triangles": fixture.triangles if fixture else 0,
                "vertices": fixture.vertices if fixture else 0,
                "morphs": fixture.morphs if fixture else 0,
                "compiler": result.compiler,
                "ok": result.ok,
                "median_seconds": None if result.median != result.median else round(result.median, 6),
                "best_seconds": None if result.best != result.best else round(result.best, 6),
                "worst_seconds": None if result.worst != result.worst else round(result.worst, 6),
                "stdev_seconds": round(result.stdev, 6),
                "runs": [round(r.wall, 6) for r in result.runs],
                "exit_codes": [r.exit_code for r in result.runs],
                "product_bytes": result.bytes_per_run,
                "product_digest": result.digest,
                "errors": [r.stderr_tail for r in result.runs if not r.ok],
            }
        )
    return {"meta": meta, "results": rows}


def write_json(path: Path, config: Config, fixtures: list[Fixture], results: list[Result]) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(
        json.dumps(to_json(config, fixtures, results), indent=2, ensure_ascii=False) + "\n",
        encoding="utf-8",
        newline="\n",
    )


def write_csv(path: Path, fixtures: list[Fixture], results: list[Result]) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    with path.open("w", encoding="utf-8", newline="") as fh:
        writer = csv.writer(fh)
        writer.writerow(
            [
                "fixture",
                "format",
                "triangles",
                "vertices",
                "morphs",
                "compiler",
                "ok",
                "median_s",
                "best_s",
                "worst_s",
                "stdev_s",
                "product_bytes",
                "product_digest",
            ]
        )
        for result in results:
            fixture = next((f for f in fixtures if f.name == result.fixture), None)
            writer.writerow(
                [
                    result.fixture,
                    fixture.format if fixture else "",
                    fixture.triangles if fixture else 0,
                    fixture.vertices if fixture else 0,
                    fixture.morphs if fixture else 0,
                    result.compiler,
                    result.ok,
                    f"{result.median:.6f}" if result.median == result.median else "",
                    f"{result.best:.6f}" if result.best == result.best else "",
                    f"{result.worst:.6f}" if result.worst == result.worst else "",
                    f"{result.stdev:.6f}",
                    result.bytes_per_run,
                    result.digest,
                ]
            )
