use crate::parser::{Block, Page};

pub fn page(page: &Page) -> String {
    let mut body = String::new();
    for block in &page.blocks {
        body.push_str(&match block {
            Block::Heading(level, text) => format!("<h{level}>{text}</h{level}>\n"),
            Block::Paragraph(text) => format!("<p>{}</p>\n", inline(text)),
            Block::Code(code) => format!("<pre><code>{code}</code></pre>\n"),
            Block::List(items) => {
                let items: String = items.iter().map(|i| format!("<li>{}</li>", inline(i))).collect();
                format!("<ul>{items}</ul>\n")
            }
        });
    }
    layout(&page.title, &body)
}

pub fn index(pages: &[Page]) -> String {
    let items: String = pages
        .iter()
        .map(|p| format!("<li><a href=\"{}.html\">{}</a> <time>{}</time></li>\n", p.slug, p.title, p.date))
        .collect();
    layout("Posts", &format!("<ul class=\"posts\">\n{items}</ul>"))
}

/// `code` and **bold**.
fn inline(text: &str) -> String {
    let mut out = String::new();
    for (i, part) in text.split('`').enumerate() {
        if i % 2 == 1 {
            out.push_str(&format!("<code>{part}</code>"));
        } else {
            out.push_str(&part.replace("**", "<strong>"));
        }
    }
    out
}

fn layout(title: &str, body: &str) -> String {
    format!("<!doctype html>\n<html><head><meta charset=\"utf-8\"><title>{title}</title></head>\n<body>\n{body}\n</body></html>\n")
}
