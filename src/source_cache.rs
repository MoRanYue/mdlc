//! 「同一个源文件在一次编译里被读好几遍」的缓存。
//!
//! # 为什么需要它
//!
//! 一次 `mdlc build` 会把**同一个**源文件读好几遍：
//!
//! 1. QC 预解析（`src/qc/parse.rs`）要拿节点名与材质名，判断「引用的文件
//!    存在、内容能读」；
//! 2. 几何读取（[`crate::compile::read_source_geometry`]）要真正的网格与
//!    参考姿态；
//! 3. 动画读取（[`crate::compile::read_source_frames`]）要帧数据。
//!
//! 对 `.smd` 这三步是同一个解析结果；对 `.dmx` 更贵 —— 同一个文件会被
//! `datamodel::deserialize` 解析三遍。实测（25 万三角形 / 30 形态键 / 431 MiB
//! 的 BlenderSourceTools 导出）：每遍约 **700 ms**，三遍占整次编译的 **27%**。
//!
//! # 缓存键与失效
//!
//! 键是 `(路径, 文件长度, mtime)`，**不是**只有路径 —— 否则同一进程里
//! 「编一个模型 → 改源文件 → 再编第二个」会拿到旧数据。`mtime` 在某些文件
//! 系统上只有 1 秒分辨率，所以键里同时带上长度；两者都不变的原地改写仍然
//! 识别不出来，这是有意的取舍（`mdlc` 是**一次性 CLI**，进程结束缓存自然
//! 消失；库用户可以调 [`clear`] 主动清空）。
//!
//! # 为什么是 thread-local
//!
//! `compile()` 全程单线程；测试又是多线程并行的。用全局锁既没必要，也会让
//! 并行的测试互相污染（`cargo test` 里同一份夹具被多个测试读是常态）。
//!
//! # 只缓存「与选项无关」的格式
//!
//! `.smd` / `.dmx` 读出来是什么只取决于文件本身。`.fbx` / `.gltf` 不是 ——
//! `srcpart` / `srcscale` / `srcaxis` / `srcshapekey*` 这些选项会改变读出来
//! 的东西（见 [`crate::compile::SrcOpts`]），所以它们**不进缓存**：宁可慢，
//! 也不能让两个带不同 `src*` 选项的 `$model` 共用一份结果。

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::SystemTime;

/// 缓存键：路径 + 长度 + mtime。
///
/// 读不到元数据时（文件不存在）用 `len = 0` / `modified = None` 退化 ——
/// 那种情况下 `load()` 自己会报错，而**错误不进缓存**。
#[derive(Clone, PartialEq, Eq, Hash)]
struct Key {
    path: PathBuf,
    len: u64,
    modified: Option<SystemTime>,
}

fn key_of(path: &Path) -> Key {
    let meta = std::fs::metadata(path).ok();
    Key {
        path: path.to_path_buf(),
        len: meta.as_ref().map(std::fs::Metadata::len).unwrap_or(0),
        modified: meta.and_then(|m| m.modified().ok()),
    }
}

thread_local! {
    /// 几何读取的结果（`.smd` / `.dmx`）。
    static GEOMETRY: RefCell<HashMap<Key, Arc<crate::compile::SourceGeometry>>> =
        RefCell::new(HashMap::new());
    /// 动画读取的结果（`.dmx`；`.smd` 与几何是同一份，不走这张表）。
    static FRAMES: RefCell<HashMap<Key, Arc<crate::smd::Smd>>> = RefCell::new(HashMap::new());
}

/// 取（或算）某个源文件的**几何**读取结果。
///
/// 只在 `load()` 成功时写入缓存；失败不缓存，所以「第一次读坏了、修好再读」
/// 能立刻看到修复。
pub(crate) fn geometry<E>(
    path: &Path,
    load: impl FnOnce() -> Result<crate::compile::SourceGeometry, E>,
) -> Result<Arc<crate::compile::SourceGeometry>, E> {
    let key = key_of(path);
    if let Some(hit) = GEOMETRY.with(|cache| cache.borrow().get(&key).cloned()) {
        return Ok(hit);
    }
    let value = Arc::new(load()?);
    GEOMETRY.with(|cache| {
        cache.borrow_mut().insert(key, Arc::clone(&value));
    });
    Ok(value)
}

/// 取（或算）某个源文件的**动画帧**读取结果。
pub(crate) fn frames<E>(
    path: &Path,
    load: impl FnOnce() -> Result<crate::smd::Smd, E>,
) -> Result<Arc<crate::smd::Smd>, E> {
    let key = key_of(path);
    if let Some(hit) = FRAMES.with(|cache| cache.borrow().get(&key).cloned()) {
        return Ok(hit);
    }
    let value = Arc::new(load()?);
    FRAMES.with(|cache| {
        cache.borrow_mut().insert(key, Arc::clone(&value));
    });
    Ok(value)
}

