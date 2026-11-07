//! A flowchart placed: every node, edge and subgraph with its frame or route.
//!
//! Subgraphs that something links into or out of are clusters in one layered layout: their
//! members rank with everything else, and border vertices keep each box a clean rectangle (see
//! `layered`). A self-contained subgraph, with no edge crossing its border, is laid out on its
//! own, in its own `direction`, and placed as one block.

use super::flowchart::{Direction, Edge, Flowchart, Node, Shape, Subgraph};
use super::layered::{
    self, Boundary, LayerDirection, LayoutCluster, LayoutEdge, LayoutNode, Options, clip,
};
use super::text::{TextMetrics, WrappedText};
use super::{Point, Rect, Size};
use std::collections::{BTreeMap, HashMap};

/// Node labels wrap at this width.
pub const MAX_LABEL_WIDTH: f64 = 190.0;
/// Edge labels wrap at this width.
pub const MAX_EDGE_LABEL_WIDTH: f64 = 150.0;
const PADDING: f64 = 16.0;
const TITLE_GAP: f64 = 8.0;
const TITLE_WIDTH: f64 = 320.0;

#[derive(Clone, Debug)]
pub struct PlacedNode {
    /// The node as parsed, with its resolved `style`.
    pub node: Node,
    /// The outline's bounding box; draw `node.shape` inside it.
    pub frame: Rect,
    /// Centered in `frame`.
    pub label: WrappedText,
}

#[derive(Clone, Debug)]
pub struct PlacedEdge {
    /// The edge as parsed: `line`, `start` and `end` say how to draw it.
    pub edge: Edge,
    /// From the source's outline to the target's, through any bends. Each step moves along
    /// the flow, so drawing it as flow-aligned curves never loops back. Can be empty when
    /// an edge couldn't be routed.
    pub points: Vec<Point>,
    pub label: WrappedText,
    /// Centered on the edge, with 5 pt of room left and right and 2 pt above and below the
    /// text; `None` when the edge has no label.
    pub label_frame: Option<Rect>,
    pub is_self_loop: bool,
    /// Whether its flow runs up and down; a block inside can flow differently.
    pub vertical: bool,
}

#[derive(Clone, Debug)]
pub struct PlacedSubgraph {
    /// The subgraph as parsed, with its resolved `style`.
    pub subgraph: Subgraph,
    pub frame: Rect,
    /// Drawn at the box's top-left, inside it: 12 pt in from the left, 6 pt down from the top.
    pub title: WrappedText,
    /// 0 for the outermost.
    pub depth: usize,
}

#[derive(Clone, Debug)]
pub struct FlowchartGeometry {
    pub size: Size,
    pub direction: Direction,
    pub nodes: Vec<PlacedNode>,
    pub edges: Vec<PlacedEdge>,
    /// Outermost first, so they draw underneath their children.
    pub subgraphs: Vec<PlacedSubgraph>,
}

