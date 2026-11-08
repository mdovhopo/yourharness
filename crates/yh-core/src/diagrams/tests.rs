//! Diagram parsing and layout, plus timing and malformed-input checks.

use super::layered::{self, LayerDirection, LayoutEdge, LayoutNode, Options};
use super::*;
use std::collections::{HashMap, HashSet};

fn metrics() -> TextMetrics {
    TextMetrics::monospaced(7.0, 16.0)
}

fn chart_of(source: &str) -> Flowchart {
    match parse(source) {
        Ok(Diagram::Flowchart(chart)) => chart,
        other => panic!("expected a flowchart, got {other:?}"),
    }
}

fn sequence_of(source: &str) -> SequenceDiagram {
    match parse(source) {
        Ok(Diagram::Sequence(diagram)) => diagram,
        other => panic!("expected a sequence diagram, got {other:?}"),
    }
}

fn lay_out(chart: &Flowchart) -> FlowchartGeometry {
    match layout(&Diagram::Flowchart(chart.clone()), &metrics()) {
        Geometry::Flowchart(geometry) => geometry,
        Geometry::Sequence(_) => unreachable!(),
    }
}

fn frames(geometry: &FlowchartGeometry) -> HashMap<String, Rect> {
    geometry.nodes.iter().map(|n| (n.node.id.clone(), n.frame)).collect()
}

// Flowchart parsing

#[test]
fn shapes_edges_and_labels() {
    let chart = chart_of(
        r#"flowchart LR
    %% a comment
    A[Start] --> B{Is it ok?}
    B -->|Yes| C([Done])
    B -- No --> D[(Database)]
    D -.-> E((Retry)) ==> A
    F[["Sub (routine)"]] --- G{{hex}}; G --o H>flag]
    I[/in/] --x J[\out\]
    K <--> L"#,
    );
    assert_eq!(chart.direction, Direction::LeftRight);
    let shapes: HashMap<&str, Shape> = chart.nodes.iter().map(|n| (n.id.as_str(), n.shape)).collect();
    assert_eq!(shapes["A"], Shape::Rectangle);
    assert_eq!(shapes["B"], Shape::Diamond);
    assert_eq!(shapes["C"], Shape::Stadium);
    assert_eq!(shapes["D"], Shape::Cylinder);
    assert_eq!(shapes["E"], Shape::Circle);
    assert_eq!(shapes["F"], Shape::Subroutine);
    assert_eq!(shapes["G"], Shape::Hexagon);
    assert_eq!(shapes["H"], Shape::Flag);
    assert_eq!(shapes["I"], Shape::Parallelogram);
    assert_eq!(shapes["J"], Shape::ParallelogramAlt);
    let label = |id: &str| chart.nodes.iter().find(|n| n.id == id).unwrap().label.clone();
    assert_eq!(label("F"), "Sub (routine)");
    assert_eq!(label("B"), "Is it ok?");

    let edge = |from: &str, to: &str| chart.edges.iter().find(|e| e.from == from && e.to == to).unwrap();
    assert_eq!(edge("B", "C").label.as_deref(), Some("Yes"));
    assert_eq!(edge("B", "D").label.as_deref(), Some("No"));
    assert_eq!(edge("D", "E").line, Line::Dotted);
    assert_eq!(edge("E", "A").line, Line::Thick);
    assert_eq!(edge("F", "G").end, Arrow::None);
    assert_eq!(edge("G", "H").end, Arrow::Circle);
    assert_eq!(edge("I", "J").end, Arrow::Cross);
    assert_eq!(edge("K", "L").start, Arrow::Arrow);
    assert_eq!(edge("K", "L").end, Arrow::Arrow);
}

#[test]
fn more_shapes() {
    let chart = chart_of("flowchart TD\nA(((x))) --> B[/t\\]\nB --> C[\\t/]\nC --> D(round)\nD ~~~ E");
    let shapes: HashMap<&str, Shape> = chart.nodes.iter().map(|n| (n.id.as_str(), n.shape)).collect();
    assert_eq!(shapes["A"], Shape::DoubleCircle);
    assert_eq!(shapes["B"], Shape::Trapezoid);
    assert_eq!(shapes["C"], Shape::TrapezoidAlt);
    assert_eq!(shapes["D"], Shape::Rounded);
    assert_eq!(chart.edges.last().unwrap().line, Line::Invisible);
}

#[test]
fn ampersands_chains_and_compact_syntax() {
    let chart = chart_of("graph TD\nA & B --> C --> D & E\nX-->Y\nmy-node-->other_node");
    assert_eq!(chart.direction, Direction::TopBottom);
    let pairs: HashSet<String> = chart.edges.iter().map(|e| format!("{}>{}", e.from, e.to)).collect();
    let expected: HashSet<String> = ["A>C", "B>C", "C>D", "C>E", "X>Y", "my-node>other_node"]
        .into_iter()
        .map(String::from)
        .collect();
    assert_eq!(pairs, expected);
}

