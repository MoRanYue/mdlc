# 作为库使用

> 这一页是给**集成方**（第三方 GUI / 构建系统）看的，普通模型作者不需要读。
> 只想编译模型请看 [README](../README.md)。

`mdlc` 同时是一个库。要给第三方 GUI / 构建系统内置编译器时，**不要自己串一遍
管线** —— 用 `mdlc::pipeline`：

```rust
use std::path::Path;
use mdlc::model::ModelDesc;
use mdlc::pipeline::{self, PipelineOptions};

let text = std::fs::read_to_string("model.toml")?;
let desc = ModelDesc::from_toml(&text)?;

// 只编译，不碰文件系统：拿到四件套字节 + 编译期中间结果。
let out = pipeline::build(&desc, Path::new("."), PipelineOptions::default())?;

// 或者直接落盘（按 `$modelname` 建目录）。
let paths = pipeline::write_files(&out, Path::new("out"))?;
println!("{}", paths.mdl.display());
```

`pipeline::build` 只读文件、不写文件；`write_files` 只写文件。**分两步是为了让
GUI 能在写盘前显示摘要或让用户确认。**

## 为什么必须用它

`compile()` 只做「描述 → 编译期中间结果」，它**完全不管碰撞 SMD**，也不写任何文件。
自己串管线最容易漏掉的是「碰撞 SMD → `physicsbone`」这一步：

```rust
// 这一步漏了，`.mdl`/`.vvd`/`.dx90.vtx` 全对，只有 physicsbone 悄悄全 0。
if let Some(cs) = &collision_smd {
    let parents = mdlc::compile::bone_parents(&compiled.desc);
    compiled.physics_bone = mdlc::phy::physics_bone_table(cs, compiled.desc.bones.len(), &parents);
}
```

## 错误与退出码

`PipelineError` 是**枚举**，每个失败点一个变体，`kind()` 映射到与 `mdlc.exe` 相同的退出码：

| 变体 | `kind()` | 退出码 | 触发 |
|---|---|---|---|
| `Compile(Vec<CompileError>)` | `Build` | 1 | 描述/QC 校验失败、读不到源 SMD |
| `ParseCollisionSmd { path, source }` | `Build` | 1 | 碰撞 SMD 解析失败 |
| `WriteMdl(WriteError)` | `Build` | 1 | `.mdl` 写出失败 |
| `BuildVvd` / `EncodeVvd` / `CheckVvd` | `Build` | 1 | VVD 构造 / 编码 / 自检失败 |
| `WriteVtx` / `CheckVtx` | `Build` | 1 | VTX 写出 / 自检失败 |
| `BuildPhy` / `CheckPhy` | `Build` | 1 | PHY 构造 / 自检失败 |
| `ResolveCollisionSmd(SrcError)` | `Io` | 2 | 碰撞 SMD 路径解析失败 |
| `ReadCollisionSmd { path, source }` | `Io` | 2 | 碰撞 SMD 读不到 |
| `CreateDir { path, source }` | `Io` | 2 | 建目录失败 |
| `WriteFile { path, source }` | `Io` | 2 | 写文件失败 |

`kind()` 是**穷举**的，没有 `_ => Build` 兜底 —— 新增变体时编译器会强制你归类。

底层错误**原样保留**（`#[source]`），不再被 `format!` 压成字符串：例如
`ReadCollisionSmd` 的 `source` 是 `std::io::Error`，可以直接
`.downcast_ref::<std::io::Error>().kind() == ErrorKind::NotFound` 判断「文件不存在」
而不是去匹配中文文案。

`lines()` 给出与 CLI 逐行相同的文案（编译失败是多行，其余是单行）；`Display`
就是它们拼起来的。GUI 直接用 `lines()` 渲染日志即可，不必自己拼字符串。

## 摘要

`PipelineOutput::summary_lines(&paths)` 复刻 `mdlc.exe` 成功时打印的那张表
（模型 / 版本 / checksum / 统计 / 每个产物的字节数与路径 / `**编译成功**`）。
CLI 自己就是逐行 `println!` 它 —— 所以 GUI 显示的与命令行看到的**逐字相同**。
