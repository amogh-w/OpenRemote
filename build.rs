//! Assembles the single-page web app from `assets/ui/*` into `$OUT_DIR/index.html`.
//! The page ships as one self-contained document (inline CSS, SVG sprite and script), so it
//! works offline on the LAN and needs no extra requests. `src/web.rs` embeds the result.

use std::{env, fs, path::Path};

fn main() {
    println!("cargo:rerun-if-changed=assets/ui");
    println!("cargo:rerun-if-changed=build.rs");

    let ui = Path::new("assets/ui");
    let read = |name: &str| fs::read_to_string(ui.join(name)).unwrap_or_else(|e| panic!("assets/ui/{name}: {e}"));

    let html = format!(
        "{head}<style>\n{palette}\n{css}\n</style>\n</head>\n<body>\n{icons}\n{body}\n<script>\n{js}\n</script>\n</body>\n</html>\n",
        head = read("head.html"),
        icons = read("icons.html"),
        body = read("body.html"),
        palette = read("palette.css"),
        css = read("app.css"),
        js = read("app.js"),
    );
    let out = Path::new(&env::var("OUT_DIR").expect("OUT_DIR")).join("index.html");
    fs::write(out, html).expect("write index.html");
}