#[test]
fn subgraphs_and_ignored_styling() {
    let chart = chart_of(
        r#"flowchart TB
    classDef hot fill:#f96
    subgraph api [API layer]
        direction LR
        A[Gateway]:::hot --> B[Service]
        subgraph db [Storage]
            C[(Postgres)]
        end
    end
    subgraph "Web app"
        W[UI]
    end
    W --> api
    B --> C
    style A fill:#bbf
    click A "https://example.com""#,
    );
    let titles: Vec<&str> = chart.subgraphs.iter().map(|s| s.title.as_str()).collect();
    assert_eq!(titles, ["API layer", "Storage", "Web app"]);
    assert_eq!(chart.subgraphs[0].node_ids, ["A", "B"]);
    assert_eq!(chart.subgraphs[0].direction, Some(Direction::LeftRight));
    assert_eq!(chart.subgraphs[1].parent.as_deref(), Some("api"));
    assert_eq!(chart.subgraphs[2].node_ids, ["W"]);
    // An edge into a subgraph lands on its first node.
    assert!(chart.edges.iter().any(|e| e.from == "W" && e.to == "A"));
    assert_eq!(chart.nodes.iter().find(|n| n.id == "A").unwrap().label, "Gateway");
}

#[test]
fn styling_is_honored() {
    let chart = chart_of(
        r#"flowchart LR
    classDef gap stroke:#e5484d,stroke-dasharray:4,stroke-width:2px
    classDef default fill:#f9f9f9
    classDef ok,good fill:green
    A[SSH open]:::gap --> B[Backups] --> C:::ok
    D --> E
    class D,E gap
    style B fill:#ff996680,stroke:rgb(10, 20, 30)
    subgraph risky [Risky]
        F
    end
    style risky stroke:red,stroke-dasharray: 5 5"#,
    );
    let node = |id: &str| chart.nodes.iter().find(|n| n.id == id).unwrap().clone();
    assert!(node("A").style.dashed && node("A").style.stroke_width == Some(2.0));
    assert_eq!(node("A").style.stroke, ShapeStyle::parse("stroke:#e5484d").stroke);
    assert_eq!(node("A").style.fill, ShapeStyle::parse("fill:#f9f9f9").fill); // from `default`
    assert_eq!(node("B").style.fill.unwrap().alpha, 128.0 / 255.0);
    assert_eq!(
        node("B").style.stroke,
        Some(DiagramColor::rgb(10.0 / 255.0, 20.0 / 255.0, 30.0 / 255.0))
    );
    assert_eq!(node("C").style.fill, DiagramColor::parse("green"));
    assert!(node("D").style.dashed && node("E").style.dashed);
    assert!(chart.subgraphs[0].style.dashed);
    assert_eq!(chart.subgraphs[0].style.stroke, DiagramColor::parse("red"));
    assert_eq!(DiagramColor::parse("#f96"), Some(DiagramColor::rgb(1.0, 0.6, 0.4)));
    assert_eq!(DiagramColor::parse("nonsense"), None);
    assert_eq!(DiagramColor::parse("rgb)("), None);
}

