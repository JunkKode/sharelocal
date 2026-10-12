fn main() {
    // The browser client is embedded into the binary at compile time. During a
    // Rust-only build (`cargo test`, `cargo check`) the Vite output may not
    // exist yet, so leave a placeholder rather than failing the build.
    let dist = std::path::Path::new("../dist");
    if !dist.join("index.html").exists() {
        let _ = std::fs::create_dir_all(dist);
        let _ = std::fs::write(
            dist.join("index.html"),
            "<!doctype html><meta charset=\"utf-8\"><title>ShareLocal</title>\
             <p>The ShareLocal web interface has not been built yet. Run <code>npm run build</code>.</p>",
        );
    }
    println!("cargo:rerun-if-changed=../dist/index.html");

    tauri_build::build()
}