pub(crate) fn layout(chart: &Flowchart, metrics: &TextMetrics) -> FlowchartGeometry {
    let mut node_index: HashMap<&str, usize> = HashMap::new();
    for (index, node) in chart.nodes.iter().enumerate() {
        node_index.entry(node.id.as_str()).or_insert(index);
    }
    let mut cluster_index: HashMap<&str, usize> = HashMap::new();
    for (index, subgraph) in chart.subgraphs.iter().enumerate() {
        cluster_index.entry(subgraph.id.as_str()).or_insert(index);
    }
    // A parent is always declared before its child; anything else is dropped, so the tree
    // can't loop.
    let cluster_parent: Vec<Option<usize>> = chart
        .subgraphs
        .iter()
        .enumerate()
        .map(|(index, subgraph)| {
            subgraph
                .parent
                .as_deref()
                .and_then(|parent| cluster_index.get(parent).copied())
                .filter(|&parent| parent < index)
        })
        .collect();
    let titles: Vec<WrappedText> = chart
        .subgraphs
        .iter()
        .map(|subgraph| WrappedText::wrap(&subgraph.title, TITLE_WIDTH, metrics))
        .collect();
    let labels: Vec<WrappedText> = chart
        .nodes
        .iter()
        .map(|node| WrappedText::wrap(&node.label, MAX_LABEL_WIDTH, metrics))
        .collect();
    let edge_labels: Vec<WrappedText> = chart
        .edges
        .iter()
        .map(|edge| {
            edge.label
                .as_deref()
                .map(|label| WrappedText::wrap(label, MAX_EDGE_LABEL_WIDTH, metrics))
                .unwrap_or_default()
        })
        .collect();
    // Edges whose ends both exist; the parser guarantees it, a hand-made chart might not.
    let ends: Vec<Option<(usize, usize)>> = chart
        .edges
        .iter()
        .map(|edge| Some((*node_index.get(edge.from.as_str())?, *node_index.get(edge.to.as_str())?)))
        .collect();

    // The subgraph tree: every cluster a node sits in, outermost first.
    let mut node_parent: Vec<Option<usize>> = vec![None; chart.nodes.len()];
    for (index, subgraph) in chart.subgraphs.iter().enumerate() {
        for id in &subgraph.node_ids {
            if let Some(&node) = node_index.get(id.as_str()) {
                node_parent[node] = Some(index);
            }
        }
    }
    let chain = |cluster: Option<usize>| -> Vec<usize> {
        let mut result = Vec::new();
        let mut current = cluster;
        while let Some(index) = current {
            result.push(index);
            current = cluster_parent[index];
        }
        result.reverse();
        result
    };
    let node_chains: Vec<Vec<usize>> = node_parent.iter().map(|&parent| chain(parent)).collect();
    let inside = |node: usize, cluster: usize| node_chains[node].contains(&cluster);

    // Self-contained subgraphs become blocks; only the outermost of nested ones.
    let self_contained: Vec<bool> = (0..chart.subgraphs.len())
        .map(|cluster| {
            (0..chart.nodes.len()).any(|node| inside(node, cluster))
                && !ends.iter().flatten().any(|&(a, b)| inside(a, cluster) != inside(b, cluster))
        })
        .collect();
    let blocks: Vec<usize> = (0..chart.subgraphs.len())
        .filter(|&cluster| {
            self_contained[cluster]
                && !chain(cluster_parent[cluster]).iter().any(|&c| self_contained[c])
        })
        .collect();
    let is_block = |cluster: usize| blocks.contains(&cluster);
    let block_of = |node: usize| node_chains[node].iter().copied().find(|&c| is_block(c));
    let is_inside_block = |cluster: usize| chain(Some(cluster)).iter().any(|&c| is_block(c));

    // Lay each block out on its own.
    let mut block_geometry: BTreeMap<usize, FlowchartGeometry> = BTreeMap::new();
    let mut block_edges: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
    for &cluster in &blocks {
        let members: Vec<usize> = (0..chart.nodes.len()).filter(|&n| inside(n, cluster)).collect();
        let parent_id = &chart.subgraphs[cluster].id;
        let subgraphs: Vec<Subgraph> = (0..chart.subgraphs.len())
            .filter(|&c| c != cluster && chain(Some(c)).contains(&cluster))
            .map(|c| {
                let mut subgraph = chart.subgraphs[c].clone();
                if subgraph.parent.as_ref() == Some(parent_id) {
                    subgraph.parent = None;
                }
                subgraph
            })
            .collect();
        let edges: Vec<usize> = (0..chart.edges.len())
            .filter(|&e| ends[e].is_some_and(|(a, b)| inside(a, cluster) && inside(b, cluster)))
            .collect();
        let inner = Flowchart {
            direction: chart.subgraphs[cluster].direction.unwrap_or(chart.direction),
            nodes: members.iter().map(|&n| chart.nodes[n].clone()).collect(),
            edges: edges.iter().map(|&e| chart.edges[e].clone()).collect(),
            subgraphs,
        };
        block_edges.insert(cluster, edges);
        block_geometry.insert(cluster, layout(&inner, metrics));
    }

    // The layered graph at this level: free nodes, then one vertex per block.
    let free_nodes: Vec<usize> = (0..chart.nodes.len()).filter(|&n| block_of(n).is_none()).collect();
    let layout_clusters: Vec<usize> = (0..chart.subgraphs.len())
        .filter(|&c| !is_inside_block(c) && !is_block(c))
        .collect();
    let mut cluster_slot: Vec<Option<usize>> = vec![None; chart.subgraphs.len()];
    for (slot, &cluster) in layout_clusters.iter().enumerate() {
        cluster_slot[cluster] = Some(slot);
    }
    let slot = |cluster: Option<usize>| cluster.and_then(|c| cluster_slot[c]);

    let mut layout_nodes: Vec<LayoutNode> = free_nodes
        .iter()
        .map(|&node| LayoutNode {
            size: node_size(chart.nodes[node].shape, labels[node].size),
            boundary: boundary(chart.nodes[node].shape),
            cluster: slot(node_parent[node]),
        })
        .collect();
    let mut block_vertex: BTreeMap<usize, usize> = BTreeMap::new();
    for &cluster in &blocks {
        let inner = block_geometry[&cluster].size;
        let title = &titles[cluster];
        block_vertex.insert(cluster, layout_nodes.len());
        layout_nodes.push(LayoutNode {
            size: Size::new(
                inner.width.max(title.size.width) + PADDING * 2.0,
                inner.height + PADDING * 2.0 + title.size.height + TITLE_GAP,
            ),
            boundary: Boundary::Rectangle,
            cluster: slot(cluster_parent[cluster]),
        });
    }
    let mut vertex_of_node: Vec<Option<usize>> = vec![None; chart.nodes.len()];
    for (vertex, &node) in free_nodes.iter().enumerate() {
        vertex_of_node[node] = Some(vertex);
    }
    // Edges inside blocks were laid out with the block; the rest are routed here.
    let level_edges: Vec<(usize, usize, usize)> = (0..chart.edges.len())
        .filter_map(|e| {
            let (a, b) = ends[e]?;
            if block_of(a).is_some() {
                return None;
            }
            Some((e, vertex_of_node[a]?, vertex_of_node[b]?))
        })
        .collect();
    let result = layered::layout(
        &layout_nodes,
        &level_edges
            .iter()
            .map(|&(e, from, to)| {
                let label = &edge_labels[e];
                LayoutEdge {
                    from,
                    to,
                    label_size: (!label.is_empty())
                        .then(|| Size::new(label.size.width + 10.0, label.size.height + 4.0)),
                }
            })
            .collect::<Vec<_>>(),
        &layout_clusters
            .iter()
            .map(|&cluster| LayoutCluster {
                parent: slot(cluster_parent[cluster]),
                title_size: titles[cluster].size,
            })
            .collect::<Vec<_>>(),
        layered_direction(chart.direction),
        Options::default(),
    );

    // Nodes and subgraph boxes, with blocks' contents dropped into place.
    let mut nodes: Vec<Option<PlacedNode>> = vec![None; chart.nodes.len()];
    for (vertex, &node) in free_nodes.iter().enumerate() {
        nodes[node] = Some(PlacedNode {
            node: chart.nodes[node].clone(),
            frame: result.frames[vertex],
            label: labels[node].clone(),
        });
    }
    let mut subgraphs: Vec<PlacedSubgraph> = Vec::new();
    for (slot_index, &cluster) in layout_clusters.iter().enumerate() {
        if let Some(frame) = result.cluster_frames[slot_index] {
            subgraphs.push(PlacedSubgraph {
                subgraph: chart.subgraphs[cluster].clone(),
                frame,
                title: titles[cluster].clone(),
                depth: chain(Some(cluster)).len() - 1,
            });
        }
    }
    let mut edges: Vec<Option<PlacedEdge>> = vec![None; chart.edges.len()];
    for &cluster in &blocks {
        let frame = result.frames[block_vertex[&cluster]];
        let depth = chain(Some(cluster)).len() - 1;
        subgraphs.push(PlacedSubgraph {
            subgraph: chart.subgraphs[cluster].clone(),
            frame,
            title: titles[cluster].clone(),
            depth,
        });
        let inner = &block_geometry[&cluster];
        let dx = frame.min_x() + PADDING;
        let dy = frame.min_y() + PADDING + titles[cluster].size.height + TITLE_GAP;
        for placed in &inner.nodes {
            let mut placed = placed.clone();
            placed.frame = placed.frame.offset(dx, dy);
            if let Some(&index) = node_index.get(placed.node.id.as_str()) {
                nodes[index] = Some(placed);
            }
        }
        for placed in &inner.subgraphs {
            let mut placed = placed.clone();
            placed.frame = placed.frame.offset(dx, dy);
            placed.depth += depth + 1;
            if placed.subgraph.parent.is_none() {
                placed.subgraph.parent = Some(chart.subgraphs[cluster].id.clone());
            }
            subgraphs.push(placed);
        }
        // The block's edges come back in the order they went in.
        for (position, &index) in block_edges[&cluster].iter().enumerate() {
            let Some(placed) = inner.edges.get(position) else { break };
            let mut placed = placed.clone();
            for point in &mut placed.points {
                point.x += dx;
                point.y += dy;
            }
            placed.label_frame = placed.label_frame.map(|frame| frame.offset(dx, dy));
            edges[index] = Some(placed);
        }
    }

    // Edges at this level: the route, with its ends moved onto ports on the nodes.
    let vertical = chart.direction.is_vertical();
    let mut ports = PortAssigner::new(chart.direction);
    for (position, &(index, _, _)) in level_edges.iter().enumerate() {
        let Some((a, b)) = ends[index] else { continue };
        let points = &result.routes[position].points;
        if a == b || points.len() < 2 {
            continue;
        }
        let (Some(from), Some(to)) = (&nodes[a], &nodes[b]) else { continue };
        let (from_frame, to_frame) = (from.frame, to.frame);
        ports.add(PortKey { edge: index, outgoing: true }, a, from_frame, points[1]);
        ports.add(PortKey { edge: index, outgoing: false }, b, to_frame, points[points.len() - 2]);
    }
    let shapes: Vec<Boundary> = chart.nodes.iter().map(|node| boundary(node.shape)).collect();
    let assigned = ports.resolve(&shapes);
    for (position, &(index, _, _)) in level_edges.iter().enumerate() {
        let edge = &chart.edges[index];
        let route = &result.routes[position];
        let mut points = route.points.clone();
        if points.len() >= 2
            && let (Some(start), Some(end)) = (
                assigned.get(&PortKey { edge: index, outgoing: true }),
                assigned.get(&PortKey { edge: index, outgoing: false }),
            )
        {
            let last = points.len() - 1;
            points[0] = *start;
            points[last] = *end;
        }
        let label = edge_labels[index].clone();
        let mut label_frame = None;
        if !label.is_empty() && !points.is_empty() {
            let center = route.label_center.unwrap_or_else(|| midpoint(&points));
            label_frame = Some(Rect::new(
                center.x - label.size.width / 2.0 - 5.0,
                center.y - label.size.height / 2.0 - 2.0,
                label.size.width + 10.0,
                label.size.height + 4.0,
            ));
        }
        edges[index] = Some(PlacedEdge {
            edge: edge.clone(),
            points,
            label,
            label_frame,
            is_self_loop: edge.from == edge.to,
            vertical,
        });
    }

    // Loops and labels can poke out; keep everything on the canvas.
    let mut placed_nodes: Vec<PlacedNode> = nodes.into_iter().flatten().collect();
    let mut placed_edges: Vec<PlacedEdge> = edges.into_iter().flatten().collect();
    let mut bounds = Rect::new(0.0, 0.0, result.size.width, result.size.height);
    for node in &placed_nodes {
        bounds = bounds.union(&node.frame);
    }
    for subgraph in &subgraphs {
        bounds = bounds.union(&subgraph.frame);
    }
    for edge in &placed_edges {
        for point in &edge.points {
            bounds = bounds.union(&Rect::new(point.x, point.y, 0.0, 0.0));
        }
        if let Some(frame) = edge.label_frame {
            bounds = bounds.union(&frame);
        }
    }
    let (dx, dy) = (-bounds.min_x(), -bounds.min_y());
    for node in &mut placed_nodes {
        node.frame = node.frame.offset(dx, dy);
    }
    for edge in &mut placed_edges {
        for point in &mut edge.points {
            point.x += dx;
            point.y += dy;
        }
        edge.label_frame = edge.label_frame.map(|frame| frame.offset(dx, dy));
    }
    for subgraph in &mut subgraphs {
        subgraph.frame = subgraph.frame.offset(dx, dy);
    }
    subgraphs.sort_by_key(|subgraph| subgraph.depth);
    FlowchartGeometry {
        size: bounds.size(),
        direction: chart.direction,
        nodes: placed_nodes,
        edges: placed_edges,
        subgraphs,
    }
}

