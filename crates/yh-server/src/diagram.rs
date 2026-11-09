//! A Mermaid block drawn for a client: laid out by `yh_core::diagrams`, painted by the same
//! painter as the Mac's (`yh_core::diagrams::paint`), recorded as paths and labels
//! (`yh_proto::Drawing`) in colour roles the client resolves for its own light or dark.

use std::fmt::Write;
use yh_core::diagrams::paint::{self, Canvas, Ink, Role, Seg, LINE_HEIGHT};
use yh_core::diagrams::{self, Geometry, TextMetrics};
use yh_proto::{Drawing, DrawnLabel, DrawnPath};

/// The diagram drawn, or why it can't be (a kind not drawn, or a source that doesn't parse).
pub fn draw(source: &str) -> Result<Drawing, String> {
    let diagram = diagrams::parse(source).map_err(|e| e.to_string())?;
    let kind = match &diagram {
        diagrams::Diagram::Flowchart(_) => "Flowchart".to_string(),
        diagrams::Diagram::Sequence(s) => s.title.clone().unwrap_or_else(|| "Sequence".into()),
    };
    let geometry: Geometry = diagrams::layout(&diagram, &metrics());
    let mut recorder = Recorder { paths: vec![] };
    paint::paint(&mut recorder, &geometry);
    let labels = paint::labels(&geometry)
        .into_iter()
        .map(|l| DrawnLabel { lines: l.lines, x: l.x, y: l.y, width: l.width, size: l.size, line_height: l.line_height, weight: l.weight, color: ink(l.color) })
        .collect();
    let size = geometry.size();
    Ok(Drawing { width: size.width, height: size.height, kind, paths: recorder.paths, labels })
}

/// The system font's advances at the labels' size (13 pt), near enough for laying out: boxes
/// are sized to their words, and labels are drawn centred with room either side.
fn metrics() -> TextMetrics {
    TextMetrics::new(LINE_HEIGHT, |text| text.chars().map(advance).sum::<f64>().ceil())
}

fn advance(c: char) -> f64 {
    let em = 13.0;
    em * match c {
        ' ' => 0.28,
        'i' | 'l' | 'I' | '.' | ',' | ':' | ';' | '\'' | '|' | '!' => 0.27,
        'f' | 'j' | 'r' | 't' | '(' | ')' | '[' | ']' | '{' | '}' | '/' | '\\' => 0.36,
        '-' | '"' => 0.4,
        'm' | 'w' => 0.82,
        'M' | 'W' => 0.9,
        '@' | '%' => 0.85,
        'A'..='Z' => 0.66,
        '0'..='9' => 0.6,
        'a'..='z' => 0.53,
        c if c.is_ascii() => 0.55,
        // Wide scripts and emoji.
        c if (c as u32) >= 0x2E80 => 1.0,
        _ => 0.6,
    }
}

/// The painter's paths, recorded.
struct Recorder {
    paths: Vec<DrawnPath>,
}

impl Canvas for Recorder {
    fn path(&mut self, segs: &[Seg], fill: Option<Ink>, stroke: Option<(Ink, f32, bool)>) {
        let mut d = String::new();
        for seg in segs {
            let _ = match *seg {
                Seg::Move(x, y) => write!(d, "M {x:.2} {y:.2} "),
                Seg::Line(x, y) => write!(d, "L {x:.2} {y:.2} "),
                Seg::Arc((rx, ry), (x, y)) => write!(d, "A {rx:.2} {ry:.2} {x:.2} {y:.2} "),
                Seg::Close => write!(d, "Z "),
            };
        }
        // A fill of nothing (an outline drawn over) is left out.
        let fill = fill.filter(|f| f.alpha > 0.).map(ink);
        let (stroke, width, dashed) = match stroke {
            Some((i, w, dashed)) => (Some(ink(i)), w, dashed),
            None => (None, 0., false),
        };
        if fill.is_none() && stroke.is_none() {
            return;
        }
        self.paths.push(DrawnPath { d: d.trim_end().to_string(), fill, stroke, width, dashed });
    }
}

fn ink(i: Ink) -> yh_proto::Ink {
    let (role, rgba) = match i.role {
        Role::Text => (Some("text"), None),
        Role::Secondary => (Some("secondary"), None),
        Role::Canvas => (Some("canvas"), None),
        Role::Card => (Some("card"), None),
        Role::Accent => (Some("accent"), None),
        Role::Rgb(r, g, b) => (None, Some([r, g, b, 1.])),
    };
    yh_proto::Ink { role: role.map(str::to_string), rgba, alpha: i.alpha }
}

#[cfg(test)]
mod tests {
    #[test]
    fn draws_a_flowchart_and_a_sequence_and_says_why_not() {
        let d = super::draw("flowchart LR\n  A[Start] --> B{Ok?}\n  B -->|yes| C((Done))").unwrap();
        assert_eq!(d.kind, "Flowchart");
        assert!(d.width > 100. && d.height > 20.);
        assert!(d.labels.iter().any(|l| l.lines == ["Start"]));
        assert!(d.labels.iter().any(|l| l.lines == ["yes"]));
        assert!(d.paths.iter().any(|p| p.d.contains('A')), "a circle's arcs");
        let s = super::draw("sequenceDiagram\n  Alice->>Bob: Hi\n  Bob-->>Alice: Hello").unwrap();
        assert!(s.labels.iter().any(|l| l.lines == ["Hi"]));
        assert!(super::draw("pie\n  \"A\": 1").is_err());
    }
}
