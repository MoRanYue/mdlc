"""Implementation modules of the mdlc benchmark suite.

`bench.py` is the only entry point; this package holds the reusable pieces:

- [`config`]   read `config.toml` and expand the user's command templates
- [`fixture`]  drive Blender to generate fixtures (mesh / morphs / animation / QC)
- [`measure`]  run one compile and record wall time and peak memory
- [`report`]   lay the results out as a table (console / Markdown / JSON)

Design rule: **standard library only** (Python 3.11+ for `tomllib`), no third-party
dependencies, so anyone who clones the repository can measure without installing
anything.
"""

__all__ = ["config", "fixture", "measure", "report"]
