//! QC 语法层 —— token 流 → [`crate::model::ModelDesc`]。
//!
//! # 结构
//!
//! 逐条对应官方 `studiomdl.cpp` 的 `Cmd_*` / `Option_*`：
//!
//! | 本模块 | 官方 |
//! |---|---|
//! | [`Parser::run`] | `ParseScript`（`studiomdl.cpp:6737`） |
//! | [`Parser::cmd_body`] / [`Parser::cmd_bodygroup`] | `Cmd_Body` / `Cmd_Bodygroup`（`989`/`1034`） |
//! | [`Parser::option_studio`] | `Option_Studio`（`917`） |
//! | [`Parser::cmd_model`] | `Cmd_Model`（`4228`） |
//! | [`Parser::cmd_sequence`] | `Cmd_Sequence` + `ParseSequence`（`2593`/`2650`） |
//! | [`Parser::cmd_animation`] | `Cmd_Animation` + `ParseAnimation`（`2387`/`2442`） |
//! | [`Parser::parse_animation_token`] | `ParseAnimationToken`（`2185`） |
//!
//! # 两处**必须**做的「额外 I/O」
//!
//! 官方在 `SimplifyModel()` 里做、而 QC 文本里**看不到**的两件事：
//!
//! 1. **骨骼表**（`BuildGlobalBonetable`，`simplify.cpp:3616`）。
//!    QC 只在 `$definebone` 里显式声明骨骼，其余骨骼来自 SMD 的 `nodes`
//!    段。mdlc 的 `[[bones]]` 必须**完整列出**每根骨骼（`compile.rs` 会
//!    对未声明的骨骼报错），所以解析器要**读一遍 SMD** 收集骨骼名。
//! 2. **材质表**（`SetSkinValues` / `lookup_texture`）。
//!    QC 的 `$cdmaterials` 只给搜索目录；材质名来自 SMD 的三角形段。
//!    同理，`[[materials].textures]` 必须完整。
//!
//! 顺序规则（实测 `BuildGlobalBonetable`）：
//! **`$definebone` 的骨骼排在前面**（按书写序），**然后**才是各 SMD
//! 里「有顶点引用」的骨骼（按 source 序 × SMD 内下标序）。

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use super::lexer::{Lexer, Token};
use super::QcError;
use crate::model::*;

// ---------------------------------------------------------------------------
// 头部 flags 位（`studio.h` 的 `STUDIOHDR_FLAGS_*`）
// ---------------------------------------------------------------------------

/// `STUDIOHDR_FLAGS_STATIC_PROP`（`1<<4`）—— 由 `mdl_writer` 自动置位。
const FLAG_STATIC_PROP: i32 = 1 << 4;
/// `STUDIOHDR_FLAGS_NO_FORCED_FADE`（`1<<11`）—— `$noforcedfade`。
const FLAG_NO_FORCED_FADE: i32 = 1 << 11;
/// `STUDIOHDR_FLAGS_FORCE_PHONEME_CROSSFADE`（`1<<12`）—— `$forcephonemecrossfade`。
const FLAG_FORCE_PHONEME_CROSSFADE: i32 = 1 << 12;
/// `STUDIOHDR_FLAGS_CONSTANT_DIRECTIONAL_LIGHT_DOT`（`1<<13`）。
const FLAG_CONSTANT_DIRECTIONAL_LIGHT_DOT: i32 = 1 << 13;
/// `STUDIOHDR_FLAGS_AMBIENT_BOOST`（`1<<16`）—— `$ambientboost`。
const FLAG_AMBIENT_BOOST: i32 = 1 << 16;
/// `STUDIOHDR_FLAGS_DO_NOT_CAST_SHADOWS`（`1<<17`）。
const FLAG_DO_NOT_CAST_SHADOWS: i32 = 1 << 17;
/// `STUDIOHDR_FLAGS_CAST_TEXTURE_SHADOWS`（`1<<18`）。
const FLAG_CAST_TEXTURE_SHADOWS: i32 = 1 << 18;
/// `STUDIOHDR_FLAGS_FORCE_OPAQUE`（`1<<2`）—— `$opaque`。
const FLAG_OPAQUE: i32 = 1 << 2;
/// `STUDIOHDR_FLAGS_TRANSLUCENT_TWOPASS`（`1<<3`）—— `$mostlyopaque`。
const FLAG_MOSTLY_OPAQUE: i32 = 1 << 3;

/// 官方 `MAXSTUDIOSEQUENCES`（`studiomdl.h:28`，**1524**）。
///
/// `$declaresequence` 与 `$sequence` **共用**同一个 `g_sequence` 池，
/// 所以上限是两者之和。
///
/// 超出时官方报 `Too many sequences (1524 max)`。
///
/// ⚠️ 与 weightlist 的 16/32 不同，这个值**没实测**过 —— 要造 1525 条
/// 序列的 QC 成本高，而 survivor 的 937 条离上限还有距离。
/// 头文件的值在这里先按「够用」处理，标注为未实测。
const MAX_SEQUENCES: usize = 1524;

/// QC 解析器。
pub struct Parser<'a> {
    /// 词法器。
    pub lex: Lexer,
    /// 主 QC 路径（仅用于报错）。
    pub main_path: &'a Path,
    /// `qdir` —— 主 QC 所在目录。
    qdir: PathBuf,
    /// 累积中的描述。
    desc: ModelDesc,
    /// `$definebone` 的骨骼（顺序敏感，必须排在 SMD 骨骼之前）。
    import_bones: Vec<Bone>,
    /// `$cd` / `$pushd` 栈（`cddir[32]`）。元素是**已带结尾 `/`** 的前缀。
    cddir: Vec<String>,
    /// `$definevariable` 之外：`$modelname` 是否已出现。
    outname: Option<String>,
    /// `$scale` 的当前值（`g_defaultscale`）。
    default_scale: f32,
    /// 头部 flags 累加（`gflags`）。
    gflags: i32,
    /// `$staticprop`。
    static_prop: bool,
    /// `$contents` 的当前值（`s_nDefaultContents`，初值 `CONTENTS_SOLID`）。
    contents: i32,
    /// `$texturegroup` 的组（`[命令][family][组内第 j 项]` 的材质名）。
    ///
    /// 官方只支持一条 `$texturegroup`（`SetSkinValues` 只读 `[0]`），
    /// 但这里保留全部以便报错更清楚。
    texture_groups: Vec<Vec<Vec<String>>>,
    /// 已按「遇到顺序」注册的材质名（`g_texture[].name`）。
    texture_order: Vec<String>,
    /// 材质名 → `texture_order` 下标。
    texture_index: HashMap<String, usize>,
    /// 当前正在解析的 bodypart 下标。
    cur_bodypart: Option<usize>,
    /// 序列名集合（查重）。
    sequence_names: HashSet<String>,
    /// 动画名集合（查重）。
    animation_names: HashSet<String>,
    /// `$weightlist` 是否出现过（暂只记录，见 `PROGRESS.md` §1.1）。
    pub weightlists: Vec<String>,    /// 所有被引用的网格/动画文件（相对 qdir），供骨骼与材质扫描。
    referenced_files: Vec<String>,
    /// 解析出的错误（累积，最后一起报）。
    errors: Vec<QcError>,
    /// `$bonemerge` 的骨骼名（骨骼表建好后回填）。
    bonemerge_names: Vec<String>,
    /// `$attachment` 里哪些带 `absolute`（写出器需要按 `g_defaultrotation` 反转）。
    attachment_absolute: Vec<bool>,
    /// `$jointsurfaceprop` 的待办（骨骼表建好后回填）。
    joint_surface_props: Vec<(String, String)>,
    /// `$lod` 出现在任何 bodypart **之前**时的暂存。
    pending_lods: Vec<LodModel>,
    /// `$model` 块里 `flexfile` 设的当前 `.vta`（官方是**粘性变量**）。
    pending_vta: Option<String>,
    /// `$jigglebone` 子块的当前块名。
    last_block_name: Option<String>,
    /// **顶层** `$sectionframes <每段帧数> <阈值>` 的全局值。
    ///
    /// 官方是全局变量；mdlc 的 IR 是逐序列字段，所以在 `finish()` 里
    /// 回填给尚未显式设置的序列。
    global_section_frames: Option<(i32, i32)>,
}

impl<'a> Parser<'a> {
    /// 新建。
    pub fn new(lex: Lexer, main_path: &'a Path) -> Self {
        let qdir = lex.qdir.clone();
        Self {
            lex,
            main_path,
            qdir,
            desc: ModelDesc {
                model: ModelMeta {
                    name: String::new(),
                    version: None,
                    checksum: None,
                    static_prop: false,
                    surface_prop: None,
                    eye_position: None,
                    illum_position: None,
                    max_eye_deflection: None,
                    hull_min: None,
                    hull_max: None,
                    extra_flags: None,
                    contents: None,
                    skip_bone_in_bbox: false,
                    optimize_vtx: false,
                    key_values: None,
                    pose_parameters: Vec::new(),
                    realign_bones: false,
                    anim_block_size: None,
                },
                physics: Physics::default(),
                materials: Materials::default(),
                bones: Vec::new(),
                bodyparts: Vec::new(),
                hitboxes: Hitboxes::default(),
                attachments: Vec::new(),
                sequences: Vec::new(),
                animations: Vec::new(),
                bonecontrollers: Vec::new(),
                ikchains: Vec::new(),
                ik_autoplay_locks: Vec::new(),
                flex_descriptors: Vec::new(),
                flex_controllers: Vec::new(),
                flex_rules: Vec::new(),
                flex_controller_ui: Vec::new(),
                mouths: Vec::new(),
                jiggle_bones: Vec::new(),
                quat_interp_bones: Vec::new(),
                include_models: Vec::new(),
                weight_lists: Vec::new(),
            },
            import_bones: Vec::new(),
            cddir: vec![String::new()],
            outname: None,
            default_scale: 1.0,
            gflags: 0,
            static_prop: false,
            contents: 1,
            texture_groups: Vec::new(),
            texture_order: Vec::new(),
            texture_index: HashMap::new(),
            cur_bodypart: None,
            sequence_names: HashSet::new(),
            animation_names: HashSet::new(),
            weightlists: Vec::new(),
            referenced_files: Vec::new(),
            errors: Vec::new(),
            bonemerge_names: Vec::new(),
            attachment_absolute: Vec::new(),
            joint_surface_props: Vec::new(),
            pending_lods: Vec::new(),
            pending_vta: None,
            last_block_name: None,
            global_section_frames: None,
        }
    }

    /// 当前 `cddir` 前缀（`cddir[numdirs]`）。
    fn cd_prefix(&self) -> String {
        self.cddir.last().cloned().unwrap_or_default()
    }

    // ---------------------------------------------------------------
    // token 辅助
    // ---------------------------------------------------------------

    fn tok(&mut self, crossline: bool) -> Result<Token, QcError> {
        self.lex.expect_token(crossline)
    }

    /// 取一个浮点参数（官方 `verify_atof`）。
    fn f(&mut self) -> Result<f32, QcError> {
        let t = self.tok(false)?;
        parse_f32(&t.text).ok_or_else(|| {
            QcError::new(t.file.clone(), t.line, format!("期望数字，得到 {:?}", t.text))
        })
    }

    /// 取一个整数参数（官方 `verify_atoi`）。
    fn i(&mut self) -> Result<i32, QcError> {
        let t = self.tok(false)?;
        parse_i32(&t.text).ok_or_else(|| {
            QcError::new(t.file.clone(), t.line, format!("期望整数，得到 {:?}", t.text))
        })
    }

    /// 取三个浮点（一个 Vector）。
    fn v3(&mut self) -> Result<[f32; 3], QcError> {
        Ok([self.f()?, self.f()?, self.f()?])
    }

    /// 是否还有 token 在本行（`TokenAvailable`）。
    fn avail(&mut self) -> bool {
        self.lex.token_available()
    }

    // ---------------------------------------------------------------
    // 主循环（`ParseScript`）
    // ---------------------------------------------------------------

    /// 跑完整个脚本，产出描述。
    pub fn run(&mut self) -> Result<ModelDesc, Vec<QcError>> {
        while let Some(t) = self.lex.next_token(true).map_err(|e| vec![e])? {
            if let Err(e) = self.dispatch(&t) {
                self.errors.push(e);
                // 出错后不再继续（后续 token 边界不可信）。
                break;
            }
        }
        if !self.errors.is_empty() {
            return Err(std::mem::take(&mut self.errors));
        }
        self.finish()
    }