#[test]
fn labels_are_cleaned() {
    let chart =
        chart_of(r#"flowchart TD; A["Line one<br/>Line #quot;two#quot;"] --> B("**bold**")"#);
    assert_eq!(chart.nodes[0].label, "Line one\nLine \"two\"");
    assert_eq!(chart.nodes[1].label, "bold");
    assert_eq!(text::clean("<b>x</b> <BR > <span class=\"a\">y</span> &amp; <bad> <p>"), "x \n y &  <p>"); // `<bad>` starts with `b`
}

#[test]
fn errors_are_readable() {
    assert!(flowchart::parse("pie title Pets").is_err());
    assert!(flowchart::parse("flowchart TD\nsubgraph x\nA-->B").is_err());
    assert!(flowchart::parse("flowchart TD\nA -->").is_err());
    assert!(parse("pie title Pets").is_err());
}

// Layered layout

/// Edges between nodes outside a box don't bend through it.
fn assert_edges_avoid_foreign_boxes(geometry: &FlowchartGeometry) {
    let frames = frames(geometry);
    for b in &geometry.subgraphs {
        for edge in geometry.edges.iter().filter(|e| !e.is_self_loop) {
            let (Some(from), Some(to)) = (frames.get(&edge.edge.from), frames.get(&edge.edge.to)) else {
                continue;
            };
            if b.frame.intersects(from) || b.frame.intersects(to) {
                continue;
            }
            // Sample each segment, not just the bends: a straight run can cut through too.
            let inner = b.frame.inset(2.0, 2.0);
            let crosses = edge.points.windows(2).any(|pair| {
                (1..20).any(|step| {
                    let t = step as f64 / 20.0;
                    inner.contains_point(Point::new(
                        pair[0].x + (pair[1].x - pair[0].x) * t,
                        pair[0].y + (pair[1].y - pair[0].y) * t,
                    ))
                })
            });
            assert!(!crosses, "{}→{} runs through {}", edge.edge.from, edge.edge.to, b.subgraph.id);
        }
    }
}

fn assert_no_overlaps(frames: &[Rect]) {
    for i in 0..frames.len() {
        for j in i + 1..frames.len() {
            assert!(
                !frames[i].inset(1.0, 1.0).intersects(&frames[j].inset(1.0, 1.0)),
                "nodes {i} and {j} overlap"
            );
        }
    }
}

fn node_frames(geometry: &FlowchartGeometry) -> Vec<Rect> {
    geometry.nodes.iter().map(|n| n.frame).collect()
}

#[test]
fn flows_in_the_requested_direction() {
    for direction in [Direction::TopBottom, Direction::BottomTop, Direction::LeftRight, Direction::RightLeft] {
        let mut chart = chart_of("flowchart TD\nA --> B --> C\nA --> C");
        chart.direction = direction;
        let geometry = lay_out(&chart);
        let a = geometry.nodes[0].frame;
        let c = geometry.nodes[2].frame;
        match direction {
            Direction::TopBottom => assert!(a.mid_y() < c.mid_y()),
            Direction::BottomTop => assert!(a.mid_y() > c.mid_y()),
            Direction::LeftRight => assert!(a.mid_x() < c.mid_x()),
            Direction::RightLeft => assert!(a.mid_x() > c.mid_x()),
        }
        assert_no_overlaps(&node_frames(&geometry));
        for node in &geometry.nodes {
            assert!(node.frame.min_x() >= -0.5 && node.frame.min_y() >= -0.5);
            assert!(node.frame.max_x() <= geometry.size.width + 0.5);
            assert!(node.frame.max_y() <= geometry.size.height + 0.5);
        }
    }
}

#[test]
fn edges_start_and_end_on_their_nodes() {
    let chart = chart_of(
        "flowchart TD\nA[Start] --> B{Check} -->|yes| C((End))\nB --> D[Long label that wraps across more than one line for sure] --> C",
    );
    let geometry = lay_out(&chart);
    let frames = frames(&geometry);
    for edge in &geometry.edges {
        let from = frames[&edge.edge.from];
        let to = frames[&edge.edge.to];
        let start = *edge.points.first().unwrap();
        let end = *edge.points.last().unwrap();
        assert!(from.inset(-1.0, -1.0).contains_point(start));
        assert!(to.inset(-1.0, -1.0).contains_point(end));
        // On the outline, not the center.
        assert!((start.y - from.mid_y()).abs() > 1.0 || (start.x - from.mid_x()).abs() > 1.0);
    }
    let labelled = geometry.edges.iter().find(|e| e.edge.label.as_deref() == Some("yes")).unwrap();
    assert!(labelled.label_frame.is_some());
    assert!(geometry.nodes.iter().find(|n| n.node.id == "D").unwrap().label.lines.len() > 1);
}

#[test]
fn cycles_and_self_loops_still_lay_out() {
    let chart = chart_of("flowchart TD\nA --> B --> C --> A\nC --> C\nB --> D");
    let geometry = lay_out(&chart);
    assert_no_overlaps(&node_frames(&geometry));
    assert!(geometry.edges.iter().all(|e| e.points.len() >= 2));
    let looped = geometry.edges.iter().find(|e| e.edge.from == "C" && e.edge.to == "C").unwrap();
    assert_eq!(looped.points.len(), 4);
    assert!(looped.is_self_loop);
}

#[test]
fn layout_settles_without_crossings_when_possible() {
    // Two independent chains drawn in an order that invites a crossing.
    let chart = chart_of("flowchart TD\nA --> D\nB --> C\nA --> E\nB --> F");
    let nodes: Vec<LayoutNode> = chart.nodes.iter().map(|_| LayoutNode::new(Size::new(40.0, 20.0))).collect();
    let index: HashMap<&str, usize> = chart.nodes.iter().enumerate().map(|(i, n)| (n.id.as_str(), i)).collect();
    let edges: Vec<LayoutEdge> = chart
        .edges
        .iter()
        .map(|e| LayoutEdge { from: index[e.from.as_str()], to: index[e.to.as_str()], label_size: None })
        .collect();
    let result = layered::layout(&nodes, &edges, &[], LayerDirection::TopBottom, Options::default());
    for (i, a) in edges.iter().enumerate() {
        for b in &edges[i + 1..] {
            let (a1, a2) = (result.frames[a.from].mid_x(), result.frames[a.to].mid_x());
            let (b1, b2) = (result.frames[b.from].mid_x(), result.frames[b.to].mid_x());
            assert!((a1 - b1) * (a2 - b2) >= 0.0, "edges cross");
        }
    }
}

#[test]
fn subgraph_boxes_wrap_their_members() {
    let chart = chart_of(
        "flowchart LR
  subgraph outer [Outer]
    A --> B
    subgraph inner [Inner]
      C
    end
  end
  B --> C --> D",
    );
    let geometry = lay_out(&chart);
    let f = frames(&geometry);
    let outer = geometry.subgraphs.iter().find(|s| s.subgraph.id == "outer").unwrap();
    let inner = geometry.subgraphs.iter().find(|s| s.subgraph.id == "inner").unwrap();
    assert!(outer.frame.contains_rect(&f["A"]) && outer.frame.contains_rect(&f["B"]));
    assert!(outer.frame.contains_rect(&inner.frame));
    assert!(inner.frame.contains_rect(&f["C"]));
    assert!(!outer.frame.contains_rect(&f["D"]));
    assert_eq!(geometry.subgraphs[0].subgraph.id, "outer"); // drawn first, underneath
}

#[test]
fn nodes_outside_a_subgraph_stay_out_of_its_box() {
    let chart = chart_of(
        "flowchart TD
    W[Write JSON line to stdin] --> M
    subgraph CLI [Claude Code]
        M[Model turn] --> P{Needs permission?}
        P -->|Yes| A[Ask in app]
        P -->|No| R[Run tool]
        A -->|Allow| R
        R --> M
    end
    M -.->|stream-json| E[Event parser]
    E --> V[(Transcript store)]
    V --> UI[[Thread view]]",
    );
    let geometry = lay_out(&chart);
    let b = geometry.subgraphs[0].frame;
    for node in geometry.nodes.iter().filter(|n| !["M", "P", "A", "R"].contains(&n.node.id.as_str())) {
        assert!(!b.intersects(&node.frame), "{} overlaps the subgraph", node.node.id);
    }
    assert_no_overlaps(&node_frames(&geometry));
}

#[test]
fn sibling_subgraphs_dont_overlap() {
    for direction in [Direction::TopBottom, Direction::LeftRight] {
        let mut chart = chart_of(
            "flowchart LR
    subgraph GitHub
        D[Dependabot] -->|PRs| R[repos] --> GA[GitHub Actions]
    end
    subgraph VPS
        E[.env secrets] --> DC[docker compose]
        S[sshd :22] --> DC
    end
    I([Internet]) --> S
    L([Admin laptop]) -->|SSH| S
    L -.->|copied by hand| E
    GA -->|pull| DC",
        );
        chart.direction = direction;
        let geometry = lay_out(&chart);
        assert_eq!(geometry.subgraphs.len(), 2);
        assert!(!geometry.subgraphs[0].frame.intersects(&geometry.subgraphs[1].frame), "subgraph boxes overlap");
    }
}

/// The shape of a real system diagram that came out badly: nested subgraphs, a separate
/// external box, nodes outside everything, edges crossing every border.
const NESTED_SYSTEM: &str = "flowchart TB
    admin([Admin laptop])
    subgraph cloud [Cloud provider: node-1]
        fw{{Firewall: no inbound except SSH}}
        subgraph k8s [Kubernetes]
            subgraph ns1 [ns ingress]
                tunnel[Tunnel x2]
            end
            proxy[Ingress proxy]
            subgraph ns2 [ns app]
                app[App] --> db[(Postgres)]
                job[Migration job] --> db
            end
            gitops[GitOps controller]
        end
    end
    subgraph ext [External services]
        dns[DNS + Access]
        registry[(Image registry)]
        git[Git hosting]
    end
    admin -->|SSH / API tunnel| fw
    fw --> tunnel
    tunnel -->|outbound tunnel| dns
    dns -->|app.example.com| proxy
    proxy --> app
    gitops -->|pulls manifests| git
    gitops --> app
    registry -.->|images| app
    classDef gap stroke:#e5484d,stroke-dasharray:4
    class fw gap";

#[test]
fn nested_subgraphs_stay_separate_and_tidy() {
    for direction in [Direction::TopBottom, Direction::LeftRight] {
        let mut chart = chart_of(NESTED_SYSTEM);
        chart.direction = direction;
        let geometry = lay_out(&chart);
        let f = frames(&geometry);
        let by_id: HashMap<&str, &PlacedSubgraph> =
            geometry.subgraphs.iter().map(|s| (s.subgraph.id.as_str(), s)).collect();
        fn members(chart: &Flowchart, id: &str) -> HashSet<String> {
            let mut result: HashSet<String> =
                chart.subgraphs.iter().find(|s| s.id == id).unwrap().node_ids.iter().cloned().collect();
            for child in chart.subgraphs.iter().filter(|s| s.parent.as_deref() == Some(id)) {
                result.extend(members(chart, &child.id));
            }
            result
        }
        for placed in &geometry.subgraphs {
            let inside = members(&chart, &placed.subgraph.id);
            for (id, frame) in &f {
                if inside.contains(id) {
                    assert!(placed.frame.contains_rect(frame), "{id} sticks out of {}", placed.subgraph.id);
                } else {
                    assert!(!placed.frame.intersects(frame), "{} covers {id}", placed.subgraph.id);
                }
                // The title has its own strip.
                let title = Rect::new(
                    placed.frame.min_x() + 12.0,
                    placed.frame.min_y() + 6.0,
                    placed.title.size.width,
                    placed.title.size.height,
                );
                assert!(!title.intersects(frame), "{}'s title hits {id}", placed.subgraph.id);
            }
            for other in geometry.subgraphs.iter().filter(|o| o.subgraph.id != placed.subgraph.id) {
                let nested = placed.frame.contains_rect(&other.frame) || other.frame.contains_rect(&placed.frame);
                assert!(
                    nested || !placed.frame.intersects(&other.frame),
                    "{} overlaps {}",
                    placed.subgraph.id,
                    other.subgraph.id
                );
            }
        }
        assert!(by_id["k8s"].frame.contains_rect(&by_id["ns2"].frame));
        assert_no_overlaps(&node_frames(&geometry));
        assert_edges_avoid_foreign_boxes(&geometry);

        // Every edge moves one way along the flow, so the drawn curve can't loop back.
        let vertical = direction == Direction::TopBottom;
        for edge in geometry.edges.iter().filter(|e| !e.is_self_loop) {
            let along: Vec<f64> = edge.points.iter().map(|p| if vertical { p.y } else { p.x }).collect();
            let steps: Vec<f64> = along.windows(2).map(|w| w[1] - w[0]).collect();
            assert!(
                steps.iter().all(|&s| s >= -0.5) || steps.iter().all(|&s| s <= 0.5),
                "{}→{} doubles back",
                edge.edge.from,
                edge.edge.to
            );
        }
    }
}

/// A subgraph that both produces and receives: repos build images, and a bot commits the new
/// tag back into one of the repos.
const DELIVERY: &str = r#"flowchart LR
    subgraph repos [Repos]
        app[app]
        mig[migrations]
        infra["infra (Terraform + scripts)"]
        gitops[gitops]
    end
    app -->|"CI: test + integration"| imgA[(registry/app:sha)]
    mig -->|"CI: migrate throwaway DB, check grants"| imgB[(registry/migrations:sha)]
    imgA --> bot[image-update action]
    imgB --> bot
    bot -->|"App token, commit tag to main"| gitops
    gitops -->|reconcile| cluster[Cluster]
    infra -->|"terraform apply (manual)"| server[Server + firewall]
    infra -->|"install.sh steps 00-50"| k8s[K8s on node]
    infra -.->|"admin scripts: tunnel, access"| cluster"#;

#[test]
fn subgraph_that_feeds_itself_still_flows_forward() {
    let geometry = lay_out(&chart_of(DELIVERY));
    let f = frames(&geometry);
    // Left to right through the whole chain, not doubling back around the box.
    assert!(f["app"].mid_x() < f["imgA"].mid_x());
    assert!(f["imgA"].mid_x() < f["bot"].mid_x());
    assert!(f["bot"].mid_x() < f["gitops"].mid_x());
    assert!(f["gitops"].mid_x() < f["cluster"].mid_x());
    let b = geometry.subgraphs[0].frame;
    for id in ["imgA", "imgB", "bot", "cluster", "server", "k8s"] {
        assert!(!b.intersects(&f[id]), "the Repos box covers {id}");
    }
    for id in ["app", "mig", "infra", "gitops"] {
        assert!(b.contains_rect(&f[id]), "{id} sticks out of Repos");
    }
    assert_no_overlaps(&node_frames(&geometry));
    assert_edges_avoid_foreign_boxes(&geometry);
}

#[test]
fn subgraph_direction_applies_when_self_contained() {
    let geometry = lay_out(&chart_of(
        "flowchart TB
    subgraph row [Row]
        direction LR
        A --> B --> C
    end
    subgraph col [Column]
        direction LR
        D --> E
    end
    X --> D",
    ));
    let f = frames(&geometry);
    // `row` has no edges across its border, so it runs left to right.
    assert!(f["A"].mid_x() < f["B"].mid_x() && f["B"].mid_x() < f["C"].mid_x());
    assert!((f["A"].mid_y() - f["C"].mid_y()).abs() < 1.0);
    // `col` is linked from outside, so it follows the chart (top to bottom).
    assert!(f["D"].mid_y() < f["E"].mid_y());
    // The block's box still wraps what was laid out inside it.
    let row = geometry.subgraphs.iter().find(|s| s.subgraph.id == "row").unwrap();
    for id in ["A", "B", "C"] {
        assert!(row.frame.contains_rect(&f[id]));
    }
}

fn big_chart(count: usize) -> Flowchart {
    let mut source = String::from("flowchart TD\n");
    for i in 0..count {
        source += &format!("N{i}[Node {i}] --> N{}\n", (i * 7 + 3) % count);
        if i % 3 == 0 {
            source += &format!("N{i} -->|step| N{}\n", (i + 11) % count);
        }
    }
    chart_of(&source)
}

#[test]
fn large_graphs_are_fast() {
    let chart = big_chart(120);
    let start = std::time::Instant::now();
    let geometry = lay_out(&chart);
    let elapsed = start.elapsed();
    eprintln!("120-node layout: {elapsed:?}");
    // Generous for debug builds and shared CI machines.
    assert!(elapsed.as_millis() < 2_000, "took {elapsed:?}");
    assert_no_overlaps(&node_frames(&geometry));
}

#[test]
fn forty_nodes_lay_out_quickly() {
    let chart = big_chart(40);
    let runs = 5;
    let start = std::time::Instant::now();
    for _ in 0..runs {
        std::hint::black_box(lay_out(&chart));
    }
    let each = start.elapsed() / runs;
    eprintln!("40-node layout: {each:?}");
    // Release builds take well under 10 ms; debug ones are many times slower.
    let budget = if cfg!(debug_assertions) { 200 } else { 10 };
    assert!(each.as_millis() < budget, "took {each:?}");
}

#[test]
fn isotonic_regression_respects_order() {
    assert_eq!(layered::isotonic(&[1.0, 3.0, 2.0, 4.0], &[1.0; 4]), [1.0, 2.5, 2.5, 4.0]);
    assert_eq!(layered::isotonic(&[5.0, 1.0], &[3.0, 1.0]), [4.0, 4.0]);
}

#[test]
fn clipping_hits_each_outline() {
    let frame = Rect::new(0.0, 0.0, 100.0, 40.0);
    let right = Point::new(200.0, 20.0);
    assert_eq!(clip(frame, Boundary::Rectangle, right), Point::new(100.0, 20.0));
    assert_eq!(clip(frame, Boundary::Ellipse, right), Point::new(100.0, 20.0));
    let below = Point::new(50.0, 300.0);
    assert_eq!(clip(frame, Boundary::Diamond, below), Point::new(50.0, 40.0));
}

#[test]
fn crossings_are_counted() {
    // 0→3 and 1→2 cross once.
    let down = vec![vec![3], vec![2], vec![], vec![]];
    assert_eq!(layered::crossings(&[0, 1], &[2, 3], &down), 1);
    assert_eq!(layered::crossings(&[0, 1], &[3, 2], &down), 0);
}

// Sequence diagrams

const CHECKOUT: &str = r#"sequenceDiagram
    title Checkout
    autonumber
    actor U as User
    participant W as Web app
    participant A as API
    U->>+W: Click "Pay"
    W->>A: POST /payments
    activate A
    Note right of A: Validates the card
    alt card ok
        A-->>W: 201 Created
    else declined
        A--xW: 402
        W->>W: Show error
    end
    deactivate A
    loop every 2s
        W-)A: poll status
    end
    Note over W,A: Webhook arrives later
    W-->>-U: Done"#;

