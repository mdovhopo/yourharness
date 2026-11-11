mod parser;
mod render;

use std::{env, fs, path::Path};

fn main() {
    let args: Vec<String> = env::args().collect();
    match args.get(1).map(String::as_str) {
        Some("build") => build(Path::new("content"), Path::new("public")),
        Some("serve") => {
            build(Path::new("content"), Path::new("public"));
            let port = args.get(2).and_then(|p| p.parse().ok()).unwrap_or(4000);
            println!("serving public/ on http://localhost:{port}");
        }
        _ => eprintln!("usage: orbit build | serve [port]"),
    }
}

fn build(from: &Path, to: &Path) {
    fs::create_dir_all(to).unwrap();
    let mut pages = Vec::new();
    for entry in fs::read_dir(from).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().map_or(false, |e| e == "md") {
            let source = fs::read_to_string(&path).unwrap();
            let page = parser::parse(&source);
            let out = to.join(path.file_stem().unwrap()).with_extension("html");
            fs::write(&out, render::page(&page)).unwrap();
            pages.push(page);
        }
    }
    pages.sort_by(|a, b| b.date.cmp(&a.date));
    fs::write(to.join("index.html"), render::index(&pages)).unwrap();
    println!("built {} pages", pages.len());
}