    /// 分发一条顶层命令。
    fn dispatch(&mut self, t: &Token) -> Result<(), QcError> {
        let name = t.text.to_ascii_lowercase();
        match name.as_str() {
            "$modelname" => self.cmd_modelname(),
            "$cd" => self.cmd_cd(),
            "$pushd" => self.cmd_pushd(),
            "$popd" => self.cmd_popd(),
            "$scale" => self.cmd_scale(),
            "$cdmaterials" => self.cmd_cdmaterials(),
            "$surfaceprop" => {
                let v = self.tok(false)?;
                self.desc.model.surface_prop = Some(v.text);
                Ok(())
            }
            "$contents" => self.cmd_contents(),
            "$eyeposition" => {
                let v = self.v3()?;
                self.desc.model.eye_position = Some(v);
                Ok(())
            }
            "$illumposition" => {
                let v = self.v3()?;
                self.desc.model.illum_position = Some(v);
                Ok(())
            }
            "$maxeyedeflection" => {
                let v = self.f()?;
                self.desc.model.max_eye_deflection = Some(v);
                Ok(())
            }
            "$bbox" => {
                let min = self.v3()?;
                let max = self.v3()?;
                self.desc.model.hull_min = Some(min);
                self.desc.model.hull_max = Some(max);
                Ok(())
            }
            "$cbox" => {
                // `$cbox` 写 `view_bb`；实测语料 0/3333 非零，这里只消费 token。
                let _ = self.v3()?;
                let _ = self.v3()?;
                Ok(())
            }
            "$staticprop" => {
                self.static_prop = true;
                self.gflags |= FLAG_STATIC_PROP;
                Ok(())
            }
            "$realignbones" => {
                self.desc.model.realign_bones = true;
                Ok(())
            }
            "$skipboneinbbox" => {
                self.desc.model.skip_bone_in_bbox = true;
                Ok(())
            }
            "$animblocksize" => {
                let v = self.i()?;
                self.desc.model.anim_block_size = Some(v);
                Ok(())
            }
            "$keyvalues" => {
                let text = self.option_keyvalues()?;
                self.desc.model.key_values = Some(text);
                Ok(())
            }
            "$sectionframes" => self.cmd_sectionframes_top(),
            "$poseparameter" => self.cmd_poseparameter(),
            "$texturegroup" => self.cmd_texturegroup(),
            "$body" => self.cmd_body(),
            "$bodygroup" => self.cmd_bodygroup(),
            "$model" => self.cmd_model(),
            "$sequence" => self.cmd_sequence(),
            "$animation" => self.cmd_animation(),
            "$definebone" => self.cmd_definebone(),
            "$bonemerge" => self.cmd_bonemerge(),
            "$attachment" => self.cmd_attachment(),
            "$hboxset" => self.cmd_hboxset(),
            "$hbox" => self.cmd_hbox(),
            "$ikchain" => self.cmd_ikchain(),
            "$ikautoplaylock" => self.cmd_ikautoplaylock(),
            "$includemodel" => self.cmd_includemodel(),
            "$lod" => self.cmd_lod(None),
            "$shadowlod" => self.cmd_lod(Some(0.0)),
            "$jigglebone" => self.cmd_jigglebone(),
            "$proceduralbones" => self.cmd_proceduralbones(),
            "$collisionmodel" => self.cmd_collisionmodel(false),
            "$collisionjoints" => self.cmd_collisionmodel(true),
            "$jointsurfaceprop" => self.cmd_jointsurfaceprop(),
            "$weightlist" => self.cmd_weightlist(),
            "$defaultweightlist" => self.cmd_defaultweightlist(),
            "$declaresequence" => self.cmd_declaresequence(),
            "$definevariable" | "$redefinevariable" => {
                // 词法层已消费（`scriplib.cpp` 在 `GetToken` 内部处理）。
                // 走到这里说明它作为**普通 token** 出现了 —— 官方也一样
                // （`GetToken` 只在词法阶段拦截；这里不会到达）。
                Ok(())
            }
            // ---- 已知但**故意不支持**的命令（见各分支注释）----
            "$fakevta" => self.skip_block_with_name("$fakevta"),
            "$nekomodel" => Err(self.lex.error(
                "$nekomodel 指向 DMX 源，mdlc 不实现 DMX（官方也委托 dmxconvert.exe）",
            )),
            "$jointconstrain" => self.cmd_jointconstrain(),
            "$animatedfriction" => self.cmd_animatedfriction(),
            "$noselfcollisions" => {
                self.desc.physics.no_self_collisions = true;
                Ok(())
            }
            "$jointcollide" => self.cmd_collision_pair(false),
            "$jointmerge" => self.cmd_collision_pair(true),
            "$mass" | "$masscenter" | "$automass" | "$maxconvexpieces" | "$phyname" | "$concave"
            | "$damping" | "$rotdamping" | "$inertia" | "$drag" | "$rootbone" => {
                self.cmd_physics_kv(&name)
            }
            // ---- 纯标志位（写进 `extra_flags`）----
            "$opaque" => self.set_flag(FLAG_OPAQUE),
            "$mostlyopaque" => self.set_flag(FLAG_MOSTLY_OPAQUE),
            "$noforcedfade" => self.set_flag(FLAG_NO_FORCED_FADE),
            "$casttextureshadows" => self.set_flag(FLAG_CAST_TEXTURE_SHADOWS),
            "$ambientboost" => self.set_flag(FLAG_AMBIENT_BOOST),
            "$donotcastshadows" => self.set_flag(FLAG_DO_NOT_CAST_SHADOWS),
            "$forcephonemecrossfade" => self.set_flag(FLAG_FORCE_PHONEME_CROSSFADE),
            "$constantdirectionallight" => {
                self.set_flag(FLAG_CONSTANT_DIRECTIONAL_LIGHT_DOT)?;
                let _ = self.f()?;
                Ok(())
            }
            // ---- 忽略（无产物痕迹 / 语料 0 次）----
            "$autocenter" | "$zbrush" | "$cliptotextures" | "$externaltextures" | "$obsolete"
            | "$minlod" | "$allowrootlods" | "$skinnedLODs" | "$motionrollback" | "$subd"
            | "$lcaseallsequences" | "$addsearchdir" | "$centerbonesonverts" | "$gamma"
            | "$hgroup" | "$decal" | "$ignorez" | "$vertexcolor" | "$unlockdefinebones"
            | "$lockbonelengths" | "$jigglebonerealign" | "$bonesaveframe"
            | "$declareanimation" | "$calctransitions" | "$skiptransition" | "$forcerealign"
            | "$collapsebones" | "$collapsebonesaggressive" | "$alwayscollapse" | "$screenalign"
            | "$renamematerial" | "$renamebone" | "$hierarchy" | "$heirarchy" | "$insertbone"
            | "$limitrotation" | "$controller" | "$root" | "$upaxis" | "$origin" | "$maxverts"
            | "$maxbones" | "$filebuffersize" | "$fakevta " => {
                self.skip_rest_of_line();
                Ok(())
            }
            other => Err(self.lex.error(format!("未知的 QC 命令 {other:?}（官方是 bad command）"))),
        }
    }

    /// 设置一个头部 flag 位。
    fn set_flag(&mut self, bit: i32) -> Result<(), QcError> {
        self.gflags |= bit;
        Ok(())
    }

    /// 消费本行剩余 token。
    fn skip_rest_of_line(&mut self) {
        while self.avail() {
            if self.lex.next_token(false).ok().flatten().is_none() {
                break;
            }
        }
    }

    /// 跳过 `$fakevta "<name>" { ... }` 这类「带名字 + 可选块」的命令。
    ///
    /// ⚠️ **不能先 `skip_rest_of_line()`** —— 那会把紧跟其后的 `{` 一起
    /// 吃掉，于是块体里的命令（`appendvta` 等）变成**顶层命令**，
    /// 报「未知的 QC 命令」。
    ///
    /// 实测 `f2.qc`：
    ///
    /// ```text
    /// $fakevta "fvanim" {
    ///     appendvta "vt4" 0
    /// }
    /// ```
    ///
    /// 官方 `Cmd_FakeVTA` 读名字后进块循环，`appendvta` **只在块内**
    /// 被识别（它是 T1 之外的命令，官方也是靠块内分支处理的）。
    /// 所以这里必须：读名字 → 逐 token 找 `{` → 跳到配对 `}`。
    fn skip_block_with_name(&mut self, _what: &str) -> Result<(), QcError> {
        // 名字（若本行还有 token）。
        if self.avail() {
            let _ = self.tok(false)?;
        }
        // 在**本行**找 `{`；找不到就说明没有块，本行结束。
        if !self.avail() {
            return Ok(());
        }
        let t = self.tok(false)?;
        if t.text != "{" {
            // 不是块开始 —— 把本行剩下的消费掉（可能是别的参数）。
            self.lex.unget(t);
            self.skip_rest_of_line();
            return Ok(());
        }
        // 跨行跳到配对的 `}`。
        let mut depth = 1;
        while depth > 0 {
            let Some(t) = self.lex.next_token(true)? else {
                break;
            };
            if t.text == "{" {
                depth += 1;
            } else if t.text == "}" {
                depth -= 1;
            }
        }
        Ok(())
    }

    // ---------------------------------------------------------------
    // 各命令
    // ---------------------------------------------------------------

    fn cmd_modelname(&mut self) -> Result<(), QcError> {
        let t = self.tok(false)?;
        // 官方：首字符是 `/` 或 `\` 时剥掉并警告。
        let name = if t.first() == b'/' || t.first() == b'\\' {
            t.text[1..].to_string()
        } else {
            t.text.clone()
        };
        self.outname = Some(name.clone());
        self.desc.model.name = name;
        Ok(())
    }

    fn cmd_cd(&mut self) -> Result<(), QcError> {
        let t = self.tok(false)?;
        self.cddir = vec![format!("{}/", t.text.trim_end_matches(['/', '\\']))];
        Ok(())
    }

    fn cmd_pushd(&mut self) -> Result<(), QcError> {
        let t = self.tok(false)?;
        let cur = self.cd_prefix();
        self.cddir
            .push(format!("{}{}/", cur, t.text.trim_end_matches(['/', '\\'])));
        Ok(())
    }

    fn cmd_popd(&mut self) -> Result<(), QcError> {
        if self.cddir.len() > 1 {
            self.cddir.pop();
        }
        Ok(())
    }

    fn cmd_scale(&mut self) -> Result<(), QcError> {
        let v = self.f()?;
        self.default_scale = v;
        Ok(())
    }

    fn cmd_cdmaterials(&mut self) -> Result<(), QcError> {
        while self.avail() {
            let t = self.tok(false)?;
            // 官方：末尾补 `/`（若没有），再 `Q_FixSlashes`。
            //
            // ⚠️ 空串**原样保留**（不补 `/`）—— 实测 `$cdmaterials ""`
            // 会产生一条空 cdtexture（受控实验 `cdexp{1,2,3}.qc`）。
            let s = if t.text.is_empty() {
                String::new()
            } else if t.text.ends_with('/') || t.text.ends_with('\\') {
                t.text.clone()
            } else {
                format!("{}/", t.text)
            };
            self.desc.materials.search_paths.push(s.replace('\\', "/"));
        }
        Ok(())
    }

    /// `$contents`（`ParseContents`，`studiomdl.cpp` 的 `Cmd_Contents`）。
    fn cmd_contents(&mut self) -> Result<(), QcError> {
        const CONTENTS_SOLID: i32 = 1;
        const CONTENTS_GRATE: i32 = 8;
        const CONTENTS_LADDER: i32 = 16;
        const CONTENTS_MONSTER: i32 = 32;
        let mut add = 0;
        let mut remove = 0;
        loop {
            let t = self.tok(false)?;
            match t.text.to_ascii_lowercase().as_str() {
                "grate" => {
                    add |= CONTENTS_GRATE;
                    remove |= CONTENTS_SOLID;
                }
                "ladder" => add |= CONTENTS_LADDER,
                "solid" => add |= CONTENTS_SOLID,
                "monster" => add |= CONTENTS_MONSTER,
                "notsolid" => remove |= CONTENTS_SOLID,
                _ => {}
            }
            if !self.avail() {
                break;
            }
        }
        self.contents = (self.contents | add) & !remove;
        Ok(())
    }

    /// `$keyvalues { ... }` —— 按官方 `Option_KeyValues` 原样重组文本。
    ///
    /// 官方把块内 token 重新拼成
    /// `mdlkeyvalue\n{\n` + 内容 + `}\n`，**第 2 层及更深**的 token 加引号。
    /// 这里产出**内层文本**（不含外层包装），由 `mdl_writer` 加包装
    /// —— 与 TOML 的 `key_values` 字段约定一致。
    fn option_keyvalues(&mut self) -> Result<String, QcError> {
        let t = self.tok(true)?;
        if t.text != "{" {
            return Ok(String::new());
        }
        let mut out = String::new();
        let mut level = 1;
        while let Some(t) = self.lex.next_token(true)? {
            if t.text == "}" {
                level -= 1;
                if level <= 0 {
                    break;
                }
                out.push_str(" }\n");
            } else if t.text == "{" {
                out.push_str("{\n");
                level += 1;
            } else if level > 1 {
                out.push('"');
                out.push_str(&t.text);
                out.push_str("\" ");
            } else {
                out.push_str(&t.text);
                out.push(' ');
            }
        }
        Ok(out)
    }

    /// **顶层** `$sectionframes <每段帧数> <阈值>`。
    ///
    /// # 为什么是顶层命令（而不是 `$sequence` 块内选项）
    ///
    /// 实测：`exp61.qc` / `f07.qc` / `t15_59.qc` 等都把它写在**顶层**
    /// （`$sequence` 之后另起一行），而 `$sequence` 的 `ParseSequence`
    /// 并不处理 `sectionframes` —— 它是 `Cmd_SectionFrames`
    /// （`studiomdl.cpp:1479` 附近的 `$animblocksize` 同一批全局命令）。
    ///
    /// # 作用域
    ///
    /// 官方把它设成**全局** `g_sectionframes` / `g_sectionframes_threshold`，
    /// 之后编译的**所有**序列都用它。mdlc 的 IR 是逐序列字段
    /// （`Sequence::section_frames`），所以这里记成「全局默认」，
    /// 在 `finish()` 里给**尚未显式设置**的序列回填。
    ///
    /// ⚠️ 官方在**写 `$sequence` 时**就读了全局值（`ParseSequence` 之后
    /// 在 `SimplifyModel` 里用），所以「`$sectionframes` 写在 `$sequence`
    /// 之后」对官方**无效**。mdlc 的逐序列字段天然表达不了这个时序 ——
    /// 实测语料里 23 个用例的写法都是「`$sequence` 之后写
    /// `$sectionframes`」，而官方产物**确实生效了**（`exp61` 等有产物），
    /// 所以这里按「生效」处理（与官方实测行为一致，而不是与源码时序一致）。
    fn cmd_sectionframes_top(&mut self) -> Result<(), QcError> {
        let len = self.i()?;
        let thr = if self.avail() { self.i()? } else { 120 };
        self.global_section_frames = Some((len, thr));
        Ok(())
    }

    /// `$poseparameter <名> <min> <max> [wrap | loop <值>]`。
    fn cmd_poseparameter(&mut self) -> Result<(), QcError> {
        let name = self.tok(false)?.text;
        let min = if self.avail() { self.f()? } else { 0.0 };
        let max = if self.avail() { self.f()? } else { 0.0 };
        let mut loop_mode = None;
        while self.avail() {
            let t = self.tok(false)?;
            match t.text.to_ascii_lowercase().as_str() {
                "wrap" => loop_mode = Some(PoseLoop::Wrap(PoseWrap::Wrap)),
                "loop" => {
                    let v = self.f()?;
                    loop_mode = Some(PoseLoop::Explicit(v));
                }
                _ => {}
            }
        }
        // ⚠️ 官方 `LookupPoseParameter` **不去重**（同名会新建一条）——
        // 实测 `ipp3.qc` 写两次 `$poseparameter "x"` 得到两条。
        self.desc.model.pose_parameters.push(PoseParameter {
            name,
            start: min,
            end: max,
            loop_mode,
        });
        Ok(())
    }

    /// `$texturegroup "name" { { "a" "b" } { "a2" "b2" } }`。
    ///
    /// 只记录**组结构**；纹理顺序与 skin 表在 `finish()` 里按
    /// `SetSkinValues` 推导（那时才知道 SMD 里的材质）。
    ///
    /// # 语义（`Cmd_TextureGroup`，`studiomdl.cpp:4740`）
    ///
    /// 外层 `{}` 是命令块（`depth` 1），**内层每个 `{}` 是一个 family**
    /// （`depth` 2）。官方在 `depth == 2` 时把 token 当材质名注册，
    /// 遇到 `}` 时 `group++`（换下一个 family）。
    ///
    /// 每个 family 是「一组材质名」，下标 `j` 是**组内位置** ——
    /// `SetSkinValues` 用它把「family 0 的第 j 个槽位」重映射到
    /// 「family i 的第 j 个材质」。
    fn cmd_texturegroup(&mut self) -> Result<(), QcError> {
        if self.avail() {
            let _ = self.tok(false)?; // 组名（官方只当注释用）
        }
        let mut groups: Vec<Vec<String>> = Vec::new();
        let mut depth = 0i32;
        while let Some(t) = self.lex.next_token(true)? {
            if t.text == "{" {
                depth += 1;
                if depth == 2 {
                    groups.push(Vec::new());
                }
            } else if t.text == "}" {
                depth -= 1;
                if depth == 0 {
                    break;
                }
            } else if depth == 2 {
                if let Some(g) = groups.last_mut() {
                    g.push(t.text);
                }
            }
        }
        // 官方只支持**一个** `$texturegroup`（`g_numtexturegroups` 递增，
        // 但 `SetSkinValues` 只读 `[0]`）。这里保留全部以便报错更清楚，
        // 但 `finish()` 只用第一个 —— 与官方一致。
        self.texture_groups.push(groups);
        Ok(())
    }

    /// `$body <name> <smd> [opts]`（`Cmd_Body`，`studiomdl.cpp:1034`）。
    fn cmd_body(&mut self) -> Result<(), QcError> {
        let name = self.tok(false)?.text;
        let base = self.next_bodypart_base();
        let mut bp = BodyPart {
            name,
            base: Some(base),
            models: Vec::new(),
        };
        let m = self.option_studio(None)?;
        bp.models.push(m);
        self.desc.bodyparts.push(bp);
        self.cur_bodypart = Some(self.desc.bodyparts.len() - 1);
        Ok(())
    }

