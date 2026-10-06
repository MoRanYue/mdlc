//! 编排管线：`ModelDesc` → 四件套字节 → 落盘。
//!
//! # 为什么需要这个模块
//!
//! [`crate::compile::compile`] 只做「描述 → 编译期 IR」，它**完全不管**
//! 碰撞 SMD，也不写任何文件；`write_mdl` / VVD / VTX / PHY 各管一段。
//! 把它们串成「一次编译」的顺序本身是有讲究的 —— 第三方工具（GUI、
//! 构建系统、批处理脚本）自己串一遍时最容易漏掉的是
//! **碰撞 SMD → `physicsbone`** 这一步：漏了以后 `.mdl` / `.vvd` /
//! `.dx90.vtx` 全对，**只有 `physicsbone` 悄悄全 0**。
//!
//! # 两步 API
//!
//! ```no_run
//! # fn main() -> Result<(), Box<dyn std::error::Error>> {
//! use std::path::Path;
//! use mdlc::pipeline::{self, PipelineOptions};
//!
//! let text = std::fs::read_to_string("x.toml")?;
//! let desc = mdlc::model::ModelDesc::from_toml(&text)?;
//! let out = pipeline::build(&desc, Path::new("."), PipelineOptions::default())?;
//! let paths = pipeline::write_files(&out, Path::new("out"))?;
//! println!("{}", paths.mdl.display());
//! # Ok(())
//! # }
//! ```
//!
//! [`build`] 只碰内存（不建目录、不写文件），[`write_files`] 只碰磁盘
//! （不再编译）。GUI 可以「先编译、让用户确认、再落盘」。
//!
//! # 顺序上的两条硬约束
//!
//! 1. **碰撞 SMD 必须在 `write_mdl` 之前解析** —— `physicsbone`
//!    （`mstudiobone_t` `+0xAC`）要写进**骨骼表**，而骨骼表是
//!    `write_mdl` 产出的。
//! 2. **三个自检必须在字节离开本模块之前跑** —— VVD / VTX / PHY 各有
//!    `check_invariants`，失败说明是写出器的 bug，而不是用户输入的问题。
//!
//! # 错误分类与退出码
//!
//! [`PipelineError::kind`] 把失败分成两类，与 `mdlc.exe` 的退出码契约
//! 一一对应（既有探针按码判定）：
//!
//! | [`PipelineErrorKind`] | 退出码 | 含义 |
//! |---|---|---|
//! | `Build` | 1 | 编译 / 写出 / 自检失败 |
//! | `Io` | 2 | 路径解析 / 读文件 / 建目录 / 写文件失败 |

use std::path::{Path, PathBuf};

use crate::compile::{CompileError, SrcError, bone_parents, compile, resolve_smd_path};
use crate::mdl_writer::{WriteError, write_mdl};
use crate::model::{CompiledModelDesc, ModelDesc};
use crate::phy::{self, PhyError};
use crate::smd::SmdError;
use crate::vtx_writer::{self, VtxOptions, VtxWriteError};
use crate::vvd::VvdError;

/// 编排选项。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PipelineOptions {
    /// 是否对每个 strip group 跑 `meshopt::optimize_vertex_cache`。
    ///
    /// 与 TOML 的 `[model] optimize_vtx` 是 **`||`** 关系 —— 任一为真即开启。
    /// 默认关闭，以保持与真 `studiomdl.exe` 逐字节一致
    /// （见 [`crate::vtx_writer::VtxOptions`]）。
    pub optimize_vtx: bool,
}

