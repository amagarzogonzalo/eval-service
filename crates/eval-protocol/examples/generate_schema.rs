//! Regenerate `protocol/eval-protocol-v1.schema.json`.
//! Run: `cargo run -p eval-protocol --example generate_schema`

fn main() {
    let json = eval_protocol::schema_json_pretty();
    let out = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../protocol/eval-protocol-v1.schema.json");
    std::fs::create_dir_all(out.parent().unwrap()).unwrap();
    std::fs::write(&out, &json).unwrap();
    println!("wrote {}", out.display());
}
