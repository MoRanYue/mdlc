//! 构建脚本 —— 只做一件事：把 `assets/mdlc.rc` 编译成 `.res` 并链进 exe。
//!
//! # 为什么需要它
//!
//! Windows 的 exe 图标不是「编译进代码」的，而是作为 `RT_GROUP_ICON` +
//! `RT_ICON` **资源**存在于 PE 文件的 `.rsrc` 段里。Rust 没有内置写资源段的
//! 手段，所以要走 `rc.exe`（Windows SDK 的资源编译器）+ 链接器参数这条路。
//!
//! # 为什么用 `embed-resource`
//!
//! 自己找 `rc.exe` 要处理：VS 各版本的安装路径、Windows Kits 注册表键、
//! `%INCLUDE%` 要指向 SDK 头文件目录、交叉编译到非 MSVC 目标时该改用
//! `windres` + `ar`。这些都是纯环境适配，与 mdlc 本身无关。
//!
//! # 为什么这里要自己判 `TARGET`（而不是全靠 `embed-resource`）
//!
//! `embed-resource` 用 `#[cfg(target_os = "windows")]` 选后端 —— 而构建脚本
//! 是**给宿主平台**编译的，那个 `target_os` 说的是**宿主**，不是我们要产出的
//! 目标。于是在「在 Windows 上交叉编译到 Linux」这种组合下，它会挑
//! `windows_msvc` 后端、真的编译一份 `.res`，然后发出
//! `cargo:rustc-link-arg-bins=<windows .res 的路径>` —— 一个 Linux 链接器
//! 拿到这个参数只会报错。
//!
//! 本仓库的 CI（`.github/workflows/artifacts.yml`）是**每个目标跑在自己的
//! 原生 runner 上**，不交叉编译，所以碰不到这条路径。但判一句 `TARGET`
//! 几乎没有成本，却把「哪天有人加了交叉编译矩阵」这个隐患直接消掉。
//!
//! # 失败策略（刻意分级）
//!
//! 关键难点：`embed-resource` 在 MSVC 上把「找不到 `rc.exe`」和「`rc.exe`
//! 编译失败」**都报成 `Failed`** —— 前者是环境缺失（不该拦构建），后者是
//! 我们自己的 `.rc` / `.ico` 写坏了（必须炸）。它自己的错误文案把这两种
//! 情形分得很清楚（`"Are you sure you have RC.EXE in your $PATH or
//! ${RC_$TARGET} or $RC is set?"` vs `"RC.EXE failed to compile specified
//! resource file"`），但按文案匹配太脆。
//!
//! 所以这里**先自己查一遍 `rc.exe` 在不在**（用它公开的
//! `find_windows_sdk_tool`，外加它文档化的 `RC` / `RC_<target>` 环境变量
//! 与 `PATH` 兜底），据此把结果分成三档：
//!
//! - **目标不是 Windows** —— 静默跳过。图标是 Windows 专有概念，
//!   Linux/macOS 构建不该因此失败（CI 的 `test` job 就跑在 `ubuntu-latest`）。
//! - **目标 Windows 但找不到资源编译器** —— 只是「环境没装 SDK」（或从非
//!   Windows 交叉编译过来），**只发 `cargo:warning`**，不中断构建。
//! - **编译器在，编译却失败** —— 这一定是 `mdlc.rc` 或 `mdlc.ico` 的问题，
//!   **直接 panic**，绝不让「图标悄悄丢失」蒙混过关。
//!
//! 一句话：环境缺失可以容忍，我们自己的资源文件出错必须炸。
//!
//! ⚠️ **非 MSVC 的 Windows 目标**（`*-pc-windows-gnu`）走的是 `windres` /
//! `llvm-rc` 那条后端，`is_supported()` **会**正常返回错误信息，于是
//! `embed-resource` 报 `NotAttempted` 而不是 `Failed` —— 那种情形不需要
//! 预检查，`match` 里直接当环境缺失处理。

use std::path::Path;