    /// `$bodygroup <name> { studio "x.smd" blank }`（`Cmd_Bodygroup`，`989`）。
    fn cmd_bodygroup(&mut self) -> Result<(), QcError> {
        let name = self.tok(false)?.text;
        let base = self.next_bodypart_base();
        let mut bp = BodyPart {
            name,
            base: Some(base),
            models: Vec::new(),
        };
        loop {
            let t = self.tok(true)?;
            if t.text == "{" {
                continue;
            }
            if t.text == "}" {
                break;
            }
            match t.text.to_ascii_lowercase().as_str() {
                "studio" => {
                    let m = self.option_studio(None)?;
                    bp.models.push(m);
                }
                "blank" => {
                    // `Option_Blank`：一个空 model。mdlc 用「无网格的 model」
                    // 表达 —— 但 `BodyModel.smd` 是必填，所以这里报错更诚实。
                    return Err(QcError::new(
                        t.file.clone(),
                        t.line,
                        "`blank` bodygroup 成员：mdlc 的 BodyModel 必须有 SMD，暂不支持 blank",
                    ));
                }
                other => {
                    return Err(QcError::new(
                        t.file.clone(),
                        t.line,
                        format!("未知的 $bodygroup 选项 {other:?}"),
                    ));
                }
            }
        }
        self.desc.bodyparts.push(bp);
        self.cur_bodypart = Some(self.desc.bodyparts.len() - 1);
        Ok(())
    }

    /// bodypart 的 `base`（`g_bodypart[n].base = prev.base * prev.nummodels`）。
    fn next_bodypart_base(&self) -> i32 {
        match self.desc.bodyparts.last() {
            None => 1,
            Some(prev) => {
                let b = prev.base.unwrap_or(1);
                b * prev.models.len().max(1) as i32
            }
        }
    }

    /// `Option_Studio`（`studiomdl.cpp:917`）：读文件名 + 行内选项。
    ///
    /// `name_override` 给 `$model` 用（它把 bodypart 名也当 model 名）。
    fn option_studio(&mut self, name_override: Option<String>) -> Result<BodyModel, QcError> {
        let t = self.tok(false)?;
        let filename = t.text.clone();
        let mut model_name = name_override;

        // 行内选项。
        while self.avail() {
            let o = self.tok(false)?;
            match o.text.to_ascii_lowercase().as_str() {
                "reverse" => {}
                "scale" => {
                    let _ = self.f()?;
                }
                "faces" | "bias" => {
                    let _ = self.tok(false)?;
                }
                "{" => {
                    self.lex.unget(o);
                    break;
                }
                other => {
                    return Err(QcError::new(
                        o.file.clone(),
                        o.line,
                        format!("未知的 studio 选项 {other:?}"),
                    ));
                }
            }
        }

        let smd = self.resolve_src(&filename);
        self.referenced_files.push(smd.clone());
        Ok(BodyModel {
            smd,
            name: model_name.take(),            lods: Vec::new(),
            eyeballs: Vec::new(),
            flexes: Vec::new(),
        })
    }

    /// 把 QC 里的文件名解析成「相对 qdir 的路径字符串」，并拼上 `cddir` 前缀。
    ///
    /// 官方用 `cddir[numdirs] + name` 拼路径，而 mdlc 的路径字段是
    /// 「相对描述文件所在目录」—— 所以这里把前缀**拼进字符串**，
    /// 写出器就不需要知道 `$pushd` 存在。
    fn resolve_src(&self, name: &str) -> String {
        let p = Path::new(name);
        if p.is_absolute() {
            return name.to_string();
        }
        let prefix = self.cd_prefix();
        if prefix.is_empty() {
            name.replace('\\', "/")
        } else {
            format!("{}{}", prefix, name.replace('\\', "/"))
        }
    }

    /// `$model <name> <smd> [opts] { ...flex/eyeball/mouth... }`（`4228`）。
    fn cmd_model(&mut self) -> Result<(), QcError> {
        let name = self.tok(false)?.text;
        let base = self.next_bodypart_base();
        let m = self.option_studio(Some(name.clone()))?;
        let mut bp = BodyPart {
            name,
            base: Some(base),
            models: vec![m],
        };

        // 块内选项（`depth` 语义与官方一致：`{` 之后 depth>0 时跨行取 token）。
        let mut depth = 0i32;
        loop {
            let t = if depth > 0 {
                match self.lex.next_token(true)? {
                    Some(t) => t,
                    None => break,
                }
            } else {
                if !self.avail() {
                    break;
                }
                self.tok(false)?
            };
            if t.text == "{" {
                depth += 1;
                continue;
            }
            if t.text == "}" {
                depth -= 1;
                if depth <= 0 {
                    break;
                }
                continue;
            }
            let low = t.text.to_ascii_lowercase();
            let model = bp.models.last_mut().expect("$model 必有一个 model");
            match low.as_str() {
                "flexfile" => {
                    let v = self.tok(false)?.text;
                    self.pending_vta = Some(self.resolve_src(&v));
                }
                "flex" => {
                    let n = self.tok(false)?.text;
                    // ⚠️ `$model <名> <smd> flex "vanim" "vt2"` 的**内联形式**：
                    // 本行还有一个 token 时它就是 `.vta` 路径
                    // （官方 `depth == 0` 时读第二个 token 当 `vtafile`）。
                    // 实测 `sc3.qc` / `t01.qc` 用的是这个写法。
                    if depth == 0 && self.avail() {
                        let v = self.tok(false)?.text;
                        self.pending_vta = Some(self.resolve_src(&v));
                    }
                    let opts = self.flex_options(0.0)?;
                    self.push_flex(model, n, opts, false)?;
                }
                "flexpair" => {
                    let n = self.tok(false)?.text;
                    let split = self.f()?;
                    if depth == 0 && self.avail() {
                        let v = self.tok(false)?.text;
                        self.pending_vta = Some(self.resolve_src(&v));
                    }
                    let opts = self.flex_options(split)?;
                    self.push_flex(model, n, opts, true)?;
                }
                "defaultflex" => {
                    if depth == 0 && self.avail() {
                        let v = self.tok(false)?.text;
                        self.pending_vta = Some(self.resolve_src(&v));
                    }
                    let opts = self.flex_options(0.0)?;
                    self.push_flex(model, "default".to_string(), opts, false)?;
                }
                "localvar" => {
                    while self.avail() {
                        let t = self.tok(false)?;
                        self.add_flexdesc(&t.text);
                    }
                }
                "eyeball" => self.option_eyeball(model)?,
                "eyelid" => self.option_eyelid(model)?,
                "mouth" => self.option_mouth(model)?,
                "flexcontroller" => self.option_flexcontroller()?,
                "spherenormals" | "attachment" => {
                    self.skip_rest_of_line();
                }
                other => {
                    if let Some(rest) = other.strip_prefix('%') {
                        // `%<flex> = <表达式>`
                        //
                        // ⚠️ 先把两张名字表**取出来**再调 —— `parse_flexrule`
                        // 需要 `&mut self.lex`，同时又要 `&self.desc` 里的名字，
                        // 直接借用会冲突。
                        let controllers = self.flex_controller_names();
                        let flexdescs = self.flex_desc_names();
                        let rest = rest.to_string();
                        let rule = super::flexrule::parse_flexrule(
                            &mut self.lex,
                            &rest,
                            &controllers,
                            &flexdescs,
                        )?;
                        self.desc.flex_rules.push(rule);
                    } else {
                        return Err(QcError::new(
                            t.file.clone(),
                            t.line,
                            format!("未知的 model 选项 {other:?}"),
                        ));
                    }
                }
            }
        }

        self.desc.bodyparts.push(bp);
        self.cur_bodypart = Some(self.desc.bodyparts.len() - 1);
        Ok(())
    }

    /// `Option_Flex` 的后缀选项：`frame` / `position` / `split` / `decay`。
    ///
    /// 官方在一个 `while (TokenAvailable())` 里消费它们，**顺序任意、
    /// 个数任意**。返回 `(frame, position, split, decay)`。
    ///
    /// 默认值（官方 `Option_Flex` 初始化）：
    /// `frame = 0`、`target1`(=position) = **1.0**、`split` = 传入的
    /// `pairsplit`、`decay` = **1.0**。
    fn flex_options(&mut self, split_default: f32) -> Result<FlexOpts, QcError> {
        let mut o = FlexOpts {
            frame: 0,
            position: 1.0,
            split: split_default,
            decay: 1.0,
        };
        while self.avail() {
            let t = self.tok(false)?;
            match t.text.to_ascii_lowercase().as_str() {
                "frame" => o.frame = self.i()?,
                "position" => o.position = self.f()?,
                "split" => o.split = self.f()?,
                "decay" => o.decay = self.f()?,
                other => {
                    // 官方这里 `TokenError("unknown option: %s")` ——
                    // 报错。实测 `t01.qc` 的 `flex "vanim" "vt2"` 就是被
                    // 这条拦住的（`"vt2"` 不是合法选项）。
                    return Err(QcError::new(
                        t.file.clone(),
                        t.line,
                        format!("未知的 flex 选项 {other:?}（官方是 unknown option）"),
                    ));
                }
            }
        }
        Ok(o)
    }

    /// `Option_Flex`：把一条 VTA 形状绑定推入当前 model。
    ///
    /// # 官方形态（`Option_Flex`，`studiomdl.cpp`）
    ///
    /// 支持三个后缀选项：`frame <n>` / `position <f>` / `split <f>` /
    /// `decay <f>`。`position` 落进 `target1`（即 TOML 的 `position`），
    /// `decay` 落进 `decay`。
    ///
    /// # ⚠️ 粘性 `vtafile` 的真实行为
    ///
    /// `Cmd_Model` 里 `char FAC[256], vtafile[256];` 是**栈上未初始化**的局部
    /// 数组，`flexfile` 只是给它赋值。所以：
    ///
    /// * `flexfile "x"` 之后，后续 `flex` / `flexpair` 复用 `"x"`；
    /// * **从未出现 `flexfile`** 时，`vtafile` 是**未初始化栈残留**
    ///   —— 官方行为不确定（实测 `t01.qc` 仍能产出 `.mdl`，
    ///   说明残留值恰好不是合法路径或官方容忍了加载失败）。
    ///
    /// mdlc **不复制未初始化行为**（那是 UB，无法复现）。
    /// 实测语料里这种写法的 29 个用例中，**只有 8 个有官方产物**，
    /// 其余 21 个官方自己也没产出 —— 说明它们本来就是无效 QC。
    /// 所以这里**显式报错**，并指出「缺 `flexfile`」这个真实原因。
    fn push_flex(
        &mut self,
        model: &mut BodyModel,
        name: String,
        opts: FlexOpts,
        pair: bool,
    ) -> Result<(), QcError> {
        let Some(vta) = self.pending_vta.clone() else {
            return Err(self.lex.error(format!(
                "`flex \"{name}\"` 之前没有 `flexfile` —— 官方此处用的是\
                 **未初始化**的 `vtafile`（UB，不可复现）。\
                 请在 `flex` 前写一行 `flexfile \"<x.vta>\"`，\
                 或用 `$model <名> <smd> flex \"<名>\" \"<vta>\"` 的内联形式。"
            )));
        };
        model.flexes.push(Flex {
            vta,
            name,
            frame: opts.frame,
            pair,
            split: opts.split,
            position: opts.position,
            decay: opts.decay,
        });
        Ok(())
    }

    /// 注册一个 flexdesc（官方 `Add_Flexdesc`）。
    fn add_flexdesc(&mut self, name: &str) {
        if self
            .desc
            .flex_descriptors
            .iter()
            .any(|d| d.name.eq_ignore_ascii_case(name))
        {
            return;
        }
        self.desc.flex_descriptors.push(FlexDescriptor {
            name: name.to_string(),
        });
    }

    fn flex_desc_names(&self) -> Vec<String> {
        self.desc
            .flex_descriptors
            .iter()
            .map(|d| d.name.clone())
            .collect()
    }

    fn flex_controller_names(&self) -> Vec<String> {
        self.desc
            .flex_controllers
            .iter()
            .map(|c| c.name.clone())
            .collect()
    }

    /// `flexcontroller <类型> [range <min> <max>] <名> [<名> ...]`。
    ///
    /// # 语义（`Option_Flexcontroller`，`studiomdl.cpp`）
    ///
    /// 第一个 token 是**类型**；之后逐个 token 扫：
    ///
    /// * `range <min> <max>` —— 更新**后续**控制器共用的范围；
    /// * 其它 token —— 就是**一个控制器名**，用**当前**范围注册。
    ///
    /// ⚠️ 所以一行可以注册**多个**控制器（实测 `fx2.qc`：
    /// `flexcontroller lid range 0 1 right_lid_raiser right_lid_droop`
    /// 注册 2 个）。早先按「一个类型 + 一个名字 + 两个数字」解析是错的。
    fn option_flexcontroller(&mut self) -> Result<(), QcError> {
        let kind = self.tok(false)?.text;
        let mut range_min = 0.0f32;
        let mut range_max = 1.0f32;
        while self.avail() {
            let t = self.tok(false)?;
            if t.text.eq_ignore_ascii_case("range") {
                range_min = self.f()?;
                range_max = self.f()?;
            } else {
                self.desc.flex_controllers.push(FlexController {
                    name: t.text,
                    kind: kind.clone(),
                    min: range_min,
                    max: range_max,
                });
            }
        }
        Ok(())
    }

    /// `eyeball <名> <骨骼> <x> <y> <z> "<材质>" <半径> <zangle> "<虹膜材质>" <pupil_scale>`。
    fn option_eyeball(&mut self, model: &mut BodyModel) -> Result<(), QcError> {
        let name = self.tok(false)?.text;
        let bone = self.tok(false)?.text;
        let org = self.v3()?;
        let material = self.tok(false)?.text;
        let radius = self.f()?;
        let zangle = self.f()?;
        let iris_material = self.tok(false)?.text;
        let pupil_scale = self.f()?;
        // 官方把 `zangle` 转成 `tan(deg2rad(zangle))`、`pupil_scale` 转成
        // `1/pupil_scale`（见 `model.rs` 的 `Eyeball` 文档）。
        let _ = iris_material;
        model.eyeballs.push(Eyeball {
            name: Some(name),
            bone,
            org,
            material,
            radius,
            zoffset: zangle.to_radians().tan(),
            iris_scale: if pupil_scale == 0.0 {
                1.0
            } else {
                1.0 / pupil_scale
            },
            upper_lid: None,
            lower_lid: None,
        });
        Ok(())
    }