#[test]
fn parses_participants_messages_and_blocks() {
    let diagram = sequence_of(CHECKOUT);
    assert_eq!(diagram.title.as_deref(), Some("Checkout"));
    assert!(diagram.autonumber);
    let ids: Vec<&str> = diagram.participants.iter().map(|p| p.id.as_str()).collect();
    assert_eq!(ids, ["U", "W", "A"]);
    let labels: Vec<&str> = diagram.participants.iter().map(|p| p.label.as_str()).collect();
    assert_eq!(labels, ["User", "Web app", "API"]);
    assert!(diagram.participants[0].is_actor);

    let Item::Message(first) = &diagram.items[0] else { panic!("expected a message") };
    assert!(first.from == "U" && first.to == "W" && first.activates_target && first.head == ArrowHead::Filled);
    assert_eq!(first.text, "Click \"Pay\"");

    let blocks: Vec<&Block> = diagram
        .items
        .iter()
        .filter_map(|item| if let Item::Block(block) = item { Some(block) } else { None })
        .collect();
    let kinds: Vec<BlockKind> = blocks.iter().map(|b| b.kind).collect();
    assert_eq!(kinds, [BlockKind::Alt, BlockKind::Loop]);
    let section_labels: Vec<&str> = blocks[0].sections.iter().map(|s| s.label.as_str()).collect();
    assert_eq!(section_labels, ["card ok", "declined"]);
    let Item::Message(declined) = &blocks[0].sections[1].items[0] else { panic!("expected a message") };
    assert!(declined.head == ArrowHead::Cross && declined.line == LineStyle::Dotted);
    let Item::Message(poll) = &blocks[1].sections[0].items[0] else { panic!("expected a message") };
    assert_eq!(poll.head, ArrowHead::Open);

    let Some(Item::Message(last)) = diagram.items.last() else { panic!("expected a message") };
    assert!(last.deactivates_source && last.line == LineStyle::Dotted);
}