/// 编排失败的原因。
///
/// # 为什么每个变体都带着**源错误**而不是一个 `String`
///
/// 先前这里只有 `Compile(Vec<CompileError>)` 与三个 `String` 变体
/// （`Collision` / `Write` / `Io`）。调用方拿到 `Io("读不到碰撞 SMD x.smd：
/// 系统找不到指定的文件。 (os error 2)")` 之后**只能去正则匹配中文前缀**
/// 才能判断「是读不到文件」还是「路径写法不对」，而真正的
/// [`std::io::Error`] —— 它带着 [`std::io::ErrorKind`] —— 已经被
/// `format!` 扔掉了。
///
/// 现在每个变体各自持有**产生它的那个错误类型**，`source()` 能把整条链
/// 走到底，调用方可以 `match` 到具体种类（例如
/// `Err(PipelineError::ReadCollisionSmd { source, .. }) if source.kind() ==
/// ErrorKind::NotFound`）。[`Self::lines`] 仍拼出**与改造前逐字节相同**
/// 的文案，所以 CLI 输出与既有探针的断言都不受影响。
///
/// ⚠️ 因此本类型**不是** `Clone` / `PartialEq`：它内部装着
/// [`std::io::Error`]，那个类型两者都不实现，而且「两次失败是否相等」
/// 对 IO 错误本来也没有意义。需要比较请用 [`Self::kind`]。
#[derive(Debug, thiserror::Error)]
pub enum PipelineError {
    /// [`crate::compile::compile`] 失败（含 [`ModelDesc::validate`] 的失败）。
    #[error("{}", Self::compile_lines(.0).join("\n"))]
    Compile(Vec<CompileError>),
    /// 碰撞 SMD 的**路径字符串**不合法（缺扩展名等）。
    #[error("错误：{}", .0)]
    ResolveCollisionSmd(#[source] SrcError),
    /// 碰撞 SMD 路径解析出来了，但文件读不进来。
    #[error("错误：读不到碰撞 SMD {}：{}", path.display(), source)]
    ReadCollisionSmd {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    /// 碰撞 SMD 读进来了，但解析不了。
    #[error("错误：解析碰撞 SMD {} 失败：{}", path.display(), source)]
    ParseCollisionSmd {
        path: PathBuf,
        #[source]
        source: SmdError,
    },
    /// [`write_mdl`] 失败（`.mdl` 字节的构造）。
    #[error("错误：{}", .0)]
    WriteMdl(#[source] WriteError),
    /// `lod::build_vvd` 失败。
    #[error("错误：构造 VVD 失败：{}", .0)]
    BuildVvd(#[source] VvdError),
    /// `Vvd::to_bytes` 失败。
    #[error("错误：写出 VVD 失败：{}", .0)]
    EncodeVvd(#[source] VvdError),
    /// VVD 的自检没过 —— 这是**本实现的 bug**，不是用户输入的问题。
    #[error("错误：写出的 VVD 不自洽（本实现的 bug）：{}", .0)]
    CheckVvd(#[source] VvdError),
    /// `vtx_writer::write_vtx_with` 失败。
    #[error("错误：写出 VTX 失败：{}", .0)]
    WriteVtx(#[source] VtxWriteError),
    /// VTX 的自检没过 —— 同上，是本实现的 bug。
    #[error("错误：写出的 VTX 不自洽（本实现的 bug）：{}", .0)]
    CheckVtx(#[source] VtxWriteError),
    /// `phy::build_phy_from_smd` / `build_ragdoll_phy_from_smd` 失败。
    #[error("错误：构造 PHY 失败：{}", .0)]
    BuildPhy(#[source] PhyError),
    /// PHY 的自检没过 —— 同上，是本实现的 bug。
    #[error("错误：写出的 PHY 自检失败（本实现的 bug）：{}", .0)]
    CheckPhy(#[source] PhyError),
    /// 建输出目录失败。
    #[error("错误：建目录 {} 失败：{}", path.display(), source)]
    CreateDir {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    /// 写产物文件失败。
    #[error("错误：写 {} 失败：{}", path.display(), source)]
    WriteFile {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

/// 失败的大类 —— 与 `mdlc.exe` 的退出码一一对应。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PipelineErrorKind {
    /// 编译 / 写出 / 自检失败。
    Build,
    /// 路径解析 / 读文件 / 建目录 / 写文件失败。
    Io,
}

impl PipelineErrorKind {
    /// 与 `mdlc.exe` 的退出码契约一致：`Build` → 1，`Io` → 2。
    pub fn exit_code(self) -> u8 {
        match self {
            PipelineErrorKind::Build => 1,
            PipelineErrorKind::Io => 2,
        }
    }
}

impl PipelineError {
    /// 失败的大类。
    ///
    /// ⚠️ 新增变体时**必须**在这里归类 —— 先前用 `_ => Build` 兜底，
    /// 那样会把「忘了归类的 IO 失败」静默报成编译失败（退出码从 2 变 1，
    /// 而既有探针是按退出码判定的）。写成穷举后，加变体会得到编译错误。
    pub fn kind(&self) -> PipelineErrorKind {
        match self {
            PipelineError::ResolveCollisionSmd(_)
            | PipelineError::ReadCollisionSmd { .. }
            | PipelineError::CreateDir { .. }
            | PipelineError::WriteFile { .. } => PipelineErrorKind::Io,
            PipelineError::Compile(_)
            | PipelineError::ParseCollisionSmd { .. }
            | PipelineError::WriteMdl(_)
            | PipelineError::BuildVvd(_)
            | PipelineError::EncodeVvd(_)
            | PipelineError::CheckVvd(_)
            | PipelineError::WriteVtx(_)
            | PipelineError::CheckVtx(_)
            | PipelineError::BuildPhy(_)
            | PipelineError::CheckPhy(_) => PipelineErrorKind::Build,
        }
    }

    /// [`Self::Compile`] 的文案（首行是计数，之后每条一行）。
    ///
    /// 抽成关联函数是为了让 `#[error(...)]` 属性也能用它 —— 属性里写不出
    /// 带循环的表达式，而这里需要「首行 + N 条缩进」的形态。
    fn compile_lines(errs: &[CompileError]) -> Vec<String> {
        let mut v = vec![format!("编译失败，{} 处错误：", errs.len())];
        v.extend(errs.iter().map(|e| format!("  - {e}")));
        v
    }

    /// 要打印给用户的**最终文案**（`mdlc.exe` 逐行原样输出；GUI 可直接显示）。
    ///
    /// 编译失败是多行（首行是计数，之后每条一行）；其余是单行。
    ///
    /// 与 [`std::fmt::Display`] 的关系：[`Self::Compile`] 之外**完全一致**
    /// （`to_string()` 就是 `lines().join("\n")`），只有 `Compile` 需要
    /// 拆成多行才好让 CLI 逐行 `diagln!`。
    pub fn lines(&self) -> Vec<String> {
        match self {
            PipelineError::Compile(errs) => Self::compile_lines(errs),
            // 其余变体的 `Display` 已经就是那一行 —— 直接复用，
            // 避免文案在两处各写一遍（先前注释里那条「不能走 Display」的
            // 警告说的是旧写法：当时 `Display` 是本函数拼出来的，
            // 现在反过来，`Display` 是各变体自己的属性）。
            other => vec![other.to_string()],
        }
    }
}

/// 一次编排的产物：各文件的**字节**，以及编译期 IR。
///
/// 这里不存路径 —— 路径由 [`PipelineOutput::paths`] 按 `out_root` 现算，
/// 所以同一份产物可以写到任意目录。
#[derive(Debug, Clone, PartialEq)]
pub struct PipelineOutput {
    /// 模型名去掉 `.mdl` 后的相对路径（如 `models/mymod/x`），决定各文件名。
    pub stem: String,
    /// `.mdl` 字节。
    pub mdl: Vec<u8>,
    /// `.vvd` 字节。
    pub vvd: Vec<u8>,
    /// `.dx90.vtx` 字节。
    pub vtx: Vec<u8>,
    /// `.ani` 字节（`$animblocksize` 的外置动画块，没有就是 `None`）。
    pub ani: Option<Vec<u8>>,
    /// `.phy` 字节（有碰撞 SMD 时才有）。
    pub phy: Option<Vec<u8>>,
    /// 配对令牌，已写进各文件头部。
    pub checksum: i32,
    /// 编译期 IR —— `physics_bone` 已按碰撞 SMD 填好。
    pub compiled: CompiledModelDesc,
}

/// 落盘后的实际路径。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutputPaths {
    /// `out_root/<stem>.mdl`
    pub mdl: PathBuf,
    /// `out_root/<stem>.vvd`
    pub vvd: PathBuf,
    /// `out_root/<stem>.dx90.vtx`
    pub vtx: PathBuf,
    /// `out_root/<stem>.ani`（没有就是 `None`）。
    pub ani: Option<PathBuf>,
    /// `out_root/<stem>.phy`（没有就是 `None`）。
    pub phy: Option<PathBuf>,
}

impl PipelineOutput {
    /// 产物在 `out_root` 下的实际路径。
    pub fn paths(&self, out_root: &Path) -> OutputPaths {
        let stem = &self.stem;
        OutputPaths {
            mdl: out_root.join(format!("{stem}.mdl")),
            vvd: out_root.join(format!("{stem}.vvd")),
            vtx: out_root.join(format!("{stem}.dx90.vtx")),
            ani: self.ani.as_ref().map(|_| out_root.join(format!("{stem}.ani"))),
            phy: self.phy.as_ref().map(|_| out_root.join(format!("{stem}.phy"))),
        }
    }

    /// 编译摘要（`mdlc.exe` 逐行原样输出；GUI 可直接显示在日志面板）。
    pub fn summary_lines(&self, paths: &OutputPaths) -> Vec<String> {
        let d = &self.compiled.desc;
        let mut v = vec![
            format!("模型        {}", d.model.name),
            format!("版本        {}", d.version()),
            format!("checksum    {}  （已写入各文件，配对一致）", self.checksum),
            String::new(),
            format!("骨骼        {}", d.bones.len()),
            format!("材质        {}", d.materials.textures.len()),
            format!("body part   {}", d.bodyparts.len()),
            format!("顶点        {}", self.compiled.total_vertices()),
            format!("三角形      {}", self.compiled.total_triangles()),
            String::new(),
            format!("MDL         {:>10} 字节  {}", self.mdl.len(), paths.mdl.display()),
            format!("VVD         {:>10} 字节  {}", self.vvd.len(), paths.vvd.display()),
            format!("VTX         {:>10} 字节  {}", self.vtx.len(), paths.vtx.display()),
        ];
        if let (Some(p), Some(b)) = (&paths.phy, &self.phy) {
            v.push(format!("PHY         {:>10} 字节  {}", b.len(), p.display()));
        }
        v.push(String::new());
        v.push("**编译成功**（各文件布局自检均通过）".to_string());
        v
    }
}

/// 编译 + 构造全部产物字节，**不落盘**。
///
/// `base` 是相对路径（`$modelname` / `$cdmaterials` / SMD 引用）的解析基准，
/// 通常是被编译文件（`.toml` / `.qc`）所在目录。
///
/// 这一步只读文件（SMD / FBX / `.vta`），不建目录、不写文件。
pub fn build(
    desc: &ModelDesc,
    base: &Path,
    opts: PipelineOptions,
) -> Result<PipelineOutput, PipelineError> {
    // ---- ① 编译：描述 + SMD/FBX → 编译期 IR ----
    //
    // `compile()` 内部第一件事就是 `desc.validate()`，所以这里不必再查一遍。
    let mut compiled = hotpath::measure_block!("pipeline: compile()", {
        compile(desc, base).map_err(PipelineError::Compile)?
    });

    // ---- ② 碰撞 SMD：**必须在 `write_mdl` 之前**解析 ----
    //
    // 因为 `physicsbone`（`mstudiobone_t` `+0xAC`）要写进**骨骼表**，
    // 而骨骼表是 `write_mdl` 产出的。碰撞几何来自**单独的 SMD**
    // （官方 `$collisionmodel` / `$collisionjoints`），所以要提前读。
    //
    // 解析结果留到下面构造 `.phy` 时**复用**，避免读两次。
    let collision_smd = match desc.physics.smd.as_deref() {
        None => None,
        Some(rel_smd) => {
            let p = resolve_smd_path(base, rel_smd).map_err(PipelineError::ResolveCollisionSmd)?;
            let text = std::fs::read_to_string(&p).map_err(|source| {
                PipelineError::ReadCollisionSmd {
                    path: p.clone(),
                    source,
                }
            })?;
            Some(
                crate::smd::parse_smd(&text).map_err(|source| PipelineError::ParseCollisionSmd {
                    path: p.clone(),
                    source,
                })?,
            )
        }
    };

    // ---- ③ `physicsbone`：只有碰撞几何能提供 solid → 骨骼的映射 ----
    //
    // ⚠️ 用**骨骼表**的父链（`compiled.desc.bones`）而不是碰撞 SMD 的
    // `nodes` —— 两者在静态道具 / 骨骼塌缩后可能不同，而落盘的
    // `physicsbone` 下标必须对**最终骨骼表**成立。
    if let Some(cs) = &collision_smd {
        let parents = bone_parents(&compiled.desc);
        compiled.physics_bone =
            phy::physics_bone_table(cs, compiled.desc.bones.len(), &parents);
    }

    // ---- ④ `.mdl` ----
    let out = hotpath::measure_block!("pipeline: write_mdl", {
        write_mdl(&compiled).map_err(PipelineError::WriteMdl)?
    });

    // ---- ⑤ `.vvd`（单 / 多 LOD 自动分派）----
    //
    // `vvd` 只在块内用（自检读它的字段）；逃逸出去的只有字节。
    let vvd_bytes = hotpath::measure_block!("pipeline: build_vvd + to_bytes", {
        let vvd = crate::lod::build_vvd(&compiled, out.checksum)
            .map_err(PipelineError::BuildVvd)?;
        let bytes = vvd.to_bytes().map_err(PipelineError::EncodeVvd)?;
        // 写出后立刻自检 —— 偏移/长度对不上说明我们的写出器有 bug。
        crate::vvd::check_invariants(&vvd, bytes.len()).map_err(PipelineError::CheckVvd)?;
        bytes
    });

    // ---- ⑥ `.dx90.vtx`：没有它模型在游戏里根本不渲染 ----
    let vtx = hotpath::measure_block!("pipeline: write_vtx", {
        vtx_writer::write_vtx_with(
            &compiled,
            VtxOptions {
                optimize_vertex_cache: desc.model.optimize_vtx || opts.optimize_vtx,
            },
        )
        .map_err(PipelineError::WriteVtx)?
    });
    vtx_writer::check_invariants(&vtx, &compiled).map_err(PipelineError::CheckVtx)?;

    // ---- ⑦ 文件名：`out_root` + 模型名（去掉 `.mdl` 换扩展名）----
    let rel = desc.output_name();
    let stem = rel.strip_suffix(".mdl").unwrap_or(&rel).to_string();

    // ---- ⑧ `.phy`（可选，由 `[physics].smd` 触发）----
    let phy_bytes = match &collision_smd {
        None => None,
        Some(smd) => {
            // `editparams.modelname` 用**不带扩展名**的模型名。
            let phys_name = Path::new(&stem)
                .file_stem()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_else(|| "unnamed".to_string());
            // `[physics].mass` 与 `.mdl` 头部 `+0x148` 是**同一个值**
            // （`write.cpp:2092` `phdr->mass = GetCollisionModelMass();`）。
            //
            // `joints` 走 ragdoll（每骨骼一个 solid），
            // 否则走 prop 形态（单 solid，可选 `concave`）。
            let mass = desc.physics.effective_mass();
            // prop 形态的 solid `name` = **碰撞 SMD 的 basename** ——
            // 官方 `ProcessSingleBody`（`collisionmodel.cpp:1563-1571`）用
            // `Q_FileBase(pmodel->filename, ...)`，与 `$modelname` 无关。
            let collision_smd_name = desc
                .physics
                .smd
                .as_deref()
                .map(|p| {
                    Path::new(p)
                        .file_stem()
                        .map(|s| s.to_string_lossy().into_owned())
                        .unwrap_or_else(|| p.to_string())
                })
                .unwrap_or_else(|| phys_name.clone());
            // `surfaceprop` 从**模型头**取（实测 2498/2498 恒等）。
            let surface_prop = desc.model.surface_prop.as_deref().unwrap_or("default");
            // 官方 `ConvertToWorldSpace` 用的是**第一个序列第 0 帧**
            // （`collisionmodel.cpp:713` `CalcBoneTransforms( g_panimation[0], 0, ... )`）；
            // 碰撞 SMD 自己的姿态完全不参与。没有序列时传 `None`。
            let pose_world = compiled
                .sequences
                .first()
                .and_then(|s| s.frames.first())
                .map(|f| phy::sequence_pose_world(&compiled.desc.bones, f));
            let ident = phy::PhyIdentity {
                model_name: &phys_name,
                collision_smd_name: &collision_smd_name,
                surface_prop,
            };
            let built = if desc.physics.joints {
                phy::build_ragdoll_phy_from_smd(
                    smd,
                    ident,
                    out.checksum as u32,
                    mass,
                    &desc.physics,
                )
            } else {
                phy::build_phy_from_smd(
                    smd,
                    ident,
                    out.checksum as u32,
                    mass,
                    &desc.physics,
                    pose_world.as_ref(),
                )
            };
            Some(built.map_err(PipelineError::BuildPhy)?)
        }
    };
    // 写出前自检 —— 失败说明是本实现的 bug。
    if let Some(b) = &phy_bytes {
        phy::check_invariants(b).map_err(PipelineError::CheckPhy)?;
    }

    Ok(PipelineOutput {
        stem,
        mdl: out.bytes,
        vvd: vvd_bytes,
        vtx: vtx.bytes,
        // `$animblocksize` 的外置动画块。
        ani: out.ani,
        phy: phy_bytes,
        checksum: out.checksum,
        compiled,
    })
}

/// 把 [`build`] 的产物落盘（自动建目录），返回实际路径。
///
/// 目录结构由模型名决定：模型名 `models/mymod/x.mdl` 会在 `out_root` 下
/// 建出 `models/mymod/`。
pub fn write_files(out: &PipelineOutput, out_root: &Path) -> Result<OutputPaths, PipelineError> {
    let paths = out.paths(out_root);

    for p in [&paths.mdl, &paths.vvd, &paths.vtx]
        .into_iter()
        .chain(paths.ani.iter())
        .chain(paths.phy.iter())
    {
        if let Some(dir) = p.parent()
            && let Err(source) = std::fs::create_dir_all(dir)
        {
            return Err(PipelineError::CreateDir {
                path: dir.to_path_buf(),
                source,
            });
        }
    }

    let write_one = |path: &Path, bytes: &[u8]| -> Result<(), PipelineError> {
        std::fs::write(path, bytes).map_err(|source| PipelineError::WriteFile {
            path: path.to_path_buf(),
            source,
        })
    };
    write_one(&paths.mdl, &out.mdl)?;
    write_one(&paths.vvd, &out.vvd)?;
    write_one(&paths.vtx, &out.vtx)?;
    if let (Some(p), Some(b)) = (&paths.ani, &out.ani) {
        write_one(p, b)?;
    }
    if let (Some(p), Some(b)) = (&paths.phy, &out.phy) {
        write_one(p, b)?;
    }
    Ok(paths)
}

/// [`build`] + [`write_files`] 的便捷组合。
pub fn build_and_write(
    desc: &ModelDesc,
    base: &Path,
    out_root: &Path,
    opts: PipelineOptions,
) -> Result<OutputPaths, PipelineError> {
    let out = build(desc, base, opts)?;
    write_files(&out, out_root)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mdl_writer::{BONE_SIZE, off};

    /// 每个用例用**唯一**的临时目录：测试并行跑，共用目录会互相删文件。
    fn tmpdir(tag: &str) -> PathBuf {
        use std::sync::atomic::{AtomicUsize, Ordering};
        static SEQ: AtomicUsize = AtomicUsize::new(0);
        let n = SEQ.fetch_add(1, Ordering::Relaxed);
        let d = std::env::temp_dir().join(format!("mdlc-pipe-{tag}-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn write(dir: &Path, name: &str, text: &str) {
        std::fs::write(dir.join(name), text).unwrap();
    }

    /// 渲染网格：2 根骨骼，一个挂在 `tip` 上的三角形。
    const MESH_SMD: &str = r#"version 1
nodes
  0 "root" -1
  1 "tip" 0
end
skeleton
  time 0
    0 0.000000 0.000000 0.000000 0.000000 0.000000 0.000000
    1 0.000000 0.000000 8.000000 0.000000 0.000000 0.000000
end
triangles
myprop
  1 -8.000000 -8.000000 0.000000 0.000000 0.000000 1.000000 0.000000 0.000000 1 1 1.000000
  1 8.000000 -8.000000 0.000000 0.000000 0.000000 1.000000 1.000000 0.000000 1 1 1.000000
  1 0.000000 8.000000 0.000000 0.000000 0.000000 1.000000 0.500000 1.000000 1 1 1.000000
end
"#;

    /// 碰撞网格：**两根骨骼各一个盒子** ⟹ 两个 solid。
    ///
    /// 单 solid 时 `physicsbone` 天然全 0（与语料 2459/2459 一致），
    /// 证明不了「编排真的填了它」—— 所以这里必须有两个。
    fn collision_smd() -> String {
        let mut s = String::from("version 1\nnodes\n");
        s.push_str("0 \"root\" -1\n1 \"tip\" 0\n");
        s.push_str("end\nskeleton\ntime 0\n");
        s.push_str("0 0 0 0 0 0 0\n1 0 0 40 0 0 0\n");
        s.push_str("end\ntriangles\nphy\n");
        for bone in 0..2 {
            let z = bone as f32 * 40.0;
            let verts: [[f32; 3]; 8] = [
                [-5.0, -5.0, z - 5.0],
                [5.0, -5.0, z - 5.0],
                [5.0, 5.0, z - 5.0],
                [-5.0, 5.0, z - 5.0],
                [-5.0, -5.0, z + 5.0],
                [5.0, -5.0, z + 5.0],
                [5.0, 5.0, z + 5.0],
                [-5.0, 5.0, z + 5.0],
            ];
            let faces: [[usize; 3]; 12] = [
                [0, 1, 2],
                [0, 2, 3],
                [4, 6, 5],
                [4, 7, 6],
                [0, 4, 5],
                [0, 5, 1],
                [1, 5, 6],
                [1, 6, 2],
                [2, 6, 7],
                [2, 7, 3],
                [3, 7, 4],
                [3, 4, 0],
            ];
            for f in faces.iter() {
                for &vi in f {
                    let p = verts[vi];
                    s.push_str(&format!(
                        "{bone} {} {} {} 0 0 1 0 0 1 {bone} 1\n",
                        p[0], p[1], p[2]
                    ));
                }
            }
        }
        s.push_str("end\n");
        s
    }

    /// 带碰撞 SMD 的描述；`physics_smd` 决定 `[physics].smd` 写什么。
    fn desc_toml(physics_smd: &str) -> String {
        format!(
            r#"
[model]
name = "models/test/minimal.mdl"
surface_prop = "metal"

[physics]
smd = "{physics_smd}"
joints = true
mass = 100.0

[materials]
search_paths = ["models/test"]
textures = [{{ name = "models/test/myprop" }}]

[[bones]]
name = "root"

[[bones]]
name = "tip"
parent = "root"

[[bodyparts]]
name = "body"

[[bodyparts.models]]
smd = "a.smd"
"#
        )
    }

    /// 读出第 `i` 根骨骼的 `physicsbone`（`mstudiobone_t` `+0xAC`）。
    ///
    /// `bone_off` 是私有模块里的常量，所以这里用公开的
    /// [`off::BONE_OFFSET`] + [`BONE_SIZE`] 自算。
    fn physics_bone_at(mdl: &[u8], i: usize) -> i32 {
        let g = |at: usize| {
            i32::from_le_bytes(mdl[at..at + 4].try_into().unwrap())
        };
        let bone_off = g(off::BONE_OFFSET) as usize;
        g(bone_off + i * BONE_SIZE + 0xAC)
    }

    fn parse(toml: &str) -> ModelDesc {
        ModelDesc::from_toml(toml).expect("描述应能解析")
    }

    /// 同 [`desc_toml`]，但**去掉整个 `[physics]` 表**（无碰撞 SMD）。
    fn plain_toml() -> String {
        desc_toml("phys.smd").replace(
            "[physics]\nsmd = \"phys.smd\"\njoints = true\nmass = 100.0\n\n",
            "",
        )
    }

    /// 一个 `n×n` 的四边形网格（三角形数够多，缓存优化才有可重排的余地）。
    ///
    /// [`MESH_SMD`] 只有一个三角形 —— `meshopt::optimize_vertex_cache`
    /// 在它上面**没有可重排的空间**，所以拿它测 `optimize_vtx` 会假阴性。
    fn grid_smd(n: usize) -> String {
        let mut s = String::from("version 1\nnodes\n0 \"root\" -1\n1 \"tip\" 0\nend\n");
        s.push_str("skeleton\ntime 0\n0 0 0 0 0 0 0\n1 0 0 8 0 0 0\nend\ntriangles\nmyprop\n");
        let at = |(i, j): (usize, usize)| {
            [
                i as f32 * 4.0 - n as f32 * 2.0,
                j as f32 * 4.0 - n as f32 * 2.0,
                0.0f32,
            ]
        };
        let uv = [(0.0, 0.0), (1.0, 0.0), (1.0, 1.0), (0.0, 0.0), (1.0, 1.0), (0.0, 1.0)];
        for i in 0..n {
            for j in 0..n {
                let quad = [(i, j), (i + 1, j), (i + 1, j + 1), (i, j + 1)];
                // 两个三角形：0-1-2 与 0-2-3。
                for k in [0usize, 1, 2, 0, 2, 3] {
                    let p = at(quad[k]);
                    let (u, v) = uv[k];
                    s.push_str(&format!(
                        "1 {} {} {} 0 0 1 {u} {v} 1 1 1.000000\n",
                        p[0], p[1], p[2]
                    ));
                }
            }
        }
        s.push_str("end\n");
        s
    }

    /// ⭐ 编排**真的**把碰撞 SMD 接进了 `physicsbone`。
    ///
    /// 这是本模块存在的理由：第三方工具自己串管线时最常漏的一步。
    /// 漏了以后 `.mdl` / `.vvd` / `.dx90.vtx` 全对，**只有这个字段悄悄全 0**。
    ///
    /// 判据（对照官方 `rjd1`，见 `src/phy.rs` 的同名测试）：
    /// 两根骨骼各一个盒子 ⟹ `physicsbone = [0, 1]`。
    #[test]
    fn build_wires_the_collision_smd_into_physicsbone() {
        let d = tmpdir("pb");
        write(&d, "a.smd", MESH_SMD);
        write(&d, "phys.smd", &collision_smd());
        let desc = parse(&desc_toml("phys.smd"));

        let out = build(&desc, &d, PipelineOptions::default()).expect("应编译成功");

        assert_eq!(
            out.compiled.physics_bone.as_deref(),
            Some([0i32, 1].as_slice()),
            "IR 里的 physics_bone 应是 [0, 1]"
        );
        // 关键：它必须真的**落进 .mdl 字节**（`mdl_writer` 是唯一消费点）。
        assert_eq!(physics_bone_at(&out.mdl, 0), 0, "骨骼 0 的 physicsbone");
        assert_eq!(
            physics_bone_at(&out.mdl, 1),
            1,
            "骨骼 1 的 physicsbone 必须是 1 —— 全 0 说明碰撞 SMD 没接进来"
        );

        let _ = std::fs::remove_dir_all(&d);
    }

    /// 没有碰撞 SMD 时**不写**该字段（保持 0），与官方一致
    /// （实测单 solid 2459/2459 全 0）。
    #[test]
    fn build_leaves_physicsbone_alone_without_a_collision_smd() {
        let d = tmpdir("nopb");
        write(&d, "a.smd", MESH_SMD);
        let desc = parse(&plain_toml());

        let out = build(&desc, &d, PipelineOptions::default()).expect("应编译成功");

        assert!(out.compiled.physics_bone.is_none());
        assert!(out.phy.is_none(), "没有碰撞 SMD 就不该有 .phy");
        assert_eq!(physics_bone_at(&out.mdl, 0), 0);
        assert_eq!(physics_bone_at(&out.mdl, 1), 0);

        let _ = std::fs::remove_dir_all(&d);
    }

    /// 有碰撞 SMD ⟹ 产出 `.phy`，且 `checksum` 与 `.mdl` 同源（配对令牌）。
    #[test]
    fn build_produces_phy_only_with_a_collision_smd() {
        let d = tmpdir("phy");
        write(&d, "a.smd", MESH_SMD);
        write(&d, "phys.smd", &collision_smd());
        let desc = parse(&desc_toml("phys.smd"));

        let out = build(&desc, &d, PipelineOptions::default()).expect("应编译成功");

        let phy = out.phy.as_ref().expect("应有 .phy");
        crate::phy::check_invariants(phy).expect("自检应通过");
        // `.mdl` 头部 `+0x08` 就是 checksum。
        let mdl_checksum =
            i32::from_le_bytes(out.mdl[off::CHECKSUM..off::CHECKSUM + 4].try_into().unwrap());
        assert_eq!(mdl_checksum, out.checksum);
        assert_eq!(
            i32::from_le_bytes(out.vvd[0x08..0x0C].try_into().unwrap()),
            out.checksum,
            "VVD 的 checksum 必须与 MDL 相同"
        );

        let _ = std::fs::remove_dir_all(&d);
    }

    /// `write_files` 按模型名建目录，并把每个产物写到算出来的路径上。
    #[test]
    fn write_files_creates_dirs_and_writes_every_product() {
        let d = tmpdir("write");
        write(&d, "a.smd", MESH_SMD);
        write(&d, "phys.smd", &collision_smd());
        let desc = parse(&desc_toml("phys.smd"));
        let out = build(&desc, &d, PipelineOptions::default()).expect("应编译成功");

        // 输出目录**故意不预先建** —— 模型名里有 `models/test/` 两级。
        let out_root = d.join("nested/out");
        let paths = write_files(&out, &out_root).expect("应写出");

        assert_eq!(paths.mdl, out_root.join("models/test/minimal.mdl"));
        assert_eq!(paths.vvd, out_root.join("models/test/minimal.vvd"));
        assert_eq!(paths.vtx, out_root.join("models/test/minimal.dx90.vtx"));
        assert_eq!(
            paths.phy.as_deref(),
            Some(out_root.join("models/test/minimal.phy").as_path())
        );
        assert_eq!(std::fs::read(&paths.mdl).unwrap(), out.mdl);
        assert_eq!(std::fs::read(&paths.vvd).unwrap(), out.vvd);
        assert_eq!(std::fs::read(&paths.vtx).unwrap(), out.vtx);
        assert_eq!(std::fs::read(paths.phy.as_ref().unwrap()).unwrap(), out.phy.clone().unwrap());

        let _ = std::fs::remove_dir_all(&d);
    }

    /// 摘要的行数与内容 —— `mdlc.exe` 逐行原样输出，GUI 直接显示。
    ///
    /// 不断言具体字节数（会随实现变化），只断言**结构**。
    #[test]
    fn summary_lines_mirror_the_cli_output() {
        let d = tmpdir("summary");
        write(&d, "a.smd", MESH_SMD);
        let desc = parse(&plain_toml());
        let out = build(&desc, &d, PipelineOptions::default()).expect("应编译成功");
        let paths = out.paths(Path::new("out"));

        let lines = out.summary_lines(&paths);
        assert_eq!(lines.len(), 15, "没有 .phy 时 15 行：{lines:?}");
        assert_eq!(lines[0], "模型        models/test/minimal.mdl");
        assert!(lines[3].is_empty(), "第 4 行是空行");
        assert!(lines[9].is_empty(), "第 10 行是空行");
        assert_eq!(lines[14], "**编译成功**（各文件布局自检均通过）");

        // 有 `.phy` 时多一行，插在 VTX 之后、空行之前。
        write(&d, "phys.smd", &collision_smd());
        let with_phy = parse(&desc_toml("phys.smd"));
        let out = build(&with_phy, &d, PipelineOptions::default()).expect("应编译成功");
        let lines = out.summary_lines(&out.paths(Path::new("out")));
        assert_eq!(lines.len(), 16, "有 .phy 时 16 行：{lines:?}");
        assert!(lines[13].starts_with("PHY "), "第 14 行应是 PHY：{lines:?}");
        assert!(lines[14].is_empty());
        assert_eq!(lines[15], "**编译成功**（各文件布局自检均通过）");

        let _ = std::fs::remove_dir_all(&d);
    }

    /// 错误分类 → 退出码。`mdlc.exe` 的契约（既有探针按码判定）：
    /// 1 = 编译 / 写出 / 自检；2 = 路径解析 / 读文件 / 建目录 / 写文件。
    #[test]
    fn error_kinds_map_to_the_cli_exit_codes() {
        assert_eq!(PipelineErrorKind::Build.exit_code(), 1);
        assert_eq!(PipelineErrorKind::Io.exit_code(), 2);

        let compile = PipelineError::Compile(vec![CompileError::Message {
            at: "x".into(),
            message: "y".into(),
        }]);
        assert_eq!(compile.kind(), PipelineErrorKind::Build);
        // 解析碰撞 SMD 失败属 **Build**（文件读到了，内容不对）。
        assert_eq!(
            PipelineError::ParseCollisionSmd {
                path: PathBuf::from("c.smd"),
                source: SmdError {
                    line: 1,
                    message: "坏".into(),
                },
            }
            .kind(),
            PipelineErrorKind::Build
        );
        assert_eq!(
            PipelineError::WriteMdl(WriteError::Internal("w".into())).kind(),
            PipelineErrorKind::Build
        );
        // 读不到 / 建目录 / 写文件属 **Io**（退出码 2）。
        assert_eq!(
            PipelineError::ReadCollisionSmd {
                path: PathBuf::from("i.smd"),
                source: std::io::Error::new(std::io::ErrorKind::NotFound, "i"),
            }
            .kind(),
            PipelineErrorKind::Io
        );
        assert_eq!(
            PipelineError::CreateDir {
                path: PathBuf::from("d"),
                source: std::io::Error::new(std::io::ErrorKind::PermissionDenied, "d"),
            }
            .kind(),
            PipelineErrorKind::Io
        );
        assert_eq!(
            PipelineError::WriteFile {
                path: PathBuf::from("f"),
                source: std::io::Error::new(std::io::ErrorKind::PermissionDenied, "f"),
            }
            .kind(),
            PipelineErrorKind::Io
        );

        // 编译失败是多行（首行计数），其余是单行。
        let lines = compile.lines();
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0], "编译失败，1 处错误：");
        assert_eq!(lines[1], "  - x: y");
        assert_eq!(
            PipelineError::WriteMdl(WriteError::Internal("i".into())).lines(),
            vec!["错误：内部错误（请报告）：i"]
        );
        // `Display` 与 `lines()` 必须一致（CLI 走前者，GUI 走后者）。
        assert_eq!(compile.to_string(), lines.join("\n"));
    }

    /// ⭐ 类型化的意义：调用方能**拿到源错误本身**，而不只是一个字符串。
    ///
    /// 改造前这里只有 `Io(String)`，`ErrorKind::NotFound` 被 `format!` 扔掉，
    /// 调用方只能正则匹配中文前缀。这条测试把这个能力钉住。
    #[test]
    fn io_failures_expose_the_source_error_kind() {
        let err = PipelineError::ReadCollisionSmd {
            path: PathBuf::from("nope.smd"),
            source: std::io::Error::new(std::io::ErrorKind::NotFound, "系统找不到指定的文件。"),
        };
        // 文案与改造前逐字节相同。
        assert_eq!(
            err.to_string(),
            "错误：读不到碰撞 SMD nope.smd：系统找不到指定的文件。"
        );

        let source = std::error::Error::source(&err).expect("应能拿到源错误");
        let io = source
            .downcast_ref::<std::io::Error>()
            .expect("源错误应是 io::Error");
        assert_eq!(io.kind(), std::io::ErrorKind::NotFound);
    }

    /// 碰撞 SMD **读不到** ⟹ `Io`（退出码 2），不是编译错误。
    #[test]
    fn missing_collision_smd_is_an_io_error() {
        let d = tmpdir("missing");
        write(&d, "a.smd", MESH_SMD);
        let desc = parse(&desc_toml("nope.smd"));

        let err = build(&desc, &d, PipelineOptions::default()).expect_err("应失败");
        assert_eq!(err.kind(), PipelineErrorKind::Io, "{err}");
        assert!(
            err.to_string().contains("读不到碰撞 SMD"),
            "文案：{err}"
        );
        // 端到端也验证一次：源错误真的是 NotFound，而不是被压平的字符串。
        let io = std::error::Error::source(&err)
            .and_then(|s| s.downcast_ref::<std::io::Error>())
            .expect("应能 downcast 到 io::Error");
        assert_eq!(io.kind(), std::io::ErrorKind::NotFound);

        let _ = std::fs::remove_dir_all(&d);
    }

    /// 碰撞 SMD 读到了但**解析不了** ⟹ `Collision`（仍属 Build，退出码 1）。
    #[test]
    fn unparsable_collision_smd_is_a_build_error() {
        let d = tmpdir("badcollision");
        write(&d, "a.smd", MESH_SMD);
        write(&d, "phys.smd", "这不是 SMD\n");
        let desc = parse(&desc_toml("phys.smd"));

        let err = build(&desc, &d, PipelineOptions::default()).expect_err("应失败");
        assert_eq!(err.kind(), PipelineErrorKind::Build, "{err}");
        assert!(
            err.to_string().contains("解析碰撞 SMD"),
            "文案：{err}"
        );

        let _ = std::fs::remove_dir_all(&d);
    }

    /// 渲染 SMD 读不到 ⟹ `Compile`（`compile()` 自己的错误，退出码 1）。
    #[test]
    fn missing_render_smd_is_a_compile_error() {
        let d = tmpdir("norender");
        let desc = parse(&desc_toml("phys.smd"));

        let err = build(&desc, &d, PipelineOptions::default()).expect_err("应失败");
        assert!(matches!(err, PipelineError::Compile(_)), "{err}");
        assert_eq!(err.kind(), PipelineErrorKind::Build);

        let _ = std::fs::remove_dir_all(&d);
    }

    /// `optimize_vtx` 与 TOML 的 `[model] optimize_vtx` 是 **`||`** 关系。
    ///
    /// 判据用**产物字节**：优化开启后 VTX 的索引序会变（缓存友好重排），
    /// 所以两个开关各自都能让 VTX 与「全关」不同，且两者结果相同。
    #[test]
    fn optimize_vtx_is_ored_with_the_toml_flag() {
        let d = tmpdir("optvtx");
        // 三角形要够多，`meshopt::optimize_vertex_cache` 才有可重排的余地
        // —— 单个三角形上它会原样返回（假阴性）。
        write(&d, "a.smd", &grid_smd(6));
        // 用不带碰撞的描述 —— 这条测试只关心 VTX，不需要 `.phy`。
        let base = parse(&plain_toml());

        let off = build(&base, &d, PipelineOptions::default()).expect("应编译成功");
        let on = build(
            &base,
            &d,
            PipelineOptions {
                optimize_vtx: true,
            },
        )
        .expect("应编译成功");
        assert_ne!(off.vtx, on.vtx, "命令行开关应改变 VTX 索引序");

        // TOML 侧单独生效（命令行关着）—— 两者是同一条路径。
        let toml_on = parse(
            &plain_toml().replace(
                "surface_prop = \"metal\"",
                "surface_prop = \"metal\"\noptimize_vtx = true",
            ),
        );
        let from_toml = build(&toml_on, &d, PipelineOptions::default()).expect("应编译成功");
        assert_eq!(from_toml.vtx, on.vtx, "TOML 与命令行应是同一条路径");
        // 两侧都开 ⟹ 仍是同一条路径（`||` 不是「两个开关叠加两次」）。
        let both = build(
            &toml_on,
            &d,
            PipelineOptions {
                optimize_vtx: true,
            },
        )
        .expect("应编译成功");
        assert_eq!(both.vtx, on.vtx, "`||` 关系：两个都开等于开一个");

        let _ = std::fs::remove_dir_all(&d);
    }
}
