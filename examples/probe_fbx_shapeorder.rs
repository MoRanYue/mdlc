//! 把 FBX shape key 的 `offset_vertices` 顺序摊开，用于核对官方 vertanim 顺序。
//!
//! 用法：`cargo run --release --example probe_fbx_shapeorder -- <file.fbx>`

use std::path::PathBuf;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.is_empty() {
        eprintln!("用法：probe_fbx_shapeorder <file.fbx> [...]");
        std::process::exit(2);
    }
    for a in args {
        let path = PathBuf::from(&a);
        println!("==== {} ====", path.display());
        let opts = mdlc::fbx::FbxOpts::default();
        match mdlc::fbx::read(&path, "probe", &opts) {
            Ok(g) => {
                println!("shape_keys = {}", g.shape_keys.len());
                for (i, k) in g.shape_keys.iter().enumerate() {
                    println!(
                        "  [{i}] name={:?} offsets={} frame={}",
                        k.name,
                        k.vertex_index.len(),
                        mdlc::fbx::FbxGeometry::shape_key_frame(i)
                    );
                    println!("      vertex_index = {:?}", k.vertex_index);
                    println!("      pos_off = {:?}", k.position_offsets);
                    println!("      nrm_off = {:?}", k.normal_offsets);
                }
            }
            Err(e) => println!("错误：{e}"),
        }
    }
}