    /// `eyelid <名> <lowerer> <l1> <neutral> <n1> <raiser> <r1> <眼>...`。
    fn option_eyelid(&mut self, model: &mut BodyModel) -> Result<(), QcError> {
        let lid_name = self.tok(false)?.text;
        let mut vals: Vec<(String, f32)> = Vec::new();
        for _ in 0..3 {
            let d = self.tok(false)?.text;
            let v = self.f()?;
            vals.push((d, v));
        }
        let mut eyes = Vec::new();
        while self.avail() {
            eyes.push(self.tok(false)?.text);
        }
        // 把这条 eyelid 挂到它引用的每个 eyeball 上。
        //
        // ⚠️ 官方 `Option_Eyelid` 用 `LookupEyeball(name)` 找眼球；
        // 找不到就报错。这里照做。
        for e in &eyes {
            let Some(eb) = model
                .eyeballs
                .iter_mut()
                .find(|x| x.name.as_deref().is_some_and(|n| n.eq_ignore_ascii_case(e)))
            else {
                return Err(self.lex.error(format!("eyelid 引用了不存在的眼球 {e:?}")));
            };
            let lid = EyeballLid {
                lid_flexdesc: lid_name.clone(),
                lowerer: EyeballLidEntry {
                    flexdesc: vals[0].0.clone(),
                    target: vals[0].1,
                },
                neutral: EyeballLidEntry {
                    flexdesc: vals[1].0.clone(),
                    target: vals[1].1,
                },
                raiser: EyeballLidEntry {
                    flexdesc: vals[2].0.clone(),
                    target: vals[2].1,
                },
            };
            // `upper`/`lower` 由名字区分（官方也是这么做的）。
            if lid_name.to_ascii_lowercase().contains("upper") {
                eb.upper_lid = Some(lid);
            } else {
                eb.lower_lid = Some(lid);
            }
        }
        Ok(())
    }

    /// `mouth <index> "<名>" "<骨骼>" <fx> <fy> <fz>`。
    fn option_mouth(&mut self, model: &mut BodyModel) -> Result<(), QcError> {
        let _ = model;
        let index = self.i()?;
        let flexdesc = self.tok(false)?.text;
        let bone = self.tok(false)?.text;
        let forward = self.v3()?;
        self.desc.mouths.push(Mouth {
            index,
            flexdesc,
            bone,
            forward,
        });
        Ok(())
    }

    /// `$declaresequence "<名>"` —— 前向声明的**空壳序列**。
    ///
    /// # 官方（`Cmd_DeclareSequence`，`studiomdl.cpp:3204-3218`）
    ///
    /// ```c
    /// if (g_sequence.Count() >= MAXSTUDIOSEQUENCES)
    ///     TokenError("Too many sequences (%d max)\n", MAXSTUDIOSEQUENCES );
    /// s_sequence_t *pseq = &g_sequence[ g_sequence.AddToTail() ];
    /// memset( pseq, 0, sizeof( s_sequence_t ) );
    /// pseq->flags = STUDIO_OVERRIDE;
    /// GetToken( false );
    /// strcpyn( pseq->name, token );
    /// ```
    ///
    /// 三件事：占槽、清零、置 `STUDIO_OVERRIDE`（0x0800）。
    /// 它**不**分配 `panim`、不读 SMD、不建动画。
    ///
    /// # 为什么 survivor 模组靠它（用户问的正是这个）
    ///
    /// 引擎在 `studio_virtualmodel.cpp:185` 做**跨模型序列替换**：
    /// 主模型里的空壳会被 `$includemodel` 进来的模型**按名字替换**。
    /// 于是「网格 + 骨骼 + flex」与「几百条动画」可以**分开发布**。
    ///
    /// 实测真实工程 `linnea_replaces_zoey`：
    /// `Zoey_$DeclareSequence.qci` 935 行**全是** `$DeclareSequence`，
    /// 编出的 `survivor_teenangst.mdl` 有 937 条序列、
    /// 其中 **933 条是 `STUDIO_OVERRIDE` 空壳**。
    ///
    /// # 与 `$sequence` 的关系（实测的硬约束）
    ///
    /// 声明之后**不能**再用 `$sequence <同名>` 去填 —— 官方报
    /// `no animations found`（`ParseSequence` 末尾检查 `numblends == 0`）。
    /// 所以这里只占名字、不建序列体，填充交给引擎。
    ///
    /// ⚠️ 但**反过来**可以：先 `$sequence` 再 `$declaresequence` 同名，
    /// 官方会产出**两条**（第二条是空壳），不报错。这是实测行为，
    /// 所以查重只在「两边都是实体序列」时报。
    fn cmd_declaresequence(&mut self) -> Result<(), QcError> {
        let name_t = self.tok(false)?;
        let name = name_t.text.clone();
        // 官方上限 `MAXSTUDIOSEQUENCES`。**注意它数的是
        // `g_sequence.Count()`** —— 声明与实体序列**共用**同一个池，
        // 所以这里要数两者之和。
        if self.desc.sequences.len() >= MAX_SEQUENCES {
            return Err(QcError::new(
                name_t.file.clone(),
                name_t.line,
                format!("序列过多：官方上限 {MAX_SEQUENCES} 条"),
            ));
        }
        // 与实体序列重名 ⟹ 官方**不报错**，产出两条（实测）。
        // 但 `sequence_names` 是「实体序列已占用」的集合，空壳**不**写进去
        // —— 否则后面一条正常 `$sequence` 会被我们误报为重复，
        // 而官方在那种情况下是 `no animations found`（不同错误）。
        self.desc.sequences.push(Sequence {
            name,
            smd: String::new(),
            fps: None,
            looping: false,
            delta: false,
            activity: None,
            activity_weight: 0,
            events: Vec::new(),
            // ⚠️ 官方 `memset` 之后这些字段**保持 0**，不是普通序列的
            // 默认值（`fade_in/out = 0.2`、`activity = -1`）。
            fade_in: 0.0,
            fade_out: 0.0,
            forward_declared: true,
            no_auto_ik: false,
            ik_rules: Vec::new(),
            iklocks: Vec::new(),
            blends: Vec::new(),
            blend_width: None,
            blend_params: Vec::new(),
            auto_layers: Vec::new(),
            movements: Vec::new(),
            section_frames: None,
            section_threshold: None,
            extra_flags: None,
            weight_list: None,
        });
        Ok(())
    }

    /// `$sequence`（`Cmd_Sequence` + `ParseSequence`）。
    fn cmd_sequence(&mut self) -> Result<(), QcError> {
        let name_t = self.tok(false)?;
        let name = name_t.text.clone();
        if !self.sequence_names.insert(name.to_ascii_lowercase()) {
            return Err(QcError::new(
                name_t.file.clone(),
                name_t.line,
                format!("重复的序列名 {name:?}（官方是 Duplicate sequence name）"),
            ));
        }

        let mut seq = Sequence {
            name: name.clone(),
            smd: String::new(),
            fps: None,
            looping: false,
            delta: false,
            activity: None,
            activity_weight: 0,
            events: Vec::new(),
            fade_in: 0.2,
            fade_out: 0.2,
            forward_declared: false,
            no_auto_ik: false,
            ik_rules: Vec::new(),
            iklocks: Vec::new(),
            blends: Vec::new(),
            blend_width: None,
            blend_params: Vec::new(),
            auto_layers: Vec::new(),
            movements: Vec::new(),
            section_frames: None,
            section_threshold: None,
            extra_flags: None,
            weight_list: None,
        };

        let mut depth = 0i32;
        // 本序列引用的动画名（blend 网格，行主序）。
        let mut blend_names: Vec<String> = Vec::new();
        loop {
            let t = if depth > 0 {
                match self.lex.next_token(true)? {
                    Some(t) => t,
                    None => break,
                }
            } else {
                if !self.avail() {
                    break;
                }
                self.tok(false)?
            };
            if t.text == "{" {
                depth += 1;
                continue;
            }
            if t.text == "}" {
                depth -= 1;
                if depth <= 0 {
                    break;
                }
                continue;
            }
            let low = t.text.to_ascii_lowercase();
            match low.as_str() {
                "fps" => seq.fps = Some(self.f()?),
                "loop" => seq.looping = true,
                "delta" => seq.delta = true,
                "predelta" => seq.delta = true,
                "noautoik" => seq.no_auto_ik = true,
                "autoik" => seq.no_auto_ik = false,
                "snap" | "hidden" | "autoplay" | "post" | "realtime" | "worldspace" => {
                    // 纯标志位关键字 —— `ParseSequence` 逐个
                    // `pseq->flags |= XXX`（`studiomdl.cpp:2720-2866`）。
                    // 位值见 `studio.h` 的 `STUDIO_*`。
                    let bit = match low.as_str() {
                        "snap" => 0x0002,      // STUDIO_SNAP
                        "autoplay" => 0x0008,  // STUDIO_AUTOPLAY
                        "post" => 0x0010,      // STUDIO_POST
                        "realtime" => 0x0080,  // STUDIO_REALTIME
                        "hidden" => 0x0400,    // STUDIO_HIDDEN
                        "worldspace" => 0x2000, // STUDIO_WORLD
                        _ => 0,
                    };
                    seq.extra_flags = Some(seq.extra_flags.unwrap_or(0) | bit);
                }
                "fadein" => seq.fade_in = self.f()?,
                "fadeout" => seq.fade_out = self.f()?,
                "activity" => {
                    let a = self.tok(false)?.text;
                    let w = if self.avail() { self.i()? } else { 0 };
                    seq.activity = Some(a);
                    seq.activity_weight = w;
                }
                "blendwidth" => seq.blend_width = Some(self.i()?),
                "blend" => {
                    let parameter = self.tok(false)?.text;
                    let start = self.f()?;
                    let end = self.f()?;
                    // 官方在解析 blend 时还会放宽 pose parameter 的 min/max。
                    if let Some(pi) = self
                        .desc
                        .model
                        .pose_parameters
                        .iter_mut()
                        .find(|p| p.name.eq_ignore_ascii_case(&parameter))
                    {
                        pi.start = pi.start.min(start).min(end);
                        pi.end = pi.end.max(start).max(end);
                    }
                    seq.blend_params.push(BlendParam {
                        parameter,
                        start,
                        end,
                    });
                }
                "addlayer" => {
                    let s = self.tok(false)?.text;
                    seq.auto_layers.push(AutoLayer {
                        sequence: s,
                        pose: 0,
                        flags: 0,
                        start: 0.0,
                        peak: 0.0,
                        tail: 0.0,
                        end: 0.0,
                    });
                }
                "event" => {
                    let (ev, consumed_depth) = self.option_event(&seq.name)?;
                    seq.events.push(ev);
                    depth -= consumed_depth;
                }
                "ikrule" => {
                    let r = self.option_ikrule()?;
                    seq.ik_rules.push(r);
                }
                "iklock" => {
                    let chain = self.tok(false)?.text;
                    let pos_weight = self.f()?;
                    let local_q_weight = self.f()?;
                    seq.iklocks.push(IkAutoplayLock {
                        chain,
                        pos_weight,
                        local_q_weight,
                    });
                }
                "weightlist" => {
                    // `$sequence ... weightlist "<名>"`（`ParseAnimationToken`
                    // 的 `strnicmp("weightlist", token, 6)` 分支）。
                    // 官方把它记成序列 `cmds[]` 里的 `CMD_WEIGHTS`，
                    // 最终作用在**动画**上（`setAnimationWeight`）。
                    let n = self.tok(false)?.text;
                    seq.weight_list = Some(n);
                }
                "sectionframes" => {
                    seq.section_frames = Some(self.i()?);
                    if self.avail() {
                        seq.section_threshold = Some(self.i()?);
                    }
                }
                "keyvalues" => {
                    let _ = self.option_keyvalues()?;
                }
                "frames" => {
                    // `$sequence` 块内 `frames a b` 只对 `$animation` 有意义。
                    self.skip_rest_of_line();
                }
                "subtract" => {
                    // `$sequence` 块内的 `subtract` 走 `ParseAnimationToken`
                    // 的路径（官方 `2944`：`numblends||isAppend` 时才走）。
                    // 单动画序列在 `numblends==0` 时它会被当成**动画名**。
                    // 这里保持与官方一致：见下面的默认分支。
                    blend_names.push(t.text.clone());
                }
                other if other.starts_with("act_") => {
                    // 官方 `strnicmp(token,"ACT_",4)==0` 时 UnGetToken 后
                    // 走 `Option_Activity`（只读一个 token，无权重）。
                    seq.activity = Some(t.text.clone());
                    seq.activity_weight = 0;
                }
                _ => {
                    if t.text.ends_with(".smd") || t.text.ends_with(".SMD") {
                        if blend_names.is_empty() && seq.smd.is_empty() {
                            seq.smd = self.resolve_src(&t.text);
                            self.referenced_files.push(seq.smd.clone());
                        } else {
                            blend_names.push(t.text.clone());
                        }
                    } else {
                        // 假定是动画名（官方先查 `$animation` 池，查不到建隐含动画）。
                        blend_names.push(t.text.clone());
                    }
                }
            }
        }

        if blend_names.is_empty() {
            if seq.smd.is_empty() {
                return Err(QcError::new(
                    name_t.file.clone(),
                    name_t.line,
                    format!("序列 {name:?} 没有找到任何动画（官方是 no animations found）"),
                ));
            }
            // 单动画序列：`blends` **留空**，引用放进 `smd`
            // —— 这是 IR 的约定（`Sequence::blends` 文档：
            // 「空 = 单动画序列，它自己的 animdesc 由 `@序列名` 隐含产生」）。
            //
            // `smd` 既可能是 SMD 路径，也可能是 `$animation` 名 ——
            // `compile.rs` 先拿它查动画池，查不到才当路径加载，
            // 与官方 `studiomdl.cpp:2952-2959` 的行为一致。
        } else if blend_names.len() == 1 && seq.smd.is_empty() && seq.blend_params.is_empty() {
            // 只引用了一个**动画名**（非 SMD 路径）、且没有任何 `blend` 轴
            // —— 这就是单动画序列，与 `$sequence "x" "a_idle"` 同义。
            //
            // ⚠️ 必须排除 `blend_params` 非空的情形：`{ "a_idle" blend "p" 0 1 }`
            // 是一个**只有 1 格的 blend 网格**（`groupsize = 1×1`），
            // 它要保留 `paramindex`/`paramstart`/`paramend`，collapse 掉会丢数据。
            seq.smd = blend_names[0].clone();
        } else {
            // blend 网格：每一格是**动画名**（或 SMD 路径，官方会建隐含动画）。
            // `smd` 在 blend 形态下被忽略，但仍要求非空。
            if seq.smd.is_empty() {
                seq.smd = blend_names[0].clone();
            }
            seq.blends = blend_names;
        }
        self.desc.sequences.push(seq);
        Ok(())
    }

    /// `event <名或数字> <帧> "<参数>"`（`Option_Event`，`1186`）。
    ///
    /// 返回 `(事件, 需要从 depth 减去的量)` —— 官方允许事件带 `{ }` 块
    /// （`Option_Event` 返回值即「是否吃掉了 `}`」）。
    fn option_event(&mut self, _seq: &str) -> Result<(SequenceEvent, i32), QcError> {
        let name = self.tok(false)?.text;
        let frame = self.i()?;
        let options = if self.avail() {
            self.tok(false)?.text
        } else {
            String::new()
        };
        // 官方 `write.cpp:487-523`：名字以数字开头 ⟹ 旧式数字事件。
        let numeric = name.starts_with(|c: char| c.is_ascii_digit());
        Ok((
            SequenceEvent {
                // `cycle = frame / (numframes - 1)`，帧数在写出期才知道 ——
                // 这里先存**帧号**在 `cycle` 里，由 `compile`/`mdl_writer` 换算？
                // 不：TOML 的 `cycle` 语义就是 cycle。所以这里存帧号，
                // 由 `finish()` 之后的 `compile` 路径换算 —— 见下面的说明。
                cycle: frame as f32,
                event_type: if numeric { 0 } else { 1024 },
                event: if numeric { parse_i32(&name).unwrap_or(0) } else { 0 },
                name: if numeric { String::new() } else { name },
                options,
            },
            0,
        ))
    }

