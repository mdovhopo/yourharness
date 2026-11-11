/// A page: its front matter and Markdown body.
pub struct Page {
    pub title: String,
    pub date: String,
    pub slug: String,
    pub blocks: Vec<Block>,
}

pub enum Block {
    Heading(usize, String),
    Paragraph(String),
    Code(String),
    List(Vec<String>),
}

pub fn parse(source: &str) -> Page {
    let (front, body) = split_front_matter(source);
    let mut title = String::new();
    let mut date = String::new();
    for line in front.lines() {
        if let Some((key, value)) = line.split_once(':') {
            match key.trim() {
                "title" => title = value.trim().to_string(),
                "date" => date = value.trim().to_string(),
                _ => {}
            }
        }
    }
    let slug = title.to_lowercase().replace(' ', "-");
    Page { title, date, slug, blocks: blocks(body) }
}

fn split_front_matter(source: &str) -> (&str, &str) {
    if let Some(rest) = source.strip_prefix("---\n") {
        if let Some(end) = rest.find("---\n") {
            return (&rest[..end], &rest[end + 4..]);
        }
    }
    ("", source)
}

fn blocks(body: &str) -> Vec<Block> {
    let mut out = Vec::new();
    let mut lines = body.lines().peekable();
    while let Some(line) = lines.next() {
        if line.starts_with("```") {
            let mut code = String::new();
            while let Some(l) = lines.next() {
                if l.starts_with("```") {
                    break;
                }
                code.push_str(l);
                code.push('\n');
            }
            out.push(Block::Code(code));
        } else if line.starts_with('#') {
            let level = line.chars().take_while(|c| *c == '#').count();
            out.push(Block::Heading(level, line[level..].trim().to_string()));
        } else if line.starts_with("- ") {
            let mut items = vec![line[2..].to_string()];
            while lines.peek().is_some_and(|l| l.starts_with("- ")) {
                items.push(lines.next().unwrap()[2..].to_string());
            }
            out.push(Block::List(items));
        } else if !line.trim().is_empty() {
            out.push(Block::Paragraph(line.to_string()));
        }
    }
    out
}