#[test]
fn other_arrows_and_notes() {
    let diagram = sequence_of(
        "sequenceDiagram\nA<<->>B: both\nA<<-->>B: dotted both\nA->B: plain\nA-->B: dotted\nA-xB: cross\nNote left of A: l\nNote over B: o\nbox Aqua Group\nparticipant C\nend\ncritical c\nA->>C: x\noption o\nA->>C: y\nend\npar p\nA->>B: 1\nand q\nA->>C: 2\nend\nbreak b\nA->>B: z\nend\nopt o\nA->>B: w\nend\nrect rgb(1,2,3)\nA->>B: v\nend",
    );
    let Item::Message(both) = &diagram.items[0] else { panic!() };
    assert!(both.bidirectional && both.line == LineStyle::Solid);
    let Item::Message(dotted_both) = &diagram.items[1] else { panic!() };
    assert!(dotted_both.bidirectional && dotted_both.line == LineStyle::Dotted);
    let Item::Message(plain) = &diagram.items[2] else { panic!() };
    assert_eq!(plain.head, ArrowHead::None);
    let Item::Note(left) = &diagram.items[5] else { panic!() };
    assert_eq!(left.placement, NotePlacement::LeftOf);
    let Item::Note(over) = &diagram.items[6] else { panic!() };
    assert_eq!(over.placement, NotePlacement::Over);
    let kinds: Vec<BlockKind> = diagram
        .items
        .iter()
        .filter_map(|item| if let Item::Block(block) = item { Some(block.kind) } else { None })
        .collect();
    assert_eq!(
        kinds,
        [BlockKind::Critical, BlockKind::Par, BlockKind::Break, BlockKind::Opt, BlockKind::Rect]
    );
    let geometry = sequence_layout::layout(&diagram, &metrics());
    assert_eq!(geometry.blocks.len(), 5);
}