// Ports

/// `End` is the side further along the flow (bottom when flowing down).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Side {
    Start,
    End,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
struct PortKey {
    edge: usize,
    outgoing: bool,
}

struct PortRequest {
    key: PortKey,
    side: Side,
    cross: f64,
    frame: Rect,
}

/// Edges leave and enter a node on the side facing where they go, spread out in the order of
/// their other ends so they fan out instead of crossing at the node.
struct PortAssigner {
    vertical: bool,
    forward: f64,
    requests: BTreeMap<usize, Vec<PortRequest>>,
}

impl PortAssigner {
    fn new(direction: Direction) -> Self {
        PortAssigner {
            vertical: direction.is_vertical(),
            forward: if matches!(direction, Direction::TopBottom | Direction::LeftRight) {
                1.0
            } else {
                -1.0
            },
            requests: BTreeMap::new(),
        }
    }

    fn add(&mut self, key: PortKey, node: usize, frame: Rect, toward: Point) {
        let along = if self.vertical { toward.y - frame.mid_y() } else { toward.x - frame.mid_x() };
        let side = if along * self.forward >= 0.0 { Side::End } else { Side::Start };
        let cross = if self.vertical { toward.x } else { toward.y };
        self.requests.entry(node).or_default().push(PortRequest { key, side, cross, frame });
    }

