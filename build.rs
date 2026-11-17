use html_minifier::minify;
use std::env;
use std::fs;
use std::path::Path;

fn main() {
    println!("cargo:rerun-if-changed=src/web/index.html");

    let out_dir = env::var_os("OUT_DIR").unwrap();
    let dest_path = Path::new(&out_dir).join("index.html");

    let html_path = Path::new("src/web/index.html");
    if html_path.exists() {
        let html = fs::read_to_string(html_path).unwrap();
        fs::write(dest_path, minify(html).unwrap()).unwrap();
    }
}
