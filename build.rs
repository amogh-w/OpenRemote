//! Assembles the single-page web app from `assets/ui/*` into `$OUT_DIR/index.html`.
//! The page ships as one self-contained document (inline CSS, SVG sprite and scripts), so it
//! works offline on the LAN and needs no extra requests. `src/web.rs` embeds the result.
//!
//! The Catppuccin theme (flavor, accent, corner radius) comes from the kit's `kit/theme.js`, inlined in the head so it runs before first paint.
//!
//! `app.css` (Tailwind output, includes the Catppuccin palette) and `icons.html` (Lucide sprite) are generated
//! by `npm run build` and committed, so this build does not need Node.

use std::{env, fs, path::Path};

fn main() {
    println!("cargo:rerun-if-changed=assets/ui");
    println!("cargo:rerun-if-changed=build.rs");

    let ui = Path::new("assets/ui");
    let read = |name: &str| fs::read_to_string(ui.join(name)).unwrap_or_else(|e| panic!("assets/ui/{name}: {e}"));

    // The sprite file is a bare <svg> of <symbol>s; hide it so it takes no space.
    let icons = format!("<!-- Icons: Lucide (https://lucide.dev), ISC license, see THIRD_PARTY.md. Sprite built by npm run build. -->\n{}", read("icons.html")).replacen("<svg ", "<svg width=\"0\" height=\"0\" style=\"position:absolute\" aria-hidden=\"true\" focusable=\"false\" ", 1);

    let html = format!(
        "{head}<script>\n{theme}\n</script>\n<style>\n{css}\n</style>\n</head>\n<body>\n{icons}\n{body}\n<script>\n{kit}\n</script>\n<script>\n{link}\n</script>\n<script>\n{pair}\n</script>\n<script>\n{js}\n</script>\n</body>\n</html>\n",
        head = read("head.html"),
        theme = read("kit/theme.js"),
        css = read("app.css"),
        body = read("body.html"),
        kit = read("kit/ui.js"),
        link = read("remote-client.js"),
        pair = read("pair-client.js"),
        js = read("app.js"),
    );
    let out = Path::new(&env::var("OUT_DIR").expect("OUT_DIR")).join("index.html");
    fs::write(out, html).expect("write index.html");
}