    fn resolve(&self, shapes: &[Boundary]) -> HashMap<PortKey, Point> {
        let mut result = HashMap::new();
        for (&node, list) in &self.requests {
            for side in [Side::Start, Side::End] {
                let mut group: Vec<&PortRequest> = list.iter().filter(|r| r.side == side).collect();
                group.sort_by(|a, b| a.cross.total_cmp(&b.cross));
                let Some(frame) = group.first().map(|r| r.frame) else { continue };
                let shape = shapes.get(node).copied().unwrap_or(Boundary::Rectangle);
                let cross_size = if self.vertical { frame.width } else { frame.height };
                // Rectangles have a whole side to share; pointy shapes only their tip.
                let room = match shape {
                    Boundary::Rectangle => cross_size * 0.6,
                    Boundary::Ellipse => cross_size * 0.35,
                    Boundary::Diamond => cross_size * 0.15,
                };
                let count = group.len() as f64;
                let step = if group.len() > 1 { (room / (count - 1.0)).min(22.0) } else { 0.0 };
                let first = -step * (count - 1.0) / 2.0;
                let sign = if side == Side::End { 1.0 } else { -1.0 } * self.forward;
                for (position, request) in group.iter().enumerate() {
                    let offset = first + step * position as f64;
                    let point = if shape == Boundary::Rectangle {
                        if self.vertical {
                            Point::new(
                                frame.mid_x() + offset,
                                if sign > 0.0 { frame.max_y() } else { frame.min_y() },
                            )
                        } else {
                            Point::new(
                                if sign > 0.0 { frame.max_x() } else { frame.min_x() },
                                frame.mid_y() + offset,
                            )
                        }
                    } else {
                        let toward = if self.vertical {
                            Point::new(frame.mid_x() + offset * 2.0, frame.mid_y() + sign * frame.height)
                        } else {
                            Point::new(frame.mid_x() + sign * frame.width, frame.mid_y() + offset * 2.0)
                        };
                        clip(frame, shape, toward)
                    };
                    result.insert(request.key, point);
                }
            }
        }
        result
    }
}

