"""Configuration: read `config.toml` into typed objects and expand command templates.

There are four blocks:

- `[general]`      repeat count, work directory, timeouts
- `[blender]`      which Blender generates the fixtures
- `[[fixtures]]`   one parameter set per fixture (format / scale / morph targets)
- `[[compilers]]`  one command template per compiler

**Command templates** are expanded with `str.format`. Available placeholders:

| Placeholder | Meaning |
|---|---|
| `{bin}` | absolute path of this compiler's `bin` field, after expansion |
| `{qc}` | the fixture's `.qc` path |
| `{fixture}` | the fixture directory (where the source files live) |
| `{out}` | output directory for this run (cleared before every run) |
| `{work}` | work root directory |
| `{bench}` | the `benchmarks/` directory |
| `{repo}` | repository root |
| `{game}` | `[game] dir`, for arguments like `-game` |

An unknown placeholder in any template is reported at **startup**, not halfway
through a run.
"""

from __future__ import annotations

import os
import shutil
import sys
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any

if sys.version_info >= (3, 11):
    import tomllib
else:  # pragma: no cover - 3.10 and older get a readable message instead
    tomllib = None  # type: ignore[assignment]


#: The `benchmarks/` directory itself (this file lives in `benchmarks/benchlib/config.py`).
BENCH_DIR = Path(__file__).resolve().parent.parent

#: Repository root (the parent of `benchmarks/`).
REPO_DIR = BENCH_DIR.parent


class ConfigError(Exception):
    """Raised for a bad configuration; the message is shown to the user verbatim."""


@dataclass
class General:
    """`[general]`."""

    work_dir: Path
    repeat: int = 5
    warmup: int = 1
    timeout: float = 1800.0
    hash_outputs: bool = False
    keep_outputs: bool = True


@dataclass
class Blender:
    """`[blender]`."""

    executable: str = ""
    timeout: float = 7200.0

    def resolve(self) -> str:
        """Locate the Blender executable.

        Order: whatever the config says -> the `MDLC_BENCH_BLENDER` environment
        variable -> `blender` on `PATH` -> the usual install locations.

        When nothing is found the error lists concrete fixes instead of just
        saying "not found".
        """
        if self.executable:
            path = Path(self.executable).expanduser()
            if path.is_file():
                return str(path)
            found = shutil.which(self.executable)
            if found:
                return found
            raise ConfigError(
                f"[blender] executable = {self.executable!r} is neither a file nor on PATH."
                " Point it at a real path, or leave it empty and let the suite search."
            )

        env = os.environ.get("MDLC_BENCH_BLENDER", "").strip()
        if env and Path(env).expanduser().is_file():
            return str(Path(env).expanduser())

        found = shutil.which("blender")
        if found:
            return found

        # The usual install locations. On Windows we also look at Steam builds:
        # those live in a Steam library as
        # <library>/steamapps/common/Blender/<version>/blender.exe.
        roots = [
            r"C:\Program Files\Blender Foundation",
            "/Applications",
            "/usr/share/blender",
            "/opt/blender",
            "/snap/blender/current",
        ]
        if os.name == "nt":
            roots += [
                r"C:\Program Files (x86)\Steam\steamapps\common\Blender",
                r"C:\Program Files\Steam\steamapps\common\Blender",
                r"D:\SteamLibrary\steamapps\common\Blender",
                r"E:\SteamLibrary\steamapps\common\Blender",
                r"F:\SteamLibrary\steamapps\common\Blender",
            ]

        exe_name = "blender.exe" if os.name == "nt" else "blender"
        for root in roots:
            base = Path(root)
            if not base.is_dir():
                continue
            direct = base / exe_name
            if direct.is_file():
                return str(direct)
            # <root>/<version>/blender(.exe); reversing the string sort puts the
            # newest version first.
            for child in sorted(base.iterdir(), reverse=True):
                exe = child / exe_name
                if child.is_dir() and exe.is_file():
                    return str(exe)

        raise ConfigError(
            "Blender was not found. Pick one of these:\n"
            "  1. set [blender] executable in config.toml to an absolute path;\n"
            "  2. set the MDLC_BENCH_BLENDER=<blender executable> environment variable;\n"
            "  3. put blender on PATH.\n"
            "(If the fixtures were generated already, add --no-generate to skip this step.)"
        )


#: Formats the fixture generator understands.
FORMATS = {"DMX", "SMD", "FBX"}