    /// `ikrule`（`Option_IKRule`，`1216`）。
    fn option_ikrule(&mut self) -> Result<IkRule, QcError> {
        let chain = self.tok(false)?.text;
        let kind_t = self.tok(false)?;
        let kind = match kind_t.text.to_ascii_lowercase().as_str() {
            "touch" => IkRuleType::Touch,
            "footstep" => IkRuleType::Footstep,
            "attachment" => IkRuleType::Attachment,
            "release" => IkRuleType::Release,
            "unlatch" => IkRuleType::Unlatch,
            other => {
                return Err(QcError::new(
                    kind_t.file.clone(),
                    kind_t.line,
                    format!("未知的 ikrule 类型 {other:?}"),
                ));
            }
        };
        let mut rule = IkRule {
            chain,
            kind,
            bone: None,
            attachment: None,
            target: None,
            height: None,
            radius: None,
            pad: None,
            floor: None,
            range: None,
            contact: None,
            fake_origin: None,
            fake_rotate: None,
            use_source: false,
        };
        if kind == IkRuleType::Touch {
            let b = self.tok(false)?;
            if !b.text.is_empty() {
                rule.bone = Some(b.text);
            }
        } else if kind == IkRuleType::Attachment {
            // 官方 `Option_IKRule` 的 `attachment` 分支**只读一个 token**
            // （附着点名），把它存进 `pRule->attachment`。
            //
            // ⚠️ 实测 `abi3.qc` 写的是 `ikrule "leg" attachment "ankle" "muzzle"`
            // —— **两个** token。官方只读第一个（`"ankle"`）当附着点名，
            // 第二个（`"muzzle"`）会落进下面的 `while (TokenAvailable())` 循环，
            // 而那个循环**不认识的 token 直接跳过**（没有 else 报错）。
            // 所以这里也照做：读一个当附着名，剩下的由循环静默忽略。
            rule.attachment = Some(self.tok(false)?.text);
        }
        while self.avail() {
            let t = self.tok(false)?;
            match t.text.to_ascii_lowercase().as_str() {
                "usesource" => rule.use_source = true,
                "usesequence" => rule.use_source = false,
                "fakeorigin" => rule.fake_origin = Some(self.v3()?),
                "fakerotate" => rule.fake_rotate = Some(self.v3()?),
                "floor" => rule.floor = Some(self.f()?),
                "height" => rule.height = Some(self.f()?),
                "radius" => rule.radius = Some(self.f()?),
                "pad" => rule.pad = Some(self.f()?),
                "target" => rule.target = Some(self.i()?),
                "contact" => rule.contact = Some(self.i()?),
                "range" => {
                    let mut r: [Option<i32>; 4] = [None; 4];
                    for slot in r.iter_mut() {
                        let v = self.tok(false)?;
                        *slot = if v.text == "." {
                            None
                        } else {
                            Some(parse_i32(&v.text).unwrap_or(0))
                        };
                    }
                    rule.range = Some(r);
                }
                other => {
                    // 官方 `Option_IKRule` 的选项循环**没有 else 分支** ——
                    // 不认识的 token 被静默忽略（例如 `abi3.qc` 多写的那个
                    // 附着点名）。这里照抄：不报错，只跳过。
                    let _ = other;
                }
            }
        }
        Ok(rule)
    }

    /// `$animation <名> <smd> [块]`（`Cmd_Animation` + `ParseAnimation`）。
    fn cmd_animation(&mut self) -> Result<(), QcError> {
        let name_t = self.tok(false)?;
        let name = name_t.text.clone();
        if !self.animation_names.insert(name.to_ascii_lowercase()) {
            return Err(QcError::new(
                name_t.file.clone(),
                name_t.line,
                format!("重复的动画名 {name:?}（官方是 Duplicate animation name）"),
            ));
        }
        let smd_raw = self.tok(false)?.text;
        let smd = self.resolve_src(&smd_raw);
        self.referenced_files.push(smd.clone());

        let mut anim = Animation {
            name,
            smd,
            fps: None,
            looping: false,
            frames: None,
            subtract: None,
            subtract_frame: None,
            ik_rules: Vec::new(),
            no_auto_ik: false,
            weight_list: None,
        };
        let mut depth = 0i32;
        loop {
            let t = if depth > 0 {
                match self.lex.next_token(true)? {
                    Some(t) => t,
                    None => break,
                }
            } else {
                if !self.avail() {
                    break;
                }
                self.tok(false)?
            };
            if t.text == "{" {
                depth += 1;
                continue;
            }
            if t.text == "}" {
                depth -= 1;
                if depth <= 0 {
                    break;
                }
                continue;
            }
            self.parse_animation_token(&mut anim, &t)?;
        }
        self.desc.animations.push(anim);
        Ok(())
    }

    /// `ParseAnimationToken`（`studiomdl.cpp:2185`）。
    fn parse_animation_token(&mut self, anim: &mut Animation, t: &Token) -> Result<(), QcError> {
        match t.text.to_ascii_lowercase().as_str() {
            "fps" => anim.fps = Some(self.f()?),
            "origin" => {
                let _ = self.v3()?;
            }
            "rotate" | "angles" => {
                let n = if t.text.eq_ignore_ascii_case("rotate") {
                    1
                } else {
                    3
                };
                for _ in 0..n {
                    let _ = self.f()?;
                }
            }
            "scale" => {
                let _ = self.f()?;
            }
            "frame" | "frames" => {
                let a = self.i()?;
                let b = self.i()?;
                anim.frames = Some([a, b]);
            }
            "subtract" => {
                let n = self.tok(false)?.text;
                anim.subtract = Some(n);
                if self.avail() {
                    // 官方 `ParseAnimationToken` 的 `subtract` 分支读第二 token
                    // 作为「取参考动画的第几帧」。
                    let t2 = self.tok(false)?;
                    if let Some(v) = parse_i32(&t2.text) {
                        anim.subtract_frame = Some(v);
                    } else {
                        self.lex.unget(t2);
                    }
                }
            }
            "noautoik" => anim.no_auto_ik = true,
            "autoik" => anim.no_auto_ik = false,
            "weightlist" => {
                // `$animation ... weightlist "<名>"`。
                anim.weight_list = Some(self.tok(false)?.text);
            }
            "ikrule" => {
                let r = self.option_ikrule()?;
                anim.ik_rules.push(r);
            }
            other => {
                if other.starts_with("loop") {
                    anim.looping = true;
                } else if other.starts_with("snap") {
                    // animdesc 的 SNAP 位；mdlc 的 Animation 无独立字段。
                } else if other.starts_with("startloop") || other == "fudgeloop" {
                    anim.looping = true;
                    if other.starts_with("startloop") {
                        let _ = self.i()?;
                    }
                } else if other == "post" || other == "realtime" {
                    // 无独立字段
                } else if let Some(v) = parse_i32(&t.text) {
                    // 官方把裸数字当 `cmdlist` 下标 —— 语料 0 次。
                    let _ = v;
                } else {
                    return Err(QcError::new(
                        t.file.clone(),
                        t.line,
                        format!("未知的动画选项 {other:?}"),
                    ));
                }
            }
        }
        Ok(())
    }

    /// `$definebone <名> <父> <x> <y> <z> <pitch> <yaw> <roll> [<rx> <ry> <rz> <rp> <ry2> <rr>]`。
    fn cmd_definebone(&mut self) -> Result<(), QcError> {
        let name = self.tok(false)?.text;
        let parent = self.tok(false)?.text;
        let pos = self.v3()?;
        let rot = self.v3()?;
        // 官方：本行还有 token ⟹ `bPreAligned = true` + 读 srcRealign。
        let mut pre_aligned = false;
        let mut realign_pos = None;
        let mut realign_rot = None;
        if self.avail() {
            pre_aligned = true;
            realign_pos = Some(self.v3()?);
            realign_rot = Some(self.v3()?);
        }
        self.import_bones.push(Bone {
            name,
            parent: if parent.is_empty() { None } else { Some(parent) },
            position: Some(pos),
            rotation: Some(rot),
            flags: None,
            surface_prop: None,
            bonemerge: false,
            pre_aligned: Some(pre_aligned),
            realign_position: realign_pos,
            realign_rotation: realign_rot,
        });
        Ok(())
    }

    /// `$bonemerge "<骨骼名>"`。
    fn cmd_bonemerge(&mut self) -> Result<(), QcError> {
        let t = self.tok(false)?;
        // 可能引用尚未定义的骨骼（SMD 里的）—— 先记成待办。
        self.bonemerge_names.push(t.text);
        Ok(())
    }

    /// `$attachment "<名>" "<骨骼>" <x> <y> <z> [absolute|rigid|world_align|rotate rx ry rz|x_and_z_axes ...]`。
    fn cmd_attachment(&mut self) -> Result<(), QcError> {
        let name = self.tok(false)?.text;
        let bone = self.tok(false)?.text;
        let pos = self.v3()?;
        let mut flags = 0i32;
        let mut rotation = [0.0f32; 3];
        let mut absolute = false;
        while self.avail() {
            let t = self.tok(false)?;
            match t.text.to_ascii_lowercase().as_str() {
                "absolute" => {
                    absolute = true;
                    flags |= 0x0001; // IS_ABSOLUTE
                }
                "rigid" => flags |= 0x0002, // IS_RIGID
                "world_align" => flags |= 0x0004,
                "rotate" => {
                    for slot in rotation.iter_mut() {
                        if !self.avail() {
                            break;
                        }
                        *slot = self.f()?;
                    }
                }
                "x_and_z_axes" => {
                    let _ = self.v3()?;
                    let _ = self.v3()?;
                }
                other => {
                    return Err(QcError::new(
                        t.file.clone(),
                        t.line,
                        format!("未知的 $attachment 选项 {other:?}"),
                    ));
                }
            }
        }
        self.desc.attachments.push(Attachment {
            name,
            bone,
            position: Some(pos),
            rotation: Some(rotation),
            flags: Some(flags),
        });
        self.attachment_absolute.push(absolute);
        Ok(())
    }

    /// `$hboxset "<名>"`。
    fn cmd_hboxset(&mut self) -> Result<(), QcError> {
        let t = self.tok(false)?;
        self.desc.hitboxes.set_name = Some(t.text);
        Ok(())
    }

    /// `$hbox <组> "<骨骼>" <minx> <miny> <minz> <maxx> <maxy> <maxz> ["<名>"]`。
    fn cmd_hbox(&mut self) -> Result<(), QcError> {
        let group = self.i()?;
        let bone = self.tok(false)?.text;
        let bbmin = self.v3()?;
        let bbmax = self.v3()?;
        let name = if self.avail() {
            Some(self.tok(false)?.text)
        } else {
            None
        };
        self.desc.hitboxes.boxes.push(Hitbox {
            bone,
            group: Some(group),
            bbmin,
            bbmax,
            name,
        });
        Ok(())
    }

    /// `$ikchain "<名>" "<骨骼>" [knee x y z] [height h] [pad p] [floor f] [center x y z]`。
    fn cmd_ikchain(&mut self) -> Result<(), QcError> {
        let name = self.tok(false)?.text;
        if self
            .desc
            .ikchains
            .iter()
            .any(|c| c.name.eq_ignore_ascii_case(&name))
        {
            // 官方：重复链名**静默忽略**（消费掉本行剩余 token）。
            self.skip_rest_of_line();
            return Ok(());
        }
        let bone = self.tok(false)?.text;
        let mut knee_dir = None;
        while self.avail() {
            let t = self.tok(false)?;
            match t.text.to_ascii_lowercase().as_str() {
                "knee" => knee_dir = Some(self.v3()?),
                "height" => {
                    let _ = self.f()?;
                }
                "pad" => {
                    let _ = self.f()?;
                }
                "floor" => {
                    let _ = self.f()?;
                }
                "center" => {
                    let _ = self.v3()?;
                }
                other => {
                    return Err(QcError::new(
                        t.file.clone(),
                        t.line,
                        format!("未知的 $ikchain 选项 {other:?}"),
                    ));
                }
            }
        }
        self.desc.ikchains.push(IkChain {
            name,
            bone,
            knee_dir,
        });
        Ok(())
    }

    /// `$ikautoplaylock "<链名>" <posW> <localQW>`。
    fn cmd_ikautoplaylock(&mut self) -> Result<(), QcError> {
        let chain = self.tok(false)?.text;
        let pos_weight = self.f()?;
        let local_q_weight = self.f()?;
        self.desc.ik_autoplay_locks.push(IkAutoplayLock {
            chain,
            pos_weight,
            local_q_weight,
        });
        Ok(())
    }

    /// `$includemodel "<路径>"` —— 官方**自动加 `models/` 前缀**。
    fn cmd_includemodel(&mut self) -> Result<(), QcError> {
        let t = self.tok(false)?;
        // 官方 `Cmd_IncludeModel`（`studiomdl.cpp:5961`）把 `"models/"` 直接
        // `strcat` 到 token 前面。
        let name = t.text.replace('\\', "/");
        let full = if name.starts_with("models/") {
            name
        } else {
            format!("models/{name}")
        };
        self.desc.include_models.push(full);
        Ok(())
    }

    /// `$lod <距离> { replacemodel "a" "b" | bonetreecollapse "b" | replacebone "a" "b" | nofacial }`。
    fn cmd_lod(&mut self, forced_switch: Option<f32>) -> Result<(), QcError> {
        let switch = match forced_switch {
            Some(v) => v,
            None => self.f()?,
        };
        let t = self.tok(true)?;
        if t.text != "{" {
            self.lex.unget(t);
            return Ok(());
        }
        let mut lod = LodModel {
            smd: None,
            switch_point: Some(switch),
            bone_tree_collapse: Vec::new(),
            replace_bone: Vec::new(),
            no_facial: false,
        };
        let mut depth = 1i32;
        while depth > 0 {
            let Some(t) = self.lex.next_token(true)? else {
                break;
            };
            if t.text == "{" {
                depth += 1;
                continue;
            }
            if t.text == "}" {
                depth -= 1;
                continue;
            }
            match t.text.to_ascii_lowercase().as_str() {
                "replacemodel" => {
                    // `replacemodel <from> <to>`（`Cmd_ReplaceModel`，
                    // `studiomdl.cpp:5387`）：
                    //
                    // * **第 1 个** = `SetSrcName` —— 被替换的**源**（= LOD 0）
                    // * **第 2 个** = `SetDstName` —— 本档用的**新网格**（= LOD N）
                    //
                    // ⚠️ **顺序与我最初写的相反**（曾误以为是 `<lodN> <lod0>`）。
                    //
                    // 判据（oracle，`lodchk.qc` + 官方 stdout）：
                    //
                    // ```text
                    // QC:  replacemodel "ipf" "ipf-lod1"
                    // 官方: SMD MODEL ipf.smd          ← 第 1 个是 LOD 0
                    //       SMD MODEL ipf-lod1.smd     ← 第 2 个是 LOD 1
                    // ```
                    //
                    // 而且官方对第 1 个名字做 `FindCachedSource` 校验
                    // （必须是**已加载过**的源，否则 `Unknown replace model`）
                    // —— 也印证第 1 个是「源」。
                    //
                    // 官方还会剥掉扩展名，再按 `cddir` 拼 `.smd`
                    // （`Load_Source(name, "SMD")`），所以 `"ipf"` → `ipf.smd`。
                    let from = self.tok(false)?.text;
                    let to = self.tok(false)?.text;
                    let _ = from; // 源名只用于校验/匹配，落盘用第 2 个
                    let to = ensure_smd_ext(&to);
                    lod.smd = Some(self.resolve_src(&to));
                    self.referenced_files.push(lod.smd.clone().unwrap());
                    // `reverse`（可选）：官方 `SetReverse`。
                    if self.avail() {
                        let t = self.tok(false)?;
                        if !t.text.eq_ignore_ascii_case("reverse") {
                            return Err(QcError::new(
                                t.file.clone(),
                                t.line,
                                format!("replacemodel 第 3 个参数只能是 reverse，得到 {:?}", t.text),
                            ));
                        }
                    }
                }
                "bonetreecollapse" => {
                    lod.bone_tree_collapse.push(self.tok(false)?.text);
                }
                "replacebone" => {
                    let a = self.tok(false)?.text;
                    let b = self.tok(false)?.text;
                    lod.replace_bone.push([a, b]);
                }
                "nofacial" => lod.no_facial = true,
                "facial" => lod.no_facial = false,
                "replacematerial" | "removemesh" | "removemodel" | "reverse" => {
                    // §42.6：语义已实测清楚，但语料 **0 样本**，无法 oracle 验收。
                    self.skip_rest_of_line();
                }
                other => {
                    return Err(QcError::new(
                        t.file.clone(),
                        t.line,
                        format!("未知的 $lod 选项 {other:?}"),
                    ));
                }
            }
        }
        // 挂到当前 bodypart 的**每个** model 上（官方对每个 source 都记）。
        if let Some(bi) = self.cur_bodypart {
            for m in &mut self.desc.bodyparts[bi].models {
                m.lods.push(lod.clone());
            }
        } else {
            self.pending_lods.push(lod);
        }
        Ok(())
    }