#[test]
fn layout_keeps_reading_order_and_room() {
    let diagram = sequence_of(CHECKOUT);
    let geometry = sequence_layout::layout(&diagram, &metrics());
    let xs: Vec<f64> = geometry.participants.iter().map(|p| p.center_x).collect();
    assert!(xs.windows(2).all(|w| w[0] <= w[1]));
    // Messages go down the page in order.
    let ys: Vec<f64> = geometry.messages.iter().map(|m| m.y).collect();
    assert!(ys.windows(2).all(|w| w[0] <= w[1]));
    let numbers: Vec<Option<usize>> = geometry.messages.iter().map(|m| m.number).collect();
    let expected: Vec<Option<usize>> = (1..=geometry.messages.len()).map(Some).collect();
    assert_eq!(numbers, expected);
    // Every label fits between its endpoints.
    for message in geometry.messages.iter().filter(|m| !m.is_self) {
        assert!(message.label_frame.width <= (message.to_x - message.from_x).abs() + 1.0);
    }
    // Activation bars: U→+W (closed by -U at the end) and activate/deactivate A.
    assert_eq!(geometry.activations.len(), 2);
    assert_eq!(geometry.blocks.len(), 2);
    assert_eq!(geometry.notes.len(), 2);
    assert!(geometry.size.width > 0.0 && geometry.size.height > geometry.lifeline_top);
    for block in &geometry.blocks {
        assert!(block.frame.min_x() >= -0.5);
    }
}