/// 清空本线程的源文件缓存。
///
/// `mdlc` 自己不需要它（一次编译一个进程），但**库用户**在长驻进程里反复
/// 编译、或者想强制重读时用得上。已经拿到的 [`Arc`] 不会被回收 —— 它们
/// 只是不再被缓存引用。
pub fn clear() {
    GEOMETRY.with(|cache| cache.borrow_mut().clear());
    FRAMES.with(|cache| cache.borrow_mut().clear());
    // `.fbx` / `.gltf` 的**几何结果**不进缓存（受 `src*` 选项影响），但它们的
    // **解析结果**（`ufbx::Scene`）会 —— 那份与选项无关，见 `crate::fbx`。
    crate::fbx::clear_scene_cache();
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    /// 造一个空壳几何（缓存测试只关心「是不是同一份」，不关心内容）。
    fn empty_geometry() -> crate::compile::SourceGeometry {
        crate::compile::SourceGeometry {
            smd: Arc::new(crate::smd::Smd {
                version: 1,
                nodes: Vec::new(),
                frames: Vec::new(),
                triangles: Vec::new(),
            }),
            shape_keys: Vec::new(),
            untextured_meshes: Vec::new(),
            merged_meshes: Vec::new(),
            anim_stacks: Vec::new(),
        }
    }

    /// 一个只在临时目录里出现的文件名（避免和别的测试抢同一个路径）。
    fn scratch(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join("mdlc_source_cache");
        std::fs::create_dir_all(&dir).unwrap();
        dir.join(name)
    }

    /// 同一个文件读两次必须命中缓存 —— `load` 只跑一次，且返回同一个 `Arc`。
    #[test]
    fn the_same_file_is_loaded_once() {
        let path = scratch("once.smd");
        std::fs::write(&path, "version 1\n").unwrap();
        clear();

        let calls = Cell::new(0);
        let load = || {
            calls.set(calls.get() + 1);
            Ok::<_, ()>(empty_geometry())
        };
        let first = geometry(&path, load).unwrap();
        let second = geometry(&path, load).unwrap();
        assert_eq!(calls.get(), 1, "第二次必须命中缓存，不再调用 load");
        assert!(Arc::ptr_eq(&first, &second));

        clear();
        std::fs::remove_file(&path).ok();
    }

    /// 文件变了（长度或 mtime）就必须重读。
    #[test]
    fn a_changed_file_is_loaded_again() {
        let path = scratch("changed.smd");
        std::fs::write(&path, "version 1\n").unwrap();
        clear();

        let calls = Cell::new(0);
        let load = || {
            calls.set(calls.get() + 1);
            Ok::<_, ()>(empty_geometry())
        };
        let first = geometry(&path, load).unwrap();
        assert_eq!(calls.get(), 1);

        // 追加内容 ⟹ 长度必然变（不依赖 mtime 的分辨率）。
        std::fs::write(&path, "version 1\nnodes\n0 \"a\" -1\nend\n").unwrap();
        let second = geometry(&path, load).unwrap();
        assert_eq!(calls.get(), 2, "文件变了就必须重读");
        assert!(!Arc::ptr_eq(&first, &second));

        clear();
        std::fs::remove_file(&path).ok();
    }

    /// 读取失败不写入缓存 —— 修好之后必须能立刻读到新内容。
    #[test]
    fn a_failed_load_is_not_cached() {
        let path = scratch("failed.smd");
        std::fs::write(&path, "version 1\n").unwrap();
        clear();

        assert!(geometry(&path, || Err::<crate::compile::SourceGeometry, _>("坏")).is_err());
        let ok = geometry(&path, || Ok::<_, &str>(empty_geometry())).unwrap();
        assert_eq!(ok.smd.version, 1);

        clear();
        std::fs::remove_file(&path).ok();
    }

    /// 帧缓存是**另一张表**：同一个路径的几何与帧互不干扰。
    #[test]
    fn geometry_and_frames_are_cached_separately() {
        let path = scratch("two_tables.dmx");
        std::fs::write(&path, "x").unwrap();
        clear();

        let g = geometry(&path, || Ok::<_, ()>(empty_geometry())).unwrap();
        let f = frames(&path, || {
            Ok::<_, ()>(crate::smd::Smd {
                version: 1,
                nodes: Vec::new(),
                frames: Vec::new(),
                triangles: Vec::new(),
            })
        })
        .unwrap();
        // 两张表各存各的 ⟹ 帧不可能是几何里那份 `Arc` 的别名。
        assert!(!std::ptr::eq(Arc::as_ptr(&g.smd), Arc::as_ptr(&f)));

        clear();
        std::fs::remove_file(&path).ok();
    }
}