fn main() {
    // ⚠️ 这两条必须写。cargo 的规则是：**只要**构建脚本输出了任意一条
    // `rerun-if-changed`，默认的「任何文件变化都重跑」就被关掉，改由这里
    // 列出的路径决定。漏掉 `mdlc.ico` 的话，换图标后 cargo 不会重跑
    // 构建脚本，exe 里会一直留着旧图标 —— 而且 `cargo build` 会报
    // `Finished`，看不出任何异常。
    println!("cargo:rerun-if-changed=assets/mdlc.rc");
    println!("cargo:rerun-if-changed=assets/mdlc.ico");

    // `TARGET` 由 cargo 提供给构建脚本，是**产物**的目标三元组（`HOST` 才是宿主）。
    let target = std::env::var("TARGET").expect("cargo 必定设置 TARGET");
    if !target.contains("windows") {
        return;
    }

    if !rc_available(&target) {
        println!(
            "cargo:warning=跳过图标资源：找不到 rc.exe（缺 Windows SDK，或在非 Windows 宿主上交叉编译）—— 产物将没有图标"
        );
        return;
    }

    // 传路径而不是依赖工作目录：构建脚本的 CWD 是包根目录，但写绝对路径
    // 能少一个「换个方式调用 cargo 就失效」的隐患。
    let manifest_dir =
        std::env::var("CARGO_MANIFEST_DIR").expect("cargo 必定设置 CARGO_MANIFEST_DIR");
    let rc = Path::new(&manifest_dir).join("assets").join("mdlc.rc");

    // 走到这里说明资源编译器是在的（MSVC 上由 `rc_available` 预先确认，
    // 非 MSVC 上缺 `windres` / `llvm-rc` 会报 `NotAttempted` 而不是
    // `Failed`），所以 `Failed` 一定是我们自己的资源文件有问题。
    match embed_resource::compile(&rc, embed_resource::NONE) {
        // `NotWindows` 已被上面的 TARGET 判断排除，这里一并接受以防万一。
        embed_resource::CompilationResult::Ok | embed_resource::CompilationResult::NotWindows => {}
        // 环境不具备编译资源的能力（非 MSVC 的 Windows 目标上缺 `windres` /
        // `llvm-rc`）。与「找不到 `rc.exe`」同类 —— 环境缺失，只警告。
        embed_resource::CompilationResult::NotAttempted(missing) => {
            println!("cargo:warning=跳过图标资源：{missing} —— 产物将没有图标");
        }
        embed_resource::CompilationResult::Failed(message) => panic!(
            "图标资源编译失败：{message}\n\
             资源编译器是能找到的，所以这多半是 assets/mdlc.rc 或 assets/mdlc.ico 本身写坏了。"
        ),
    }
}

/// `rc.exe` 在不在、能不能跑？
///
/// 查法与 `embed-resource` 内部**逐字对应**（它的错误文案是
/// `"Are you sure you have RC.EXE in your $PATH or ${RC_$TARGET} or $RC is set?"`）：
/// 先 `RC_<target>`（连字符与下划线两种拼法）、再 `RC`、再 Windows SDK 发现、
/// 最后裸 `rc.exe`（靠 `PATH`）。见其 `src/windows_msvc.rs:33-34` 与
/// `src/lib.rs:643`。
///
/// ⚠️ 不能只判环境变量**存不存在** —— 设了 `RC` 却指向一个不存在的文件时，
/// `embed-resource` 会照样拿它去执行然后失败，而我们若只看 `is_some()`
/// 就会误判成「rc.exe 在」，把环境问题当成资源文件问题而 panic。
/// 所以这里解析出**候选路径之后真的试着启动一次**。
///
/// 只在 MSVC 目标上做这个判断 —— 非 MSVC 的 Windows 目标走 `windres` /
/// `llvm-rc`，那条后端的 `is_supported()` **会**如实返回错误，所以缺编译器
/// 时 `embed-resource` 自己就报 `NotAttempted`，不需要在这里预先查。
fn rc_available(target: &str) -> bool {
    if !target.ends_with("-msvc") {
        return true;
    }

    // 与 `env_target_and_rc()` 同序：RC_<target> → RC_<target 下划线版> → RC。
    let from_env = std::env::var_os(format!("RC_{target}"))
        .or_else(|| std::env::var_os(format!("RC_{}", target.replace('-', "_"))))
        .or_else(|| std::env::var_os("RC"));

    // `embed-resource` 把 SDK 发现放在环境变量**之后**，这里保持同序。
    let resolved = from_env
        .map(std::path::PathBuf::from)
        .or_else(|| embed_resource::find_windows_sdk_tool("rc.exe"));

    // 环境变量指向了不存在的文件 ⟹ 不能当成「在」，要继续往下找。
    if let Some(path) = resolved
        && is_runnable(&path)
    {
        return true;
    }

    // 最后一档：裸 `rc.exe`，由 `PATH` 解析。
    is_runnable(std::path::Path::new("rc.exe"))
}

/// 能不能把它启动起来。
///
/// `rc.exe /?` 在成功与失败两种情况下都会返回非零（它就是打印帮助后报错），
/// 所以我们只关心 `spawn` 成不成功 —— 那正是「文件在不在、能不能执行」。
fn is_runnable(program: &std::path::Path) -> bool {
    std::process::Command::new(program)
        .arg("/?")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok()
}