// Shapes

fn layered_direction(direction: Direction) -> LayerDirection {
    match direction {
        Direction::TopBottom => LayerDirection::TopBottom,
        Direction::BottomTop => LayerDirection::BottomTop,
        Direction::LeftRight => LayerDirection::LeftRight,
        Direction::RightLeft => LayerDirection::RightLeft,
    }
}

fn boundary(shape: Shape) -> Boundary {
    match shape {
        Shape::Circle | Shape::DoubleCircle => Boundary::Ellipse,
        Shape::Diamond => Boundary::Diamond,
        _ => Boundary::Rectangle,
    }
}

/// The outline that fits a label of `label` size, with breathing room.
pub fn node_size(shape: Shape, label: Size) -> Size {
    let w = label.width.max(20.0);
    let h = label.height.max(16.0);
    match shape {
        Shape::Rectangle | Shape::Rounded => Size::new(w + 28.0, h + 18.0),
        Shape::Stadium => Size::new(w + 28.0 + h, h + 18.0),
        Shape::Subroutine => Size::new(w + 44.0, h + 18.0),
        Shape::Cylinder => Size::new(w + 28.0, h + 34.0),
        Shape::Circle => {
            let d = w.max(h) + 28.0;
            Size::new(d, d)
        }
        Shape::DoubleCircle => {
            let d = w.max(h) + 40.0;
            Size::new(d, d)
        }
        Shape::Diamond => {
            // Half-diagonals that contain the label's corners with a margin.
            let p = w / 2.0;
            let q = h / 2.0;
            Size::new(2.0 * (p + q + 14.0), 2.0 * (q + p * 0.5 + 12.0))
        }
        Shape::Hexagon
        | Shape::Parallelogram
        | Shape::ParallelogramAlt
        | Shape::Trapezoid
        | Shape::TrapezoidAlt => Size::new(w + 28.0 + h, h + 18.0),
        Shape::Flag => Size::new(w + 40.0, h + 18.0),
    }
}

fn midpoint(points: &[Point]) -> Point {
    let (Some(first), Some(last)) = (points.first(), points.last()) else {
        return Point::default();
    };
    if points.len() % 2 == 1 {
        return points[points.len() / 2];
    }
    if points.len() == 2 {
        return Point::new((first.x + last.x) / 2.0, (first.y + last.y) / 2.0);
    }
    let a = points[points.len() / 2 - 1];
    let b = points[points.len() / 2];
    Point::new((a.x + b.x) / 2.0, (a.y + b.y) / 2.0)
}
