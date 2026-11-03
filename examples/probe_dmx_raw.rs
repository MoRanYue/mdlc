//! 原始 DMX 结构浏览器：直接走 `datamodel`，不做任何语义解释。
//!
//! 用法：
//!   probe_dmx_raw <file.dmx>                 类普查（每个类出现几次、带哪些属性）
//!   probe_dmx_raw <file.dmx> --tree          打印元素树
//!   probe_dmx_raw <file.dmx> --class DmeLog  打印该类每个元素的属性明细
//!
//! 这个工具只用来「看清 BST 到底写了什么」—— `probe_dmx` 走的是已经解释过的
//! `DmxModel`，看不到动画/形态键那些还没接进 IR 的类。

use std::collections::{BTreeMap, HashSet};
use std::io::BufReader;

use datamodel::attribute::AttributeValue;
use datamodel::element::Element;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let tree = args.iter().any(|a| a == "--tree");
    let class = args
        .iter()
        .position(|a| a == "--class")
        .and_then(|i| args.get(i + 1))
        .cloned();
    let mut files: Vec<String> = Vec::new();
    let mut skip_next = false;
    for a in &args {
        if skip_next {
            skip_next = false;
            continue;
        }
        if a == "--class" {
            skip_next = true;
            continue;
        }
        if a.starts_with("--") {
            continue;
        }
        files.push(a.clone());
    }
    if files.is_empty() {
        eprintln!("用法：probe_dmx_raw <file.dmx> [--tree] [--class Name]");
        std::process::exit(2);
    }

    let mut failed = 0;
    for arg in files {
        println!("==== {arg} ====");
        let file = match std::fs::File::open(&arg) {
            Ok(f) => f,
            Err(e) => {
                println!("[错误] {e}");
                failed += 1;
                continue;
            }
        };
        let mut reader = BufReader::new(file);
        let (header, root) = match datamodel::deserialize(&mut reader) {
            Ok(v) => v,
            Err(e) => {
                println!("[错误] {e}");
                failed += 1;
                continue;
            }
        };
        println!(
            "[header] format={} version={}",
            header.format, header.format_version
        );
        println!("[root] class={}", root.get_class());

        // 去重摊平（datamodel 保留对象身份，共享元素只出现一次）
        let mut seen = HashSet::new();
        let mut flat = Vec::new();
        collect(&root, &mut seen, &mut flat);
        println!("[elements] {}", flat.len());

        if let Some(want) = &class {
            for el in &flat {
                if &*el.get_class() != want {
                    continue;
                }
                println!("-- {} --", el.get_id());
                for (k, a) in el.get_attributes().iter() {
                    println!("    {k}: {}", summarize(a.get_inner_value()));
                }
            }
            continue;
        }

        let mut census: BTreeMap<String, (usize, BTreeMap<String, String>)> = BTreeMap::new();
        for el in &flat {
            let name = el.get_class().to_string();
            let entry = census.entry(name).or_default();
            entry.0 += 1;
            for (k, a) in el.get_attributes().iter() {
                entry.1.insert(k.clone(), type_name(a.get_inner_value()));
            }
        }
        for (name, (count, attrs)) in &census {
            println!("  {name} ×{count}");
            for (k, t) in attrs {
                println!("      {k}: {t}");
            }
        }

        if tree {
            println!("[tree]");
            print_tree(&root, 0, 6);
        }
    }
    if failed > 0 {
        std::process::exit(1);
    }
}

fn collect(el: &Element, seen: &mut HashSet<String>, out: &mut Vec<Element>) {
    let id = el.get_id().to_string();
    if !seen.insert(id) {
        return;
    }
    out.push(el.clone());
    for (_, a) in el.get_attributes().iter() {
        match a.get_inner_value() {
            AttributeValue::Element(Some(child)) => collect(child, seen, out),
            AttributeValue::ElementArray(items) => {
                for child in items.iter().flatten() {
                    collect(child, seen, out);
                }
            }
            _ => {}
        }
    }
}

fn print_tree(el: &Element, depth: usize, limit: usize) {
    if depth > limit {
        return;
    }
    let pad = "  ".repeat(depth);
    let label = el
        .get_attribute("name")
        .and_then(|a| match a.get_inner_value() {
            AttributeValue::String(s) => Some(s.clone()),
            _ => None,
        })
        .unwrap_or_default();
    println!("{pad}{} {label:?}", el.get_class());
    for (_, a) in el.get_attributes().iter() {
        match a.get_inner_value() {
            AttributeValue::Element(Some(child)) => print_tree(child, depth + 1, limit),
            AttributeValue::ElementArray(items) => {
                for child in items.iter().flatten() {
                    print_tree(child, depth + 1, limit);
                }
            }
            _ => {}
        }
    }
}

fn type_name(v: &AttributeValue) -> String {
    match v {
        AttributeValue::Element(Some(_)) => "element".into(),
        AttributeValue::Element(None) => "element(null)".into(),
        AttributeValue::ElementArray(a) => format!("element[{}]", a.len()),
        AttributeValue::IntegerArray(a) => format!("int[{}]", a.len()),
        AttributeValue::FloatArray(a) => format!("float[{}]", a.len()),
        AttributeValue::StringArray(a) => format!("string[{}]", a.len()),
        AttributeValue::Vector2Array(a) => format!("vector2[{}]", a.len()),
        AttributeValue::Vector3Array(a) => format!("vector3[{}]", a.len()),
        AttributeValue::QuaternionArray(a) => format!("quat[{}]", a.len()),
        other => format!("{:?}", other.attribute_type()),
    }
}

fn summarize(v: &AttributeValue) -> String {
    let t = type_name(v);
    match v {
        AttributeValue::Integer(i) => format!("{t} = {i}"),
        AttributeValue::Float(f) => format!("{t} = {f}"),
        AttributeValue::Boolean(b) => format!("{t} = {b}"),
        AttributeValue::String(s) => format!("{t} = {s:?}"),
        AttributeValue::Vector2(v) => format!("{t} = {v:?}"),
        AttributeValue::Vector3(v) => format!("{t} = {v:?}"),
        AttributeValue::Quaternion(q) => format!("{t} = {q:?}"),
        AttributeValue::IntegerArray(a) if a.len() <= 24 => format!("{t} = {a:?}"),
        AttributeValue::FloatArray(a) if a.len() <= 12 => format!("{t} = {a:?}"),
        AttributeValue::StringArray(a) if a.len() <= 12 => format!("{t} = {a:?}"),
        AttributeValue::Vector2Array(a) if a.len() <= 12 => format!(
            "{t} = {:?}",
            a.iter().map(|v| [v.x, v.y]).collect::<Vec<_>>()
        ),
        AttributeValue::Vector3Array(a) if a.len() <= 6 => format!("{t} = {a:?}"),
        AttributeValue::Vector3Array(a) if a.len() <= 24 => format!(
            "{t} = {:?}",
            a.iter()
                .map(|v| [v.x, v.y, v.z])
                .collect::<Vec<_>>()
        ),
        AttributeValue::QuaternionArray(a) if a.len() <= 6 => format!("{t} = {a:?}"),
        AttributeValue::QuaternionArray(a) if a.len() <= 24 => format!(
            "{t} = {:?}",
            a.iter()
                .map(|q| [q.x, q.y, q.z, q.w])
                .collect::<Vec<_>>()
        ),
        AttributeValue::Time(v) => format!("{t} = {}", v.0),
        AttributeValue::TimeArray(a) if a.len() <= 24 => {
            format!("{t} = {:?}", a.iter().map(|x| x.0).collect::<Vec<_>>())
        }
        _ => t,
    }
}