@dataclass
class Fixture:
    """One entry of `[[fixtures]]`."""

    name: str
    format: str = "DMX"
    subdivisions: int = 112
    size: float = 240.0
    morphs: int = 0
    materials: int = 1
    sequence_frames: int = 2
    note: str = ""

    def __post_init__(self) -> None:
        self.format = self.format.upper()
        if self.format not in FORMATS:
            raise ConfigError(
                f"fixture {self.name!r} has format = {self.format!r}, which is not understood; "
                f"pick one of {' / '.join(sorted(FORMATS))}."
            )
        if self.subdivisions < 1:
            raise ConfigError(f"fixture {self.name!r}: subdivisions must be >= 1.")
        if self.morphs < 0:
            raise ConfigError(f"fixture {self.name!r}: morphs cannot be negative.")

    @property
    def vertices(self) -> int:
        """Grid vertices (`(n+1)^2`)."""
        return (self.subdivisions + 1) ** 2

    @property
    def triangles(self) -> int:
        """Triangle count (`2n^2`, since the mesh is triangulated before export)."""
        return 2 * self.subdivisions * self.subdivisions

    def describe(self) -> str:
        bits = [f"{self.format}", f"{self.triangles:,} tris", f"{self.vertices:,} verts"]
        if self.morphs:
            bits.append(f"{self.morphs} morphs")
        if self.materials > 1:
            bits.append(f"{self.materials} materials")
        if self.sequence_frames:
            bits.append(f"{self.sequence_frames}-frame animation")
        return " / ".join(bits)


@dataclass
class Compiler:
    """One entry of `[[compilers]]`."""

    id: str
    command: list[str]
    bin: str = ""
    search: list[str] = field(default_factory=lambda: ["{out}"])
    expect: list[str] = field(default_factory=lambda: ["*.mdl"])
    cwd: str = "{fixture}"
    env: dict[str, str] = field(default_factory=dict)
    note: str = ""


@dataclass
class Game:
    """`[game]`: for arguments such as `studiomdl -game <dir>`."""

    dir: str = ""


@dataclass
class Config:
    """The whole configuration."""

    general: General
    blender: Blender
    game: Game
    fixtures: list[Fixture]
    compilers: list[Compiler]
    source: Path

    # -- lookup -----------------------------------------------------------

    def fixture(self, name: str) -> Fixture:
        for f in self.fixtures:
            if f.name == name:
                return f
        raise ConfigError(f"no fixture named {name!r}. Available: {self.fixture_names()}")

    def compiler(self, ident: str) -> Compiler:
        for c in self.compilers:
            if c.id == ident:
                return c
        raise ConfigError(f"no compiler named {ident!r}. Available: {self.compiler_ids()}")

    def fixture_names(self) -> str:
        return ", ".join(f.name for f in self.fixtures) or "(none)"

    def compiler_ids(self) -> str:
        return ", ".join(c.id for c in self.compilers) or "(none)"

    # -- templates --------------------------------------------------------

    def placeholders(self) -> dict[str, str]:
        """Placeholders available to every template."""
        return {
            "bench": str(BENCH_DIR),
            "repo": str(REPO_DIR),
            "work": str(self.general.work_dir),
            "game": self.game.dir,
        }

    def expand(self, template: str, extra: dict[str, str]) -> str:
        """Expand one template, reporting unknown placeholders in plain words."""
        try:
            return template.format(**self.placeholders(), **extra)
        except KeyError as exc:
            known = ", ".join(sorted({*self.placeholders(), *extra}))
            raise ConfigError(
                f"template {template!r} uses the unknown placeholder {exc.args[0]!r}; "
                f"available: {known}"
            ) from None
        except IndexError as exc:  # positional parameters such as `{0}`
            raise ConfigError(f"template {template!r} cannot use positional parameters: {exc}") from None

    def executable(self, template: str, extra: dict[str, str]) -> str:
        """Expand `bin` into a path that **actually exists**.

        Two platform-specific fixes, so that writing `.../mdlc` in the config
        still works on Windows:

        1. append `.exe` there;
        2. resolve bare names (`mdlc`) through `PATH`.

        If neither works the value is returned unchanged -- the caller decides
        whether that is an error or something to keep going with.
        """
        raw = self.expand(template, extra)
        if not raw:
            return ""
        path = Path(raw)
        if path.is_file():
            return str(path)
        if os.name == "nt" and not path.suffix and path.with_suffix(".exe").is_file():
            return str(path.with_suffix(".exe"))
        found = shutil.which(raw)
        return found or raw

    def expand_list(self, templates: list[str], extra: dict[str, str]) -> list[str]:
        return [self.expand(t, extra) for t in templates]

    def command_line(self, compiler: Compiler, extra: dict[str, str]) -> list[str]:
        """Expand `command` into a real argv."""
        extra = dict(extra)
        extra["bin"] = self.executable(compiler.bin, extra) if compiler.bin else ""
        return self.expand_list(compiler.command, extra)


