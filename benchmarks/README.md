# Benchmark suite

Compare how fast different Source model compilers are on the same set of
**automatically generated** fixtures.

The problem it solves: "mdlc is fast" or "mdlc is slow" only means something
next to another compiler, and hand-made fixtures are both tedious and easy to get
wrong (the same nominal size of DMX / SMD / FBX can end up with different vertex
counts). This suite chains fixture generation, timing, output validation and a
result table into one command.

## Requirements

| Item | Requirement |
|---|---|
| Python | **3.11+** (uses the built-in `tomllib`; no third-party packages) |
| Blender | 3.x / 4.x / 5.x with **BlenderSourceTools** enabled (needed for DMX / SMD export) |
| Compiler under test | A command line that works, producing outputs you can find |

Blender is located in this order: the path in `config.toml` -> the
`MDLC_BENCH_BLENDER` environment variable -> `blender` on `PATH` -> common install
locations (including Steam libraries).

## Quick start

```bash
# 1. See what the config contains
python benchmarks/bench.py --list

# 2. Run one real-sized fixture (Blender generates it on first use, ~30 s)
python benchmarks/bench.py -f dmx-25k

# 3. Run everything, 5 runs per cell, save the results
python benchmarks/bench.py --repeat 5 --json results.json
```

The output looks like this:

```
-- dmx-25k  .  DMX / 25,088 tris / 12,769 verts / 2-frame animation
  compiler            median      best     worst   spread   throughput   outputs
  ------------------------------------------------------------------------------
  mdlc           *     38ms      38ms      38ms      0ms     657k t/s   799.7KB

-- dmx-25k-m30  .  DMX / 25,088 tris / 12,769 verts / 30 morphs / 2-frame animation
  compiler            median      best     worst   spread   throughput   outputs
  ------------------------------------------------------------------------------
  mdlc           *    125ms     124ms     125ms      2ms     201k t/s     6.6MB

(5 measured runs per cell, plus 1 discarded warmup run)
```

`*` marks the fastest compiler for that fixture; `outputs` is the total size of
the produced files, and `throughput` is the triangle count divided by the median
wall time.

## Configuration

The config file is `benchmarks/config.toml` by default; `--config` points at
another one. The file is fully commented, so only the essentials are repeated
here.

### `[general]`

| Key | Default | Meaning |
|---|---|---|
| `work_dir` | `work` | Where fixtures and outputs live (relative to the config file) |
| `repeat` | `5` | Measured runs per combination |
| `warmup` | `1` | Warm-up runs (discarded, not part of the statistics) |
| `timeout` | `1800` | Per-compile timeout in seconds |
| `hash_outputs` | `false` | SHA256 the outputs (shows whether runs agree, but reads every output back) |

### `[[fixtures]]`

The size comes from `subdivisions`: vertices `(n+1)^2`, triangles `2n^2`.

| `subdivisions` | Triangles | Rough generation time |
|---|---|---|
| 32 | 2,048 | < 1 s |
| 112 | 25,088 | ~3 s |
| 355 | 252,050 | ~4 minutes and 452 MB for DMX |

`format` is `DMX` / `SMD` / `FBX`; `morphs` is the number of morph targets;
`materials` > 1 hands the faces to several materials in turn; `sequence_frames`
> 0 also exports an animation.

### `[[compilers]]`

```toml
[[compilers]]
id = "mdlc"
bin = "{repo}/target/release/mdlc"
command = ["{bin}", "build-qc", "{qc}", "--out", "{out}"]
search = ["{out}"]
expect = ["*.mdl", "*.vvd"]
```

`command` is an **argv array** (not a shell string). Placeholders:

| Placeholder | Meaning |
|---|---|
| `{bin}` | This entry's `bin` (`.exe` is appended on Windows; a bare name goes through `PATH`) |
| `{qc}` | The fixture's `.qc` path |
| `{fixture}` | The fixture directory |
| `{out}` | This run's output directory (**wiped before every run**) |
| `{work}` / `{bench}` / `{repo}` / `{game}` | Work directory / `benchmarks/` / repository root / `[game] dir` |