/// Real Claude output (Haiku, with our diagram instruction), kept as a fixture.
const OAUTH: &str = r#"sequenceDiagram
    actor U as User (Browser)
    participant A as App Server
    participant S as Auth Server

    U->>A: 1. Click "Log in"
    A->>A: 2. Create PKCE verifier + challenge, state
    A-->>U: 3. Redirect to /authorize (client_id, redirect_uri, code_challenge, state)
    U->>S: 4. GET /authorize
    S->>U: 5. Login + consent page
    U->>S: 6. Credentials + consent
    S-->>U: 7. Redirect to app callback (code, state)
    U->>A: 8. GET /callback?code&state
    A->>A: 9. Verify state
    A->>S: 10. POST /token (code, code_verifier, client credentials)
    S->>S: 11. Verify code + PKCE
    S-->>A: 12. access_token, refresh_token, id_token
    A->>A: 13. Store refresh token server-side, create session cookie
    A-->>U: 14. Set session cookie, redirect to app

    Note over U,S: Later, access token expires
    U->>A: 15. API request (session cookie)
    A->>S: 16. POST /token (grant_type=refresh_token, refresh_token)
    S->>S: 17. Validate refresh token, rotate it
    S-->>A: 18. New access_token (+ new refresh_token)
    A->>A: 19. Store rotated refresh token
    A-->>U: 20. Response to original request
"#;

#[test]
fn real_model_output_parses_and_lays_out() {
    let diagram = sequence_of(OAUTH);
    let labels: Vec<&str> = diagram.participants.iter().map(|p| p.label.as_str()).collect();
    assert_eq!(labels, ["User (Browser)", "App Server", "Auth Server"]);
    let geometry = sequence_layout::layout(&diagram, &metrics());
    assert_eq!(geometry.messages.len(), 20);
    assert_eq!(geometry.notes.len(), 1);
}