# -- loading --------------------------------------------------------------


def _get(table: dict[str, Any], key: str, default: Any) -> Any:
    return table[key] if key in table else default


def _as_list(value: Any, what: str) -> list[str]:
    if isinstance(value, str):
        return [value]
    if isinstance(value, list) and all(isinstance(v, str) for v in value):
        return list(value)
    raise ConfigError(f"{what} must be a string or an array of strings.")


def load(path: Path) -> Config:
    """Read `config.toml` and run the basic checks."""
    if tomllib is None:  # pragma: no cover
        raise ConfigError(
            "Python 3.11+ is required (for the built-in tomllib). On older versions "
            "you can `pip install tomli` and adjust this module."
        )
    if not path.is_file():
        raise ConfigError(
            f"configuration file {path} was not found. Point at one with "
            "`python bench.py --config <file>`, or copy benchmarks/config.toml and edit it."
        )

    with path.open("rb") as fh:
        raw = tomllib.load(fh)

    general_raw = raw.get("general", {})
    work = Path(_get(general_raw, "work_dir", "work"))
    if not work.is_absolute():
        work = (path.parent / work).resolve()
    general = General(
        work_dir=work,
        repeat=int(_get(general_raw, "repeat", 5)),
        warmup=int(_get(general_raw, "warmup", 1)),
        timeout=float(_get(general_raw, "timeout", 1800.0)),
        hash_outputs=bool(_get(general_raw, "hash_outputs", False)),
        keep_outputs=bool(_get(general_raw, "keep_outputs", True)),
    )
    if general.repeat < 1:
        raise ConfigError("[general] repeat must be at least 1.")
    if general.warmup < 0:
        raise ConfigError("[general] warmup cannot be negative.")

    blender_raw = raw.get("blender", {})
    blender = Blender(
        executable=str(_get(blender_raw, "executable", "") or ""),
        timeout=float(_get(blender_raw, "timeout", 7200.0)),
    )

    game = Game(dir=str(raw.get("game", {}).get("dir", "") or ""))

    fixtures: list[Fixture] = []
    for i, item in enumerate(raw.get("fixtures", [])):
        if "name" not in item:
            raise ConfigError(f"[[fixtures]] entry #{i + 1} has no name.")
        fixtures.append(
            Fixture(
                name=str(item["name"]),
                format=str(_get(item, "format", "DMX")),
                subdivisions=int(_get(item, "subdivisions", 112)),
                size=float(_get(item, "size", 240.0)),
                morphs=int(_get(item, "morphs", 0)),
                materials=int(_get(item, "materials", 1)),
                sequence_frames=int(_get(item, "sequence_frames", 2)),
                note=str(_get(item, "note", "")),
            )
        )
    if not fixtures:
        raise ConfigError("[[fixtures]] is empty -- add at least one fixture.")
    names = [f.name for f in fixtures]
    if len(set(names)) != len(names):
        raise ConfigError(f"duplicate fixture names: {names}")

    compilers: list[Compiler] = []
    for i, item in enumerate(raw.get("compilers", [])):
        if "id" not in item:
            raise ConfigError(f"[[compilers]] entry #{i + 1} has no id.")
        if "command" not in item:
            raise ConfigError(f"compiler {item['id']!r} has no command.")
        compilers.append(
            Compiler(
                id=str(item["id"]),
                command=_as_list(item["command"], f"{item['id']}.command"),
                bin=str(_get(item, "bin", "") or ""),
                search=_as_list(_get(item, "search", ["{out}"]), f"{item['id']}.search"),
                expect=_as_list(_get(item, "expect", ["*.mdl"]), f"{item['id']}.expect"),
                cwd=str(_get(item, "cwd", "{fixture}")),
                env={k: str(v) for k, v in _get(item, "env", {}).items()},
                note=str(_get(item, "note", "")),
            )
        )
    if not compilers:
        raise ConfigError("[[compilers]] is empty -- add at least one compiler.")
    ids = [c.id for c in compilers]
    if len(set(ids)) != len(ids):
        raise ConfigError(f"duplicate compiler ids: {ids}")

    return Config(
        general=general,
        blender=blender,
        game=game,
        fixtures=fixtures,
        compilers=compilers,
        source=path.resolve(),
    )
