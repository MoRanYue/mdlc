//! 词法层探针：把 QC 分词结果 dump 出来，与官方 `scriplib.cpp` 的行为对照。
//!
//! 用法:
//!   cargo run --release --example qc_lex_probe -- <file.qc> [--max N]
//!
//! # 为什么需要这个探针
//!
//! 词法层有六条**反直觉**行为（见 `src/qc/mod.rs`），任何一条做错都会
//! 静默产出不同的 token 序列。用真实 QC（尤其 `bones.qci` 那种
//! 裸 `\r` 结尾的）跑一遍，比读源码更能确认。

use std::path::Path;

use mdlc::qc::lexer::Lexer;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some(path) = args.first() else {
        eprintln!("用法: qc_lex_probe <file.qc> [--max N]");
        std::process::exit(2);
    };
    let max: usize = args
        .iter()
        .position(|a| a == "--max")
        .and_then(|i| args.get(i + 1))
        .and_then(|s| s.parse().ok())
        .unwrap_or(40);

    let p = Path::new(path);
    let mut lex = Lexer::new(p.parent().unwrap_or(Path::new(".")).to_path_buf());
    if let Err(e) = lex.load(p) {
        eprintln!("加载失败: {e}");
        std::process::exit(1);
    }

    let mut n = 0usize;
    let mut total = 0usize;
    loop {
        match lex.next_token(true) {
            Ok(Some(t)) => {
                total += 1;
                if n < max {
                    println!(
                        "{:>5}:{:<4} {:?}",
                        t.line,
                        if t.quoted { "Q" } else { "" },
                        t.text
                    );
                    n += 1;
                }
            }
            Ok(None) => break,
            Err(e) => {
                eprintln!("错误: {e}");
                std::process::exit(1);
            }
        }
    }
    println!("---- token 总数: {total} ----");
    println!("加载过的文件: {:#?}", lex.loaded_files);
}