#[test]
fn unbalanced_blocks_are_errors() {
    assert!(sequence::parse("sequenceDiagram\nloop x\nA->>B: hi").is_err());
    assert!(sequence::parse("sequenceDiagram\nA->>B hi").is_err());
    assert!(sequence::parse("sequenceDiagram\nend").is_err());
    assert!(sequence::parse("sequenceDiagram\nelse x").is_err());
}

#[test]
fn dispatches_by_header() {
    assert!(is_supported("flowchart LR\nA-->B"));
    assert!(is_supported("%% hi\nsequenceDiagram\nA->>B: x"));
    assert!(is_supported("---\ntitle: x\n---\ngraph TD\nA-->B"));
    assert!(!is_supported("pie title Pets\n\"Dogs\": 3"));
    assert!(parse("---\ntitle: x\n---\ngraph TD\nA-->B").is_ok(), "front matter should be skipped");
    assert!(parse("%%{init: {'theme': 'dark'}}%%\nflowchart LR\nA-->B").is_ok());
    assert!(parse("gantt\ntitle x").is_err(), "gantt isn't drawn");
    let geometry = layout(&parse(CHECKOUT).unwrap(), &metrics());
    assert!(geometry.size().width > 0.0);
}

// Malformed input

#[test]
fn garbage_never_panics() {
    let samples = [
        CHECKOUT,
        NESTED_SYSTEM,
        DELIVERY,
        OAUTH,
        "flowchart TD\nA[Start] --> B{Check} -->|yes| C((End))\nB --o D[(x)] -.-> E>f] ==> A\nC --> C",
        "flowchart LR\nsubgraph a\nsubgraph b\nX-->Y\nend\nend\nY-->a\nstyle a fill:rgb(1,2,3),stroke:#abc",
        "graph TD\nA(((ü))) --x B[\\é/] & C{{日本}}\n",
    ];
    let garbage = [
        "", "\n\n", "flowchart", "graph", "sequenceDiagram", "flowchart TD\n-->", "flowchart TD\nA -->|",
        "flowchart TD\nA[", "flowchart TD\nA[\"unterminated", "flowchart TD\nA --> B --", "flowchart TD\nend",
        "flowchart TD\nsubgraph\nend", "flowchart TD\nA:::", "flowchart TD\n:::x", "flowchart TD\nA-- x",
        "flowchart TD\nA -- --> B", "flowchart TD\nstyle", "flowchart TD\nclassDef", "flowchart TD\nclass A",
        "sequenceDiagram\nNote", "sequenceDiagram\nNote over: x", "sequenceDiagram\nNote left of: x",
        "sequenceDiagram\nparticipant", "sequenceDiagram\n->>: x", "sequenceDiagram\nA->>+: x",
        "sequenceDiagram\nactivate", "sequenceDiagram\nA->>-B: x\ndeactivate B\ndeactivate C",
        "---\n---", "---\nunclosed", "%%{", "<br>", "flowchart TD\nA[<b>x</b]",
        "flowchart TD\nA((x)) --> A", "flowchart TD\nA --> B\nB --> A\nA --> A",
    ];
    let check = |source: &str| {
        if let Ok(diagram) = parse(source) {
            let geometry = layout(&diagram, &metrics());
            let size = geometry.size();
            assert!(size.width.is_finite() && size.height.is_finite(), "{source:?}");
        }
        let _ = is_supported(source);
    };
    for source in garbage {
        check(source);
    }
    // Every prefix of the real samples, cut at any character.
    for sample in samples {
        let cuts: Vec<usize> = sample.char_indices().map(|(i, _)| i).chain([sample.len()]).collect();
        for &cut in &cuts {
            check(&sample[..cut]);
        }
        // And with characters dropped or swapped here and there.
        let chars: Vec<char> = sample.chars().collect();
        for step in [3usize, 7, 13] {
            let mangled: String = chars.iter().enumerate().filter(|(i, _)| i % step != 0).map(|(_, c)| c).collect();
            check(&mangled);
            let swapped: String = chars
                .iter()
                .map(|&c| match c {
                    '[' => ']',
                    ']' => '[',
                    '-' => '>',
                    '>' => '-',
                    other => other,
                })
                .collect();
            check(&swapped);
        }
    }
}

#[test]
fn edge_label_frames_and_ports_stay_on_canvas() {
    let geometry = lay_out(&chart_of(NESTED_SYSTEM));
    for edge in &geometry.edges {
        for point in &edge.points {
            assert!(point.x >= -0.5 && point.y >= -0.5);
            assert!(point.x <= geometry.size.width + 0.5 && point.y <= geometry.size.height + 0.5);
        }
        if let Some(frame) = edge.label_frame {
            assert!(Rect::new(0.0, 0.0, geometry.size.width, geometry.size.height).inset(-0.5, -0.5).contains_rect(&frame));
        }
    }
}