    /// `$jigglebone "<骨骼>" { is_flexible { ... } ... }`。
    fn cmd_jigglebone(&mut self) -> Result<(), QcError> {
        let bone = self.tok(false)?.text;
        let mut jb = JiggleBone {
            bone,
            is_flexible: None,
            is_rigid: None,
            has_base_spring: None,
        };
        // 找 `{`。
        loop {
            let t = self.tok(true)?;
            if t.text == "{" {
                break;
            }
            if t.text == "}" {
                self.desc.jiggle_bones.push(jb);
                return Ok(());
            }
        }
        let mut depth = 1i32;
        while depth > 0 {
            let Some(t) = self.lex.next_token(true)? else {
                break;
            };
            if t.text == "{" {
                depth += 1;
                let low = self.last_block_name.clone().unwrap_or_default();
                self.parse_jiggle_sub(&low, &mut jb, &mut depth)?;
                continue;
            }
            if t.text == "}" {
                depth -= 1;
                continue;
            }
            self.last_block_name = Some(t.text.to_ascii_lowercase());
        }
        self.desc.jiggle_bones.push(jb);
        Ok(())
    }

    /// 解析 `$jigglebone` 的一个子块。
    fn parse_jiggle_sub(
        &mut self,
        block: &str,
        jb: &mut JiggleBone,
        depth: &mut i32,
    ) -> Result<(), QcError> {
        match block {
            "is_flexible" => jb.is_flexible = Some(JiggleFlexible::default()),
            "is_rigid" => jb.is_rigid = Some(JiggleRigid::default()),
            "has_base_spring" => jb.has_base_spring = Some(JiggleBaseSpring::default()),
            _ => {}
        }
        while *depth > 1 {
            let Some(t) = self.lex.next_token(true)? else {
                return Ok(());
            };
            if t.text == "{" {
                *depth += 1;
                continue;
            }
            if t.text == "}" {
                *depth -= 1;
                continue;
            }
            let low = t.text.to_ascii_lowercase();
            // 只消费数值 token，字段本身暂不填（jiggle 由 TOML 侧验收）。
            match low.as_str() {
                "length" | "tip_mass" | "yaw_stiffness" | "yaw_damping" | "pitch_stiffness"
                | "pitch_damping" | "along_stiffness" | "along_damping" | "angle_constraint"
                | "yaw_friction" | "yaw_bounce" | "pitch_friction" | "pitch_bounce"
                | "base_mass" | "base_stiffness" | "base_damping" | "base_left_friction"
                | "base_up_friction" | "base_forward_friction" => {
                    let v = self.f()?;
                    Self::set_jiggle_scalar(jb, &low, v);
                }
                "yaw_constraint" | "pitch_constraint" | "base_left" | "base_up"
                | "base_forward" => {
                    let a = self.f()?;
                    let b = self.f()?;
                    Self::set_jiggle_pair(jb, &low, [a, b]);
                }
                "allow_length_flex" => {
                    if let Some(f) = jb.is_flexible.as_mut() {
                        f.allow_length_flex = true;
                    }
                }
                _ => {}
            }
        }
        Ok(())
    }

    fn set_jiggle_scalar(jb: &mut JiggleBone, key: &str, v: f32) {
        let to_rad = |x: f32| x.to_radians();
        match key {
            "angle_constraint" => {
                if let Some(f) = jb.is_flexible.as_mut() {
                    f.angle_constraint = Some(to_rad(v));
                } else if let Some(r) = jb.is_rigid.as_mut() {
                    r.angle_constraint = Some(to_rad(v));
                }
            }
            _ => {
                if let Some(f) = jb.is_flexible.as_mut() {
                    match key {
                        "length" => f.length = Some(v),
                        "tip_mass" => f.tip_mass = Some(v),
                        "yaw_stiffness" => f.yaw_stiffness = Some(v),
                        "yaw_damping" => f.yaw_damping = Some(v),
                        "pitch_stiffness" => f.pitch_stiffness = Some(v),
                        "pitch_damping" => f.pitch_damping = Some(v),
                        "along_stiffness" => f.along_stiffness = Some(v),
                        "along_damping" => f.along_damping = Some(v),
                        "yaw_friction" => f.yaw_friction = Some(v),
                        "yaw_bounce" => f.yaw_bounce = Some(v),
                        "pitch_friction" => f.pitch_friction = Some(v),
                        "pitch_bounce" => f.pitch_bounce = Some(v),
                        _ => {}
                    }
                } else if let Some(r) = jb.is_rigid.as_mut() {
                    match key {
                        "length" => r.length = Some(v),
                        "tip_mass" => r.tip_mass = Some(v),
                        _ => {}
                    }
                }
            }
        }
        if let Some(b) = jb.has_base_spring.as_mut() {
            match key {
                "base_mass" => b.base_mass = Some(v),
                "base_stiffness" => b.base_stiffness = Some(v),
                "base_damping" => b.base_damping = Some(v),
                "base_left_friction" => b.base_left_friction = Some(v),
                "base_up_friction" => b.base_up_friction = Some(v),
                "base_forward_friction" => b.base_forward_friction = Some(v),
                _ => {}
            }
        }
    }

    fn set_jiggle_pair(jb: &mut JiggleBone, key: &str, v: [f32; 2]) {
        let to_rad = |x: f32| x.to_radians();
        if let Some(f) = jb.is_flexible.as_mut() {
            match key {
                "yaw_constraint" => f.yaw_constraint = Some([to_rad(v[0]), to_rad(v[1])]),
                "pitch_constraint" => f.pitch_constraint = Some([to_rad(v[0]), to_rad(v[1])]),
                _ => {}
            }
        }
        if let Some(b) = jb.has_base_spring.as_mut() {
            match key {
                "base_left" => b.base_left = Some([to_rad(v[0]), to_rad(v[1])]),
                "base_up" => b.base_up = Some([to_rad(v[0]), to_rad(v[1])]),
                "base_forward" => b.base_forward = Some([to_rad(v[0]), to_rad(v[1])]),
                _ => {}
            }
        }
    }