`search` + `expect` decide what counts as output: files are globbed from the
`search` directory with the `expect` patterns. **An exit code of 0 with no output
found is a failure** -- that catches the false green where the flags were wrong
and the compiler produced nothing.

Adding another compiler (any exe configured in Crowbar, any other mdl compiler)
takes one entry and no code changes. A commented-out example for the official
`studiomdl` is in `config.toml`; uncomment it and point `bin` / `[game] dir` at
your install.

## Fixture cache

The fixture directory name carries every parameter that affects the output:

```
work/fixtures/dmx-25k-m30-s112-m30-t1-f2/
    mesh.dmx        anims/idle.dmx        fixture.qc        manifest.json
```

As long as the parameters are unchanged the fixture is reused and Blender is not
run again. Delete the directory to force a rebuild. `--no-generate` errors out
instead of calling Blender when a fixture is missing.

## Measurement rules

- **Wall-clock time only** (`time.perf_counter` around the whole child process).
  Peak memory needs a process-accounting API and the standard library has no
  reliable cross-platform way to get it; forcing it in would make the suite
  "runs on one OS only". If you care about memory, pair it with `time -v` or a
  process monitor on your side.
- **The output directory is wiped before every run**: otherwise the second run
  may just hit the previous run's files and you end up measuring "the compiler
  did nothing".
- **Warm-up runs are discarded**: the first run reads from disk and warms the CPU
  caches, so it is noticeably slower.
- The median is steadier than the mean; `spread` (worst - best) tells you whether
  the machine was steady. When it is large, do not trust differences of a few
  tens of milliseconds.

## Known limits and traps

These are repeated here as well as in the code, because all of them actually
happened:

1. **A morph target's default value is 1.0, not 0.** Blender's
   `shape_key_add()` defaults to `value = 1.0`; without resetting it the exported
   "base mesh" is really the shape with every morph target at full strength, so
   every face normal differs and vertices can no longer be welded (10,201
   vertices became 58,336 in testing, which looks like "this format is 4x slower"
   but is a broken fixture).
2. **A mesh with shape keys can no longer be `modifier_apply`-ed**, so
   triangulation has to happen before the shape keys are created.
3. **BlenderSourceTools' `export_path` is a directory, not a file name**: the
   DMX/SMD file name comes from the collection and the animation is written into
   an `anims/` subdirectory; FBX bakes the animation into the same file.
4. **Payload and output size are not the same thing**: at the same size a DMX
   source file can be several times larger than FBX (452 MB vs 80 MB) while the
   compile times are close -- file size is not a performance metric.
5. **Compare on one machine only**: results from different machines, compiler
   flags or `-O` levels are not comparable.

## Directory layout

```
benchmarks/
    bench.py                 command line entry point
    config.toml              default configuration
    benchlib/
        config.py            config parsing and template expansion
        fixture.py           drive Blender, write the QC, cache fixtures
        measure.py           timing and output collection
        report.py            table / JSON / CSV output
    blender/
        make_fixture.py      the Blender-side generation script
    work/                    generated files (not committed)
```

## Adding your own compiler

1. Add a `[[compilers]]` block to `config.toml` (copy the `mdlc` entry, change
   `command`);
2. Check that `--list` shows it;
3. `python benchmarks/bench.py -c your-id`.

If it needs a specific working directory or environment variables, use the `cwd`
and `env` fields:

```toml
[[compilers]]
id = "myc"
bin = "C:/tools/myc.exe"
command = ["{bin}", "-game", "{game}", "{qc}"]
search = ["{game}/models/bench"]
expect = ["*.mdl"]
cwd = "{fixture}"          # the fixture directory is the default
env = { MYC_STRICT = "1" }
```
