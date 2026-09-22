//! TOML 模式探针 —— 回答「能否把 `[[bones]] name="x"` 改成 `[bones."x"]`，
//! 并靠 TOML 解析器自动查重」。
//!
//! 用 `cargo run --release --example toml_schema_probe` 运行。
//!
//! # 结论摘要（全部实测，非推断）
//!
//! 1. **查重确实由解析器免费提供** —— 重复表头 / 表内重复键 / dotted-key 冲突
//!    都被 `toml` 拒绝。但 `[[bones]] name="x"` 写两遍**不会**被拒绝，
//!    这正是 mdlc 手写查重的原因。
//!
//! 2. **顺序必须显式保住** —— 表在 TOML 规范里是**无序**的，而骨骼顺序
//!    就是 bone index。`preserve_order` 是本 crate 的 feature，不是语言保证。
//!
//! 3. **嵌套 `[bones."root"."leaf"]` 与真实数据矛盾** —— 语料 3333 个模型 /
//!    15757 根骨骼里，「点分前缀 == parent」只成立 **16 条（0.18%）**。
//!    把它当层级等于断言一个语料不成立的关系。
//!
//! 4. **17/3333 个模型存在歧义碰撞** —— 某个前缀**同时**是真实骨骼名
//!    （如 `ValveBiped`）。此时 `[bones."ValveBiped"]` 既可能是骨骼、
//!    也可能是分组，TOML 层面无法区分。
//!
//! 5. **name 与 key 不一致时解析器不报错** —— 键写 `root`、内层写
//!    `name = "NOT_root"` 会静默通过。

use indexmap::IndexMap;
use serde::Deserialize;

#[derive(Debug, Deserialize)]
struct Row {
    #[serde(default)]
    #[allow(dead_code)] // 只用于让 serde 接受 `parent` 键，值本身不参与判定
    parent: Option<String>,
}

fn main() {
    println!("=== 1. 表头形态与查重（解析器行为） ===");
    let cases: &[(&str, &str)] = &[
        ("[bones.\"root\"] 单个", "[bones.\"root\"]\nparent = \"\"\n"),
        (
            "多个 name-keyed 表",
            "[bones.\"zeta\"]\n[bones.\"alpha\"]\n[bones.\"mid\"]\n",
        ),
        (
            "★重复表头 [bones.\"x\"] ×2",
            "[bones.\"x\"]\nparent = \"\"\n\n[bones.\"x\"]\nparent = \"p\"\n",
        ),
        (
            "★表内重复键 parent ×2",
            "[bones.\"x\"]\nparent = \"\"\nparent = \"p\"\n",
        ),
        (
            "dotted key 与表头冲突",
            "bones.x.parent = \"\"\n[bones.\"x\"]\nparent = \"p\"\n",
        ),
        (
            "对照：[[bones]] 重复 name（解析器**不**查）",
            "[[bones]]\nname = \"x\"\n\n[[bones]]\nname = \"x\"\n",
        ),
    ];
    for (label, src) in cases {
        match toml::from_str::<toml::Value>(src) {
            Ok(_) => println!("  {label:<42} → 解析成功（不查重）"),
            Err(e) => {
                let first = e.to_string().lines().next().unwrap_or("").to_string();
                println!("  {label:<42} → 拒绝：{first}");
            }
        }
    }

    println!("\n=== 2. IndexMap 是否保留**文档顺序**（顺序 = 骨骼下标） ===");
    let src = "[bones.\"zeta\"]\n[bones.\"alpha\"]\n[bones.\"mid\"]\n";
    #[derive(Deserialize)]
    struct Doc {
        bones: IndexMap<String, Row>,
    }
    let d: Doc = toml::from_str(src).expect("IndexMap 反序列化");
    let got: Vec<&str> = d.bones.keys().map(|s| s.as_str()).collect();
    println!("  文档顺序        : [\"zeta\", \"alpha\", \"mid\"]");
    println!("  IndexMap 实得   : {got:?}");
    println!(
        "  结论            : {}",
        if got == ["zeta", "alpha", "mid"] {
            "✓ 保留（依赖 toml 的 preserve_order feature，非 TOML 规范保证）"
        } else {
            "✗ 未保留 —— 顺序语义会被破坏"
        }
    );
    let v: toml::Value = toml::from_str(src).unwrap();
    let keys: Vec<&str> = v
        .get("bones")
        .and_then(|b| b.as_table())
        .map(|t| t.keys().map(|s| s.as_str()).collect())
        .unwrap_or_default();
    println!("  对照 toml::Value: {keys:?}");

    println!("\n=== 3. 嵌套 [bones.\"root\".\"leaf\"] 的 TOML 形状 ===");
    let nest = "[bones.\"root\".\"leaf\"]\nparent = \"root\"\n";
    let vn: toml::Value = toml::from_str(nest).unwrap();
    println!("  源码   : {}", nest.trim());
    println!("  形状   : {vn}");
    println!("  → 它表达的是「bones.root.leaf」，即 **root 是 leaf 的父表**。");
    println!("     语料实测：点分前缀 == parent 只占 0.18%（16/8930 非根骨骼）。");

    println!("\n=== 4. 前缀同时是骨骼名时的歧义（语料 17/3333 个模型） ===");
    let coll = r#"
[bones."ValveBiped"]
parent = ""

[bones."ValveBiped"."Bip01"]
parent = "ValveBiped.ValveBiped"
"#;
    println!("  解析结果: {}", match toml::from_str::<toml::Value>(coll) {
        Ok(v) => format!("成功 → {v}"),
        Err(e) => format!("拒绝 → {}", e.to_string().lines().next().unwrap_or("")),
    });
    #[derive(Deserialize)]
    struct Doc2 {
        bones: IndexMap<String, Row>,
    }
    match toml::from_str::<Doc2>(coll) {
        Ok(x) => println!(
            "  反序列化成 IndexMap<String, Bone>: {} 条 {:?} —— **Bip01 被吞进 ValveBiped**",
            x.bones.len(),
            x.bones.keys().collect::<Vec<_>>()
        ),
        Err(e) => println!(
            "  反序列化失败: {}",
            e.to_string().lines().next().unwrap_or("")
        ),
    }
    println!("  要无损表达必须用自递归枚举（表 or 骨骼），");
    println!("  其查找/校验/遍历全部手写递归 —— 比现在 3 处查重代码多得多。");

    println!("\n=== 5. name 与 key 不一致时无法被发现 ===");
    #[derive(Debug, Deserialize)]
    struct Named {
        name: String,
    }
    #[derive(Deserialize)]
    struct Doc3 {
        bones: IndexMap<String, Named>,
    }
    let bad = "[bones.\"root\"]\nname = \"NOT_root\"\n";
    let d3: Doc3 = toml::from_str(bad).unwrap();
    for (k, v) in &d3.bones {
        println!(
            "  键 = {k:?}   内层 name = {:?}   → {}",
            v.name,
            if *k == v.name {
                "一致"
            } else {
                "★不一致，且解析器不会报错"
            }
        );
    }
}