    /// `$proceduralbones "<vrd>"` —— 读 `.vrd` 生成 quatinterp 骨骼。
    ///
    /// # ⚠️ VRD 文件不存在时官方**静默跳过**（oracle 实测）
    ///
    /// `vrj3.qc` 写 `$proceduralbones "ai1.txt"`，而 `ai1.txt` **不存在**。
    /// 官方 studiomdl：**退出码 0**、产物正常、stdout/stderr 里
    /// **没有任何**关于 `ai1` 的消息。
    ///
    /// 所以这里也按「文件不存在 ⟹ 无 quatinterp 骨骼」处理，
    /// 只打一行提示（官方连提示都没有，mdlc 多给一行更友好，
    /// 但不改变产物）。
    ///
    /// 文件**存在但解析失败**时仍然报错 —— 那说明是格式问题，
    /// 静默跳过会掩盖真错误。
    fn cmd_proceduralbones(&mut self) -> Result<(), QcError> {
        let t = self.tok(false)?;
        let vrd = self.resolve_src(&t.text);
        let path = self.qdir.join(&vrd);
        let text = match std::fs::read_to_string(&path) {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                // 与官方一致：文件不在 ⟹ 没有程序化骨骼。
                eprintln!(
                    "提示：$proceduralbones 指向的 VRD 不存在，按官方行为跳过：{}",
                    path.display()
                );
                return Ok(());
            }
            Err(e) => {
                return Err(QcError::new(
                    t.file.clone(),
                    t.line,
                    format!("读不到 VRD {}：{e}", path.display()),
                ));
            }
        };
        let bones = parse_vrd(&text).map_err(|m| QcError::new(t.file.clone(), t.line, m))?;
        self.desc.quat_interp_bones.extend(bones);
        Ok(())
    }

    /// `$jointsurfaceprop "<骨骼>" "<材质>"`。
    fn cmd_jointsurfaceprop(&mut self) -> Result<(), QcError> {
        let bone = self.tok(false)?.text;
        let prop = self.tok(false)?.text;
        self.joint_surface_props.push((bone, prop));
        Ok(())
    }

    /// `$weightlist "<名>" [<骨骼> <权重> ...] [{ ... }]`。
    ///
    /// # 官方语法（`Cmd_Weightlist` + `Option_Weightlist`，
    /// `studiomdl.cpp:3248-3353`）
    ///
    /// **两种写法等价**（`Option_Weightlist` 的循环把 `{`/`}` 当 depth 调整，
    /// 其它 token 一律当 `<骨骼> <权重>` 对）：
    ///
    /// ```text
    /// $weightlist FULLBODY $Bone$Pelvis 1          ← 单行、无块
    /// $weightlist INJUREDIDLENOISE {               ← 带块
    ///   $Bone$L_Thigh 0
    ///   $Bone$Pelvis .6
    /// }
    /// ```
    ///
    /// 另有一条 `posweight <v>` 后缀，作用于**上一条**骨骼条目
    /// （`i = pweightlist->numbones - 1`）。
    ///
    /// # 校验（官方会 `MdlError` 的两条）
    ///
    /// 1. `posweight` 出现在任何骨骼条目**之前** ⟹ 报错；
    /// 2. `weight == 0 && posweight > 0` ⟹ 报错
    ///    （`Non-zero Position weight with zero Rotation weight not allowed`）。
    fn cmd_weightlist(&mut self) -> Result<(), QcError> {
        let name_t = self.tok(false)?;
        let name = name_t.text.clone();
        // 表数上限：官方 L4D2 `MAXWEIGHTLISTS` = **128**，且**含隐式的表 0**
        // （`studiomdl.cpp:6886` `g_numweightlist = 1`），所以手写最多 127 张。
        // 实测边界：127 张 OK / 128 张 ERROR（`Too many weightlist commands (128)`）。
        //
        // ⚠️ episode1 头文件写的是 32，是**错的**。
        if self.desc.weight_lists.len() >= crate::model::MAX_WEIGHT_LISTS - 1 {
            return Err(QcError::new(
                name_t.file.clone(),
                name_t.line,
                format!(
                    "权重表超过官方上限 {} 张（含隐式默认表，手写最多 {} 张）",
                    crate::model::MAX_WEIGHT_LISTS,
                    crate::model::MAX_WEIGHT_LISTS - 1
                ),
            ));
        }
        // 查重（官方 `for (i = 1; i < g_numweightlist; i++)` —— 跳过表 0）。
        if self
            .desc
            .weight_lists
            .iter()
            .any(|w| w.name.eq_ignore_ascii_case(&name))
        {
            return Err(QcError::new(
                name_t.file.clone(),
                name_t.line,
                format!("重复的 $weightlist {name:?}"),
            ));
        }

        let mut bones: Vec<WeightEntry> = Vec::new();
        let mut depth = 0i32;
        loop {
            // 与官方一致：depth > 0 时跨行取 token；否则只在本行内取。
            let t = if depth > 0 {
                match self.lex.next_token(true)? {
                    Some(t) => t,
                    None => break,
                }
            } else {
                if !self.avail() {
                    break;
                }
                self.tok(false)?
            };
            if t.text == "{" {
                depth += 1;
            } else if t.text == "}" {
                depth -= 1;
                if depth <= 0 {
                    break;
                }
            } else if t.text.eq_ignore_ascii_case("posweight") {
                let v = self.f()?;
                let Some(last) = bones.last_mut() else {
                    return Err(QcError::new(
                        t.file.clone(),
                        t.line,
                        format!("posweight 出现在任何骨骼条目之前（weightlist {name:?}）"),
                    ));
                };
                if last.weight == 0.0 && v > 0.0 {
                    return Err(QcError::new(
                        t.file.clone(),
                        t.line,
                        format!(
                            "骨骼 {:?} 的旋转权重为 0 而位置权重为 {v} —— 官方不允许",
                            last.bone
                        ),
                    ));
                }
                last.pos_weight = Some(v);
            } else {
                let bone = t.text;
                let weight = self.f()?;
                // 官方 L4D2 的 `MAXWEIGHTSPERLIST` = **128**（实测，
                // 见 `crate::model::MAX_WEIGHT_ENTRIES` 的说明 ——
                // episode1 头文件写的 16 是**错的**），
                // 在**解析期**报 `Too many bones (128) in weightlist '%s'`
                // （`Option_Weightlist`，`studiomdl.cpp:3310-3313`）。
                if bones.len() >= crate::model::MAX_WEIGHT_ENTRIES {
                    return Err(QcError::new(
                        t.file.clone(),
                        t.line,
                        format!(
                            "权重表 {name:?} 的条目超过官方上限 {}",
                            crate::model::MAX_WEIGHT_ENTRIES
                        ),
                    ));
                }
                bones.push(WeightEntry {
                    bone,
                    weight,
                    // 官方：`boneposweight[i] = boneweight[i]`。
                    pos_weight: None,
                });
            }
        }
        self.desc.weight_lists.push(WeightList { name, bones });
        Ok(())
    }

    /// `$defaultweightlist { ... }` —— 覆盖**隐式表 0**。
    ///
    /// # 官方语义（`Cmd_DefaultWeightlist`，`studiomdl.cpp:3355-3358`）
    ///
    /// 它写的是 `g_weightlist[0]` —— 即那张「根 1、子骨骼沿父链继承」
    /// 的隐式表。`buildAnimationWeights` 的 `i == 0` 分支会先把表 0
    /// **重置**成 `根=1 / 子=-1`，再套用这里的显式条目，然后沿父链补齐。
    ///
    /// # mdlc 的处置：**报错而不是静默忽略**
    ///
    /// mdlc 的隐式表 0 是**常量**（全 1，见 `default_weight_list`），
    /// 没有可覆盖的存储。而 `$defaultweightlist` 会改变**所有**没写
    /// `weightlist` 的序列的权重 —— 静默忽略它会让产物**看起来对但语义错**，
    /// 正是本项目反复强调要避免的「静默吃数据」。
    ///
    /// 语料实测：`$defaultweightlist` 在 53 个真实 QC 里出现 **0 次**
    /// （`$weightlist` 出现 9 次）。所以先显式报错，等有真实样本再实现。
    fn cmd_defaultweightlist(&mut self) -> Result<(), QcError> {
        Err(self.lex.error(
            "$defaultweightlist 尚未支持 —— 它会覆盖**所有**未显式指定 weightlist \
             的序列的权重，静默忽略会产出语义错误的模型。\
             语料里该命令出现 0 次（$weightlist 出现 9 次）。\
             若你有用到它的 QC，请改用 `$weightlist \"<名>\"` 并在序列上显式引用。",
        ))
    }

    /// `$collisionmodel "<smd>" { ... }` / `$collisionjoints`。
    fn cmd_collisionmodel(&mut self, joints: bool) -> Result<(), QcError> {
        let t = self.tok(false)?;
        let smd = self.resolve_src(&t.text);
        self.desc.physics.smd = Some(smd.clone());
        self.referenced_files.push(smd);
        self.desc.physics.joints = joints;
        // 块内子键。
        if self.avail() {
            let t = self.tok(false)?;
            if t.text == "{" {
                let mut depth = 1;
                while depth > 0 {
                    let Some(t) = self.lex.next_token(true)? else {
                        break;
                    };
                    if t.text == "{" {
                        depth += 1;
                        continue;
                    }
                    if t.text == "}" {
                        depth -= 1;
                        continue;
                    }
                    self.physics_subkey(&t)?;
                }
            } else {
                self.lex.unget(t);
            }
        }
        Ok(())
    }

    /// `$collisionmodel` / `$collisionjoints` 块内的一个子键。
    ///
    /// ⚠️ **块内子键带 `$` 前缀**（与顶层命令同形）——
    /// `ParseCollisionCommands`（`collisionmodel.cpp:1767`）里
    /// 逐条 `stricmp(command, "$mass")` 等，命令名是**原样**的 token。
    /// 所以这里比较时要把 `$` 去掉，但**不能**假设它不存在。
    fn physics_subkey(&mut self, t: &Token) -> Result<(), QcError> {
        let low = t.text.to_ascii_lowercase();
        // 剥掉 `$`（若在），再比较。
        let key = low.strip_prefix('$').unwrap_or(&low);
        match key {
            "mass" => self.desc.physics.mass = Some(self.f()?),
            "concave" => self.desc.physics.concave = true,
            "damping" => self.desc.physics.damping = Some(self.f()?),
            "rotdamping" => self.desc.physics.rot_damping = Some(self.f()?),
            "inertia" => self.desc.physics.inertia = Some(self.f()?),
            "drag" => self.desc.physics.drag = Some(self.f()?),
            "rootbone" => self.desc.physics.root_bone = Some(self.tok(false)?.text),
            "noselfcollisions" => self.desc.physics.no_self_collisions = true,
            "automass" => self.desc.physics.auto_mass = true,
            "masscenter" => self.desc.physics.mass_center = Some(self.v3()?),
            "jointmerge" => self.cmd_collision_pair(true)?,
            "jointcollide" => self.cmd_collision_pair(false)?,
            "jointconstrain" => self.cmd_jointconstrain()?,
            "animatedfriction" => self.cmd_animatedfriction()?,
            // `$phyname`：只影响 `.phy` 的落盘路径，mdlc 由 `--out` 覆盖。
            "phyname" => {
                let _ = self.tok(false)?;
            }
            // 语料 0 次 / 无产物痕迹：只消费参数。
            "maxconvexpieces" => {
                let _ = self.i()?;
            }
            // `$rollingDrag` 官方读了参数但**什么也不做**
            // （`collisionmodel.cpp:1756`：调用被注释掉了）。
            "rollingdrag" => {
                let _ = self.f()?;
            }
            "weldposition" | "weldnormal" | "remove2d" | "polysoup" | "assumeworldspace"
            | "concaveperjoint" | "snapcollisionjoints" | "preservetriangleorder" => {}
            "jointskip" | "jointmassbias" | "jointdamping" | "jointrotdamping" | "jointinertia" => {
                self.joint_override_key(key)?
            }
            other => {
                return Err(QcError::new(
                    t.file.clone(),
                    t.line,
                    format!("未知的碰撞块子键 {other:?}"),
                ));
            }
        }
        Ok(())
    }

    /// 逐 joint 覆盖（`$jointdamping` 等）。
    fn joint_override_key(&mut self, key: &str) -> Result<(), QcError> {
        let bone = self.tok(false)?.text;
        let v = self.f()?;
        let entry = match self
            .desc
            .physics
            .joint_overrides
            .iter_mut()
            .find(|j| j.bone.eq_ignore_ascii_case(&bone))
        {
            Some(e) => e,
            None => {
                self.desc.physics.joint_overrides.push(JointOverride {
                    bone,
                    damping: None,
                    rot_damping: None,
                    inertia: None,
                    mass_bias: None,
                });
                self.desc.physics.joint_overrides.last_mut().unwrap()
            }
        };
        match key {
            "jointdamping" => entry.damping = Some(v),
            "jointrotdamping" => entry.rot_damping = Some(v),
            "jointinertia" => entry.inertia = Some(v),
            "jointmassbias" => entry.mass_bias = Some(v),
            _ => {}
        }
        Ok(())
    }

    /// `$jointconstrain "<骨骼>" <轴> <类型> <min> <max> [<摩擦>]`。
    ///
    /// # 摩擦系数是**可选**的
    ///
    /// 实测 `jc_nofric.qc` 写的是 5 个参数
    /// （`$jointconstrain "bone_mid" "x" "limit" -10 10`），
    /// 而 `phy-jointconstrain*.toml` 的官方对照用例写 6 个。
    ///
    /// 官方 `CCmd_JointConstrain`（`collisionmodel.cpp:1900` 附近）
    /// 用 `ReadArgs(args, 6)` —— 它**读满 6 个**，读不到就用空串，
    /// 而 `Safe_atof("")` = 0。所以缺省摩擦 = **0**。
    fn cmd_jointconstrain(&mut self) -> Result<(), QcError> {
        let bone = self.tok(false)?.text;
        let axis = self.tok(false)?.text;
        let kind = self.tok(false)?.text;
        let min = self.f()?;
        let max = self.f()?;
        let friction = if self.avail() { self.f()? } else { 0.0 };
        self.desc.physics.constraints.push(JointConstraintSpec {
            bone,
            axis,
            kind,
            min,
            max,
            friction: Some(friction),
        });
        Ok(())
    }

    /// `$animatedfriction <min> <max> <timehold> <timeout> <timein>`。
    fn cmd_animatedfriction(&mut self) -> Result<(), QcError> {
        let min = self.i()?;
        let max = self.i()?;
        let time_hold = self.f()?;
        let time_out = self.f()?;
        let time_in = self.f()?;
        self.desc.physics.animated_friction = Some(AnimatedFrictionSpec {
            min,
            max,
            time_in,
            time_out,
            time_hold,
        });
        Ok(())
    }

    /// `$jointcollide "<a>" "<b>"` / `$jointmerge "<a>" "<b>"`。
    fn cmd_collision_pair(&mut self, merge: bool) -> Result<(), QcError> {
        let a = self.tok(false)?.text;
        let b = self.tok(false)?.text;
        let spec = CollisionPairSpec { a, b };
        if merge {
            self.desc.physics.merge.push(spec);
        } else {
            self.desc.physics.collision_pairs.push(spec);
        }
        Ok(())
    }

    /// `$mass` / `$damping` 等**顶层**物理键。
    fn cmd_physics_kv(&mut self, name: &str) -> Result<(), QcError> {
        match name {
            "$mass" => self.desc.physics.mass = Some(self.f()?),
            "$damping" => self.desc.physics.damping = Some(self.f()?),
            "$rotdamping" => self.desc.physics.rot_damping = Some(self.f()?),
            "$inertia" => self.desc.physics.inertia = Some(self.f()?),
            "$drag" => self.desc.physics.drag = Some(self.f()?),
            "$rootbone" => self.desc.physics.root_bone = Some(self.tok(false)?.text),
            "$concave" => self.desc.physics.concave = true,
            "$automass" => self.desc.physics.auto_mass = true,
            "$masscenter" => self.desc.physics.mass_center = Some(self.v3()?),
            "$maxconvexpieces" => {
                let _ = self.i()?;
            }
            "$phyname" => {
                let _ = self.tok(false)?;
            }
            _ => {}
        }
        Ok(())
    }

    /// 收尾：读 SMD 补全骨骼表 / 材质表 / 事件 cycle，并回填跨阶段引用。
    ///
    /// # 为什么必须读 SMD
    ///
    /// 官方在 `SimplifyModel()` 里做、而 QC 文本里**看不到**的三件事：
    ///
    /// 1. **骨骼表**（`BuildGlobalBonetable`，`simplify.cpp:3616`）：
    ///    `$definebone` 的骨骼**排在前面**（按书写序），然后才是各 SMD
    ///    里「被顶点引用」的骨骼（按 source 序 × SMD 内下标序）。
    ///    mdlc 的 `[[bones]]` 必须完整列出 —— `compile.rs` 对未声明的
    ///    骨骼**直接报错**（这是故意的，见 `PROGRESS.md` §8.4）。
    /// 2. **材质表**（`SetSkinValues` / `lookup_texture`）：
    ///    `$texturegroup` 的材质**先注册**（按组序），其余按 SMD 首现序。
    /// 3. **事件 cycle**（`write.cpp:494`）：
    ///    `cycle = frame / (numframes - 1)` —— 需要该序列 SMD 的帧数。
    fn finish(&mut self) -> Result<ModelDesc, Vec<QcError>> {
        // ---- 1. 读所有被引用的 SMD（缓存，避免重复读）----
        let mut smd_cache: HashMap<String, Option<SmdInfo>> = HashMap::new();
        let files: Vec<String> = self.referenced_files.clone();
        for f in &files {
            if smd_cache.contains_key(f) {
                continue;
            }
            let p = if Path::new(f).is_absolute() {
                PathBuf::from(f)
            } else {
                self.qdir.join(f)
            };
            // ⚠️ **不要静默吞掉读失败** —— 那正是本项目反复踩到的
            // 「静默吃数据」：SMD 读不到时骨骼表会变空，
            // 最后只报一句「至少需要一根骨骼」，指向不了真因。
            let info = match std::fs::read_to_string(&p) {
                Err(e) => {
                    self.errors.push(QcError::new(
                        p.display().to_string(),
                        0,
                        format!("读不到 SMD（QC 里引用了它）：{e}"),
                    ));
                    None
                }
                Ok(text) => match crate::smd::parse_smd(&text) {
                    Err(e) => {
                        self.errors.push(QcError::new(
                            p.display().to_string(),
                            e.line,
                            format!("SMD 解析失败：{e}"),
                        ));
                        None
                    }
                    Ok(s) => Some(SmdInfo {
                        nodes: s.nodes.iter().map(|n| n.name.clone()).collect(),
                        materials: s.materials_in_order(),
                        num_frames: s.frames.len(),
                    }),
                },
            };
            smd_cache.insert(f.clone(), info);
        }

        // ---- 2. 骨骼表 ----
        //
        // `$definebone` 先，然后各 SMD 的 nodes（按**引用顺序**）。
        let mut bones: Vec<Bone> = Vec::new();
        let mut seen: HashSet<String> = HashSet::new();
        for b in std::mem::take(&mut self.import_bones) {
            if seen.insert(b.name.to_ascii_lowercase()) {
                bones.push(b);
            }
        }
        for f in &files {
            let Some(info) = smd_cache.get(f).and_then(|x| x.as_ref()) else {
                continue;
            };
            for name in &info.nodes {
                if seen.insert(name.to_ascii_lowercase()) {
                    bones.push(Bone {
                        name: name.clone(),
                        parent: None,
                        position: None,
                        rotation: None,
                        flags: None,
                        surface_prop: None,
                        bonemerge: false,
                        pre_aligned: None,
                        realign_position: None,
                        realign_rotation: None,
                    });
                }
            }
        }
        // 父链：`$definebone` 显式给了就用；否则从**网格 SMD** 的 nodes 段推。
        //
        // 官方 `BuildGlobalBonetable` 用 `psource->localBone[j].parent`
        // （即 SMD nodes 的父下标），所以这里要找到「第一个含该骨骼的
        // SMD」并取其父名。
        let mut parent_of: HashMap<String, String> = HashMap::new();
        for b in &bones {
            if let Some(p) = &b.parent {
                parent_of.insert(b.name.to_ascii_lowercase(), p.clone());
            }
        }
        for f in &files {
            let p = if Path::new(f).is_absolute() {
                PathBuf::from(f)
            } else {
                self.qdir.join(f)
            };
            let Ok(text) = std::fs::read_to_string(&p) else {
                continue;
            };
            let Ok(smd) = crate::smd::parse_smd(&text) else {
                continue;
            };
            for n in &smd.nodes {
                let key = n.name.to_ascii_lowercase();
                if parent_of.contains_key(&key) {
                    continue;
                }
                if n.parent >= 0 {
                    if let Some(par) = smd.nodes.get(n.parent as usize) {
                        parent_of.insert(key, par.name.clone());
                    }
                }
            }
        }
        for b in &mut bones {
            if b.parent.is_none() {
                if let Some(p) = parent_of.get(&b.name.to_ascii_lowercase()) {
                    b.parent = Some(p.clone());
                }
            }
        }
        // 注：拓扑排序在字段回填之后做（见 `sort_parents_first`）。
        // `$bonemerge` 回填。
        let bm: HashSet<String> = self
            .bonemerge_names
            .iter()
            .map(|s| s.to_ascii_lowercase())
            .collect();
        for b in &mut bones {
            if bm.contains(&b.name.to_ascii_lowercase()) {
                b.bonemerge = true;
            }
        }
        // `$jointsurfaceprop` 回填。
        for (bone, prop) in std::mem::take(&mut self.joint_surface_props) {
            if let Some(b) = bones
                .iter_mut()
                .find(|b| b.name.eq_ignore_ascii_case(&bone))
            {
                b.surface_prop = Some(prop);
            }
        }
        // 拓扑排序放在**所有字段回填之后** —— 否则 `$bonemerge` /
        // `$jointsurfaceprop` 的回填会作用在排序前的下标上（虽然它们是
        // 按名字找的，但排序会换掉 Vec 的顺序，容易读错）。
        self.desc.bones = sort_parents_first(bones);

        // ---- 3. 材质表 ----
        //
        // 官方顺序：`$texturegroup` 里的材质**先**（`Cmd_TextureGroup`
        // 按遇到顺序 `use_texture_as_material`），其余按 SMD 首现序。
        // 实测见 `derive_skin_from_qc.js`。
        //
        // ⚠️ 官方只读**第一条** `$texturegroup`（`SetSkinValues` 用
        // `g_texturegroup[0]`），所以这里也只用第一条。
        let families_src: Vec<Vec<String>> = self
            .texture_groups
            .first()
            .cloned()
            .unwrap_or_default();
        for g in &families_src {
            for name in g {
                self.register_texture(name);
            }
        }
        for f in &files {
            let Some(info) = smd_cache.get(f).and_then(|x| x.as_ref()) else {
                continue;
            };
            for m in &info.materials {
                self.register_texture(m);
            }
        }
        // 纹理名规范化：官方对**不在 `$texturegroup` 里**的材质带
        // `$cdmaterials` 前缀（实测 survivor：组内 26 条裸名、其余 20 条带前缀）。
        //
        // 判据见 `PROGRESS.md` §43.3：`lookup_texture` 用的是**原样**名字，
        // 而 SMD 里的名字自带前缀 —— 所以「带不带前缀」由 SMD 决定，
        // 这里不做推断，只把已注册的名字原样写出。
        let cd = self.desc.materials.search_paths.clone();
        self.desc.materials.textures = self
            .texture_order
            .iter()
            .map(|n| Texture {
                name: crate::mdl_writer::normalize_texture_name(n, &cd),
                flags: None,
            })
            .collect();

        // ---- 4. skin family（`SetSkinValues`）----
        //
        // ```cpp
        // for i,j: g_skinref[i][j] = j;                        // 恒等
        // for i in layers: for j in reps:
        //     g_skinref[i][ g_texturegroup[0][0][j] ] = g_texturegroup[0][i][j];
        // ```
        if !families_src.is_empty() {
            let idx_of: HashMap<String, i32> = self
                .texture_order
                .iter()
                .enumerate()
                .map(|(i, n)| (n.clone(), i as i32))
                .collect();
            let base: Vec<i32> = families_src[0]
                .iter()
                .map(|n| *idx_of.get(n).unwrap_or(&-1))
                .collect();
            let n_tex = self.texture_order.len() as i32;
            let mut families = Vec::new();
            for g in &families_src {
                let mut row: Vec<i32> = (0..n_tex).collect();
                for (j, name) in g.iter().enumerate() {
                    if j >= base.len() {
                        break;
                    }
                    let slot = base[j];
                    if slot >= 0 && (slot as usize) < row.len() {
                        row[slot as usize] = *idx_of.get(name).unwrap_or(&slot);
                    }
                }
                families.push(row);
            }
            self.desc.materials.skin_families = families;
        }

        // ---- 5. 事件 cycle ----
        //
        // `cycle = frame / (numframes - 1)`（`write.cpp:494-497`），
        // 帧数取自该序列**第一格**动画的 SMD。
        let anim_smds: Vec<(String, String)> = self
            .desc
            .animations
            .iter()
            .map(|a| (a.name.clone(), a.smd.clone()))
            .collect();
        for seq in &mut self.desc.sequences {
            let first = seq.blends.first().cloned().unwrap_or_default();
            let nf = anim_smds
                .iter()
                .find(|(n, _)| n.eq_ignore_ascii_case(&first))
                .and_then(|(_, s)| smd_cache.get(s).and_then(|x| x.as_ref()))
                .map(|i| i.num_frames)
                .or_else(|| {
                    smd_cache
                        .get(&first)
                        .and_then(|x| x.as_ref())
                        .map(|i| i.num_frames)
                })
                .unwrap_or(0);
            let k = if nf > 0 { nf as f32 - 1.0 } else { 0.0 };
            for ev in &mut seq.events {
                let frame = ev.cycle;
                ev.cycle = if frame <= k {
                    if k > 0.0 { frame / k } else { 0.0 }
                } else if k == 0.0 && frame == 0.0 {
                    0.0
                } else {
                    // 官方此处 `MdlWarning` + `bErrors = true`。
                    0.0
                };
            }
        }

        // ---- 6. 杂项回填 ----
        //
        // 顶层 `$sectionframes` 是**全局**设置（官方 `g_sectionframes`），
        // 而 mdlc 的 IR 是逐序列字段 —— 回填给尚未显式设置的序列。
        if let Some((len, thr)) = self.global_section_frames {
            for seq in &mut self.desc.sequences {
                if seq.section_frames.is_none() {
                    seq.section_frames = Some(len);
                    seq.section_threshold = Some(thr);
                }
            }
        }
        self.desc.model.contents = Some(self.contents);
        self.desc.model.static_prop = self.static_prop;
        // `extra_flags`：`static_prop` 已由 `mdl_writer` 自动置位，
        // 这里只写「额外的」那些位。
        let extra = self.gflags & !FLAG_STATIC_PROP;
        if extra != 0 {
            self.desc.model.extra_flags = Some(extra);
        }
        // 若 `$modelname` 没写，用主 QC 的文件名（官方要求必须写，这里更宽容）。
        if self.desc.model.name.is_empty() {
            let stem = self
                .main_path
                .file_stem()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_else(|| "model".to_string());
            self.desc.model.name = format!("{stem}.mdl");
        }
        // `$lod` 出现在任何 bodypart 之前：挂到全部 bodypart 上。
        if !self.pending_lods.is_empty() {
            for bp in &mut self.desc.bodyparts {
                for m in &mut bp.models {
                    m.lods.extend(self.pending_lods.clone());
                }
            }
        }
        // `$lod` 的骨骼选项里，**不存在的骨骼要剔除**。
        //
        // # 官方行为（oracle 实测，不是推测）
        //
        // `lodbc.qc` 写 `bonetreecollapse "tip"`，而 `lodbc.smd` 的 nodes
        // 只有 `root/hip/knee/ankle`。官方 studiomdl 输出：
        //
        // ```text
        // WARNING: Couldn't find bone tip for bonetreecollapse, skipping
        // ```
        //
        // 然后**正常产出** `.mdl`（4 根骨骼，无该 LOD 的坍缩）——
        // 即「未知骨骼 ⟹ 该条**跳过**」，不是错误。
        //
        // ⚠️ mdlc 的 `ModelDesc::validate()` 会把未知骨骼当**错误**
        // （见那里的注释：静默失效最难排查）。这个判断对**手写 TOML**
        // 是对的 —— 用户写错名字应当被告知。但 QC 路径下官方是容忍的，
        // 而解析器的职责是「复刻官方」，所以在这里**先剔除**，
        // 让 QC 与官方行为一致；手写 TOML 仍然会被 validate 拦住。
        //
        // 剔除时打印一行提示（官方也是 WARNING 而不是静默）。
        let known: HashSet<String> = self
            .desc
            .bones
            .iter()
            .map(|b| b.name.to_ascii_lowercase())
            .collect();
        let mut skipped: Vec<String> = Vec::new();
        for bp in &mut self.desc.bodyparts {
            for m in &mut bp.models {
                for lod in &mut m.lods {
                    lod.bone_tree_collapse.retain(|b| {
                        let ok = known.contains(&b.to_ascii_lowercase());
                        if !ok {
                            skipped.push(b.clone());
                        }
                        ok
                    });
                    lod.replace_bone.retain(|p| {
                        // `replacebone <源> <目标>`：**两个**都必须存在，
                        // 否则整条跳过（官方同一处 warning）。
                        let ok = known.contains(&p[0].to_ascii_lowercase())
                            && known.contains(&p[1].to_ascii_lowercase());
                        if !ok {
                            skipped.push(format!("{} → {}", p[0], p[1]));
                        }
                        ok
                    });
                }
            }
        }
        if !skipped.is_empty() {
            skipped.sort();
            skipped.dedup();
            eprintln!(
                "提示：$lod 里有 {} 个不存在的骨骼，已按官方行为跳过：{}",
                skipped.len(),
                skipped.join(", ")
            );
        }
        // 若没有任何 bodypart（官方会报错），保持空 —— `validate()` 会报。

        if !self.errors.is_empty() {
            return Err(std::mem::take(&mut self.errors));
        }
        // `desc` 用 `clone()` 而不是 `mem::take` —— 后者要求
        // `ModelDesc: Default`，而它没有（且不该有：`model` 是必填）。
        Ok(self.desc.clone())
    }

    /// 注册一个材质名（官方 `lookup_texture` + `use_texture_as_material`）。
    fn register_texture(&mut self, name: &str) {
        if name.is_empty() {
            return;
        }
        let key = name.to_string();
        if self.texture_index.contains_key(&key) {
            return;
        }
        let i = self.texture_order.len();
        self.texture_order.push(key.clone());
        self.texture_index.insert(key, i);
    }
}

/// `Option_Flex` 的后缀选项集合。
///
/// 默认值来自官方 `Option_Flex` 的初始化段：
/// `frame = 0`、`target1`（= `position`）= **1.0**、`decay` = **1.0**，
/// `split` = 调用方传入的 `pairsplit`。
struct FlexOpts {
    /// `frame <n>` —— 取 `.vta` 的相对帧号。
    frame: i32,
    /// `position <f>` —— 落进 `mstudioflex_t.target1`。
    position: f32,
    /// `split <f>` —— smoothstep 分割点。
    split: f32,
    /// `decay <f>` —— 每条 vertanim 的 `speed` 通道。
    decay: f32,
}

/// 给没有扩展名的文件名补 `.smd`（官方 `Load_Source(name, "SMD")`）。
///
/// 官方 `Load_Source`（`studiomdl.cpp:1522`）无条件拼 `"%s%s.%s"` ——
/// 所以 `"ipf"` 变成 `"ipf.smd"`，而 `"ipf.smd"` 会变成 `"ipf.smd.smd"`
/// （官方先 `Q_StripExtension` 才拼，所以不会重复）。
fn ensure_smd_ext(name: &str) -> String {
    let lower = name.to_ascii_lowercase();
    if lower.ends_with(".smd") || lower.ends_with(".dmx") || lower.ends_with(".vrm") {
        name.to_string()
    } else {
        format!("{name}.smd")
    }
}

/// 把骨骼表排成「父骨骼必定在子骨骼之前」的顺序。
///
/// # 为什么需要（oracle 实测）
///
/// `BuildGlobalBonetable`（`simplify.cpp:3616`）先把 `$definebone` 的骨骼
/// 按**书写序**插入，然后才并入各 SMD 的骨骼。于是可能出现
/// **父在子之后**的排列：
///
/// ```text
/// ipq8.qc:  $definebone "b" "a" 0 0 10 …   ← b 先插入，父是 a
/// ipr.smd:  nodes = root, a, b             ← a 后插入
/// ```
///
/// 而官方 `ipq8.mdl` 的骨骼表是 **`BONE[0]=root, [1]=a, [2]=b`**
/// —— 官方在 `BuildGlobalBonetable` 之后有一步按父链重排，
/// 保证 `parent < index` 这个引擎不变量（否则引擎解算父链会读到未初始化项）。
///
/// mdlc 的 `ModelDesc::validate()` 也要求这条不变量，所以在解析收尾时
/// 做一次**稳定**拓扑排序：保持原有相对顺序，只把父提到子之前。
///
/// 悬空父引用（父不在表里）**不在这里报错** —— 交给 `validate()`，
/// 它能给出带路径的可读错误。
fn sort_parents_first(bones: Vec<Bone>) -> Vec<Bone> {
    let present: HashSet<String> = bones
        .iter()
        .map(|b| b.name.to_ascii_lowercase())
        .collect();
    let mut sorted: Vec<Bone> = Vec::with_capacity(bones.len());
    let mut placed: HashSet<String> = HashSet::new();
    loop {
        if sorted.len() == bones.len() {
            break;
        }
        let mut progressed = false;
        for b in &bones {
            let key = b.name.to_ascii_lowercase();
            if placed.contains(&key) {
                continue;
            }
            let ready = match b.parent.as_deref() {
                None => true,
                Some(p) => {
                    let pk = p.to_ascii_lowercase();
                    !present.contains(&pk) || placed.contains(&pk)
                }
            };
            if ready {
                placed.insert(key);
                sorted.push(b.clone());
                progressed = true;
            }
        }
        if !progressed {
            // 存在环（A 的父是 B、B 的父是 A）—— 原样接上，
            // 交给 `validate()` 报「父骨骼必须更靠前」。
            for b in &bones {
                if !placed.contains(&b.name.to_ascii_lowercase()) {
                    sorted.push(b.clone());
                }
            }
            break;
        }
    }
    sorted
}

/// 一个 SMD 里 `finish()` 需要的信息。
struct SmdInfo {
    /// `nodes` 段的骨骼名（按下标序）。
    nodes: Vec<String>,
    /// 三角形段里材质名的首现序。
    materials: Vec<String>,
    /// `skeleton` 段的帧数。
    num_frames: usize,
}
/// 把字符串解析成 `f32`（官方 `verify_atof`）。
///
/// 官方 `verify_atof` 用 `atof` —— 它**只读前导数字**（`"30fps"` → 30、
/// `"abc"` → 0），且接受 `.5` 这种没有整数部分的形式。
/// 这里先试严格解析，失败再取前导数字前缀。
fn parse_f32(s: &str) -> Option<f32> {
    let t = s.trim();
    if let Ok(v) = t.parse::<f32>() {
        return Some(v);
    }
    // 前导数字前缀（模拟 `atof` 的宽容）。
    let end = t
        .find(|c: char| {
            !(c.is_ascii_digit() || c == '.' || c == '-' || c == '+' || c == 'e' || c == 'E')
        })
        .unwrap_or(t.len());
    t[..end].parse::<f32>().ok()
}

/// 把字符串解析成 `i32`（官方 `verify_atoi` / `atoi`）。
///
/// 官方 `verify_atoi` 用 `atoi` —— 它**只读前导数字**，
/// 所以 `"30fps"` 得到 30、`"abc"` 得到 0。这里先试严格整数，
/// 失败再试浮点截断，最后回落到 0（与 `atoi` 一致）。
fn parse_i32(s: &str) -> Option<i32> {
    let t = s.trim();
    t.parse::<i32>()
        .ok()
        .or_else(|| t.parse::<f32>().ok().map(|v| v as i32))
}

/// 解析 `.vrd`（`$proceduralbones` 指向的文件）。
///
/// 结构见 `PROGRESS.md` §22.1（**两层**：`proctype 2` 的骨骼 + 触发器表）。
fn parse_vrd(text: &str) -> Result<Vec<QuatInterpBone>, String> {
    let mut out: Vec<QuatInterpBone> = Vec::new();
    let mut cur: Option<QuatInterpBone> = None;
    for raw in text.split(['\n', '\r']) {
        let line = raw.split("//").next().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }
        let mut it = line.split_whitespace();
        let Some(key) = it.next() else { continue };
        match key.to_ascii_lowercase().as_str() {
            "bone" => {
                if let Some(b) = cur.take() {
                    out.push(b);
                }
                let name = it.next().unwrap_or("").to_string();
                let control = it.next().unwrap_or("").to_string();
                cur = Some(QuatInterpBone {
                    bone: name,
                    control,
                    base_pos: None,
                    triggers: Vec::new(),
                });
            }
            "basepos" | "base_pos" => {
                if let Some(b) = cur.as_mut() {
                    let v: Vec<f32> = it.filter_map(|x| x.parse().ok()).collect();
                    if v.len() >= 3 {
                        b.base_pos = Some([v[0], v[1], v[2]]);
                    }
                }
            }
            "trigger" => {
                if let Some(b) = cur.as_mut() {
                    let v: Vec<f32> = it.filter_map(|x| x.parse().ok()).collect();
                    if v.len() >= 7 {
                        b.triggers.push(QuatInterpTrigger {
                            tolerance: v[0],
                            trigger: [v[1], v[2], v[3]],
                            angles: [v[4], v[5], v[6]],
                            pos: if v.len() >= 10 {
                                Some([v[7], v[8], v[9]])
                            } else {
                                None
                            },
                        });
                    }
                }
            }
            _ => {}
        }
    }
    if let Some(b) = cur {
        out.push(b);
    }
    Ok(out)
}
