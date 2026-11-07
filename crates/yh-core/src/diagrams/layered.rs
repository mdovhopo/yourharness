//! Layered ("Sugiyama") graph layout with clusters, the same family of algorithm dagre and
//! Graphviz's `dot` use: break cycles, assign layers, insert dummy nodes for long edges, order
//! each layer (barycenter sweeps plus adjacent swaps) while keeping every cluster contiguous,
//! then place nodes with weighted isotonic regression so edges run straight.
//!
//! Clusters get invisible left and right border vertices on every layer they span. Borders
//! take part in ordering and placement like any vertex, which is what keeps each cluster a
//! clean rectangle that nothing else intrudes on.
//!
//! Everything is computed top-to-bottom; other directions are a transform at the end.

use super::{Point, Rect, Size};
use std::collections::{BTreeMap, HashMap, VecDeque};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum LayerDirection {
    TopBottom,
    BottomTop,
    LeftRight,
    RightLeft,
}

impl LayerDirection {
    fn is_horizontal(self) -> bool {
        matches!(self, LayerDirection::LeftRight | LayerDirection::RightLeft)
    }
}

/// The outline edges are clipped to.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Boundary {
    Rectangle,
    Ellipse,
    Diamond,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct LayoutNode {
    pub size: Size,
    pub boundary: Boundary,
    /// The innermost cluster this node is in.
    pub cluster: Option<usize>,
}

impl LayoutNode {
    #[cfg(test)]
    pub fn new(size: Size) -> Self {
        LayoutNode { size, boundary: Boundary::Rectangle, cluster: None }
    }
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct LayoutEdge {
    pub from: usize,
    pub to: usize,
    pub label_size: Option<Size>,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct LayoutCluster {
    pub parent: Option<usize>,
    /// Drawn at the cluster's top-left, inside its box.
    pub title_size: Size,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct Options {
    pub node_spacing: f64,
    pub edge_spacing: f64,
    pub rank_spacing: f64,
    pub cluster_padding: f64,
    pub cluster_gap: f64,
    pub title_gap: f64,
    pub sweeps: usize,
}

impl Default for Options {
    fn default() -> Self {
        Options {
            node_spacing: 36.0,
            edge_spacing: 14.0,
            rank_spacing: 52.0,
            cluster_padding: 16.0,
            cluster_gap: 28.0,
            title_gap: 8.0,
            sweeps: 12,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Route {
    /// From the source's boundary to the target's boundary, through any bends.
    pub points: Vec<Point>,
    pub label_center: Option<Point>,
}

#[derive(Clone, Debug)]
pub(crate) struct LayoutResult {
    /// By node index.
    pub frames: Vec<Rect>,
    /// By edge index.
    pub routes: Vec<Route>,
    /// By cluster index; `None` for clusters with no nodes.
    pub cluster_frames: Vec<Option<Rect>>,
    pub size: Size,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Border {
    Left,
    Right,
}

#[derive(Clone, Copy, Debug)]
struct Vertex {
    width: f64,
    height: f64,
    rank: usize,
    x: f64,
    is_dummy: bool,
    /// Innermost cluster; for a border vertex, the cluster it bounds.
    cluster: Option<usize>,
    border: Option<Border>,
}

impl Vertex {
    fn new(width: f64, height: f64, rank: usize, is_dummy: bool, cluster: Option<usize>) -> Self {
        Vertex { width, height, rank, x: 0.0, is_dummy, cluster, border: None }
    }
}

pub(crate) fn layout(
    nodes: &[LayoutNode],
    edges: &[LayoutEdge],
    clusters: &[LayoutCluster],
    direction: LayerDirection,
    options: Options,
) -> LayoutResult {
    if nodes.is_empty() {
        return LayoutResult {
            frames: Vec::new(),
            routes: edges.iter().map(|_| Route::default()).collect(),
            cluster_frames: clusters.iter().map(|_| None).collect(),
            size: Size::ZERO,
        };
    }
    let horizontal = direction.is_horizontal();
    // Work top-to-bottom; for left-to-right, a node's width runs along the layer.
    let internal_size =
        |size: Size| if horizontal { (size.height, size.width) } else { (size.width, size.height) };

    // Cluster tree helpers. A parent always comes before its child, but guard against a
    // malformed tree anyway.
    let chains: Vec<Vec<usize>> = (0..clusters.len())
        .map(|cluster| {
            let mut chain = Vec::new();
            let mut current = Some(cluster);
            while let Some(index) = current {
                if index >= clusters.len() || chain.contains(&index) || chain.len() > clusters.len() {
                    break;
                }
                chain.push(index);
                current = clusters[index].parent;
            }
            chain.reverse();
            chain
        })
        .collect();
    let chain = |cluster: Option<usize>| -> &[usize] {
        cluster.and_then(|c| chains.get(c)).map_or(&[], |chain| chain.as_slice())
    };
    let common_cluster = |a: Option<usize>, b: Option<usize>| -> Option<usize> {
        let mut shared = None;
        for (p, q) in chain(a).iter().zip(chain(b)) {
            if p != q {
                break;
            }
            shared = Some(*p);
        }
        shared
    };
    // Title room along the layers (vertical flow) or across them (horizontal flow).
    let title_along = |cluster: usize| {
        if horizontal { 0.0 } else { clusters[cluster].title_size.height + options.title_gap }
    };
    let title_across = |cluster: Option<usize>| match cluster {
        Some(cluster) if horizontal => clusters[cluster].title_size.height + options.title_gap,
        _ => 0.0,
    };

    let mut vertices: Vec<Vertex> = nodes
        .iter()
        .map(|node| {
            let (width, height) = internal_size(node.size);
            Vertex::new(width, height, 0, false, node.cluster.filter(|&c| c < clusters.len()))
        })
        .collect();
    let has_labels = edges.iter().any(|edge| edge.label_size.is_some() && edge.from != edge.to);
    let min_length = if has_labels { 2 } else { 1 };

    // 1. Break cycles: reverse edges that point back up a depth-first search.
    let real_edges: Vec<(usize, LayoutEdge)> = edges
        .iter()
        .copied()
        .enumerate()
        .filter(|(_, edge)| edge.from != edge.to && edge.from < nodes.len() && edge.to < nodes.len())
        .collect();
    let mut reversed = vec![false; edges.len()];
    {
        let mut adjacency: Vec<Vec<(usize, usize)>> = vec![Vec::new(); nodes.len()];
        for &(index, edge) in &real_edges {
            adjacency[edge.from].push((index, edge.to));
        }
        // 0 new, 1 on stack, 2 done. Iterative, so a long chain can't overflow the stack.
        let mut state = vec![0u8; nodes.len()];
        for root in 0..nodes.len() {
            if state[root] != 0 {
                continue;
            }
            state[root] = 1;
            let mut stack = vec![(root, 0usize)];
            while let Some(&mut (node, ref mut next_edge)) = stack.last_mut() {
                if let Some(&(edge, next)) = adjacency[node].get(*next_edge) {
                    *next_edge += 1;
                    if state[next] == 1 {
                        reversed[edge] = true;
                    } else if state[next] == 0 {
                        state[next] = 1;
                        stack.push((next, 0));
                    }
                } else {
                    state[node] = 2;
                    stack.pop();
                }
            }
        }
    }
    // (edge index, from, to), pointing down.
    let directed: Vec<(usize, usize, usize)> = real_edges
        .iter()
        .map(|&(index, edge)| {
            if reversed[index] { (index, edge.to, edge.from) } else { (index, edge.from, edge.to) }
        })
        .collect();

    // 2. Ranks: longest path from the sources, then pull sources down next to their first
    // child so they don't float at the top.
    let mut successors: Vec<Vec<usize>> = vec![Vec::new(); nodes.len()];
    let mut predecessors: Vec<Vec<usize>> = vec![Vec::new(); nodes.len()];
    for &(_, from, to) in &directed {
        successors[from].push(to);
        predecessors[to].push(from);
    }
    let mut indegree: Vec<usize> = predecessors.iter().map(Vec::len).collect();
    let mut queue: VecDeque<usize> = (0..nodes.len()).filter(|&n| indegree[n] == 0).collect();
    let mut topological = Vec::with_capacity(nodes.len());
    while let Some(node) = queue.pop_front() {
        topological.push(node);
        for &next in &successors[node] {
            vertices[next].rank = vertices[next].rank.max(vertices[node].rank + min_length);
            indegree[next] -= 1;
            if indegree[next] == 0 {
                queue.push_back(next);
            }
        }
    }
    for &node in topological.iter().rev() {
        if predecessors[node].is_empty() && !successors[node].is_empty() {
            let lowest = successors[node].iter().map(|&s| vertices[s].rank).min().unwrap_or(0);
            let lowest = lowest.saturating_sub(min_length);
            vertices[node].rank = vertices[node].rank.max(lowest);
        }
    }

    // 3. Long edges become chains through dummy vertices, one per layer crossed. A dummy
    // belongs to the innermost cluster holding both ends, so it never cuts through a box it
    // has no business in.
    let mut chains_by_edge: HashMap<usize, Vec<usize>> = HashMap::new();
    let mut label_vertex: HashMap<usize, usize> = HashMap::new();
    let mut down: Vec<Vec<usize>> = vec![Vec::new(); nodes.len()];
    let mut up: Vec<Vec<usize>> = vec![Vec::new(); nodes.len()];
    for &(index, from, to) in &directed {
        let mut path = vec![from];
        let from_rank = vertices[from].rank;
        let to_rank = vertices[to].rank;
        let span = to_rank.saturating_sub(from_rank);
        let label_rank = from_rank + span / 2;
        let home = common_cluster(vertices[from].cluster, vertices[to].cluster);
        for rank in from_rank + 1..to_rank {
            let mut dummy = Vertex::new(0.0, 0.0, rank, true, home);
            if rank == label_rank
                && let Some(label) = edges[index].label_size
            {
                (dummy.width, dummy.height) = internal_size(label);
                label_vertex.insert(index, vertices.len());
            }
            vertices.push(dummy);
            down.push(Vec::new());
            up.push(Vec::new());
            path.push(vertices.len() - 1);
        }
        path.push(to);
        for pair in path.windows(2) {
            down[pair[0]].push(pair[1]);
            up[pair[1]].push(pair[0]);
        }
        chains_by_edge.insert(index, path);
    }

    // 4. Cluster spans, and border vertices on every layer of each span.
    let mut span: Vec<Option<(usize, usize)>> = vec![None; clusters.len()];
    for vertex in &vertices {
        for &cluster in chain(vertex.cluster) {
            span[cluster] = Some(match span[cluster] {
                Some((lo, hi)) => (lo.min(vertex.rank), hi.max(vertex.rank)),
                None => (vertex.rank, vertex.rank),
            });
        }
    }
    let mut border_up: HashMap<usize, usize> = HashMap::new();
    let mut border_down: BTreeMap<usize, usize> = BTreeMap::new();
    let mut left_borders: Vec<Vec<usize>> = vec![Vec::new(); clusters.len()];
    let mut right_borders: Vec<Vec<usize>> = vec![Vec::new(); clusters.len()];
    for cluster in 0..clusters.len() {
        let Some((lo, hi)) = span[cluster] else { continue };
        for side in [Border::Left, Border::Right] {
            let mut previous: Option<usize> = None;
            for rank in lo..=hi {
                let mut vertex = Vertex::new(0.0, 0.0, rank, true, Some(cluster));
                vertex.border = Some(side);
                vertices.push(vertex);
                down.push(Vec::new());
                up.push(Vec::new());
                let index = vertices.len() - 1;
                if let Some(previous) = previous {
                    border_up.insert(index, previous);
                    border_down.insert(previous, index);
                }
                previous = Some(index);
                if side == Border::Left {
                    left_borders[cluster].push(index);
                } else {
                    right_borders[cluster].push(index);
                }
            }
        }
    }

    // 5. Order within layers.
    let rank_count = vertices.iter().map(|v| v.rank).max().unwrap_or(0) + 1;
    let mut layers: Vec<Vec<usize>> = vec![Vec::new(); rank_count];
    {
        // Initial order: depth-first from the sources, so related nodes start close.
        let mut placed = vec![false; vertices.len()];
        let mut place = |root: usize, layers: &mut Vec<Vec<usize>>| {
            if placed[root] {
                return;
            }
            placed[root] = true;
            layers[vertices[root].rank].push(root);
            let mut stack = vec![(root, 0usize)];
            while let Some(&mut (vertex, ref mut next)) = stack.last_mut() {
                if let Some(&child) = down[vertex].get(*next) {
                    *next += 1;
                    if !placed[child] {
                        placed[child] = true;
                        layers[vertices[child].rank].push(child);
                        stack.push((child, 0));
                    }
                } else {
                    stack.pop();
                }
            }
        };
        for vertex in 0..vertices.len() {
            if up[vertex].is_empty() && vertices[vertex].border.is_none() {
                place(vertex, &mut layers);
            }
        }
        for vertex in 0..vertices.len() {
            place(vertex, &mut layers);
        }
    }
    // For counting crossings, cluster walls are edges too: an edge cutting through a box
    // crosses its walls, so the ordering steers it around instead.
    let mut walls = down.clone();
    for (&upper, &lower) in &border_down {
        walls[upper].push(lower);
    }
    let ordering = Ordering { vertices: &vertices, chains: &chains, up: &up, down: &down, walls: &walls };
    layers = ordering.minimize(layers, options.sweeps);

    // 6. Coordinates across each layer.
    let separation = |vertices: &[Vertex], a: usize, b: usize| -> f64 {
        let va = vertices[a];
        let vb = vertices[b];
        let gap = match (va.border, vb.border) {
            (Some(Border::Right), Some(Border::Left)) => options.cluster_gap + title_across(vb.cluster),
            // Into a cluster: its padding, plus its title when titles sit across.
            (Some(Border::Left), _) => options.cluster_padding + title_across(va.cluster),
            (_, Some(Border::Right)) => options.cluster_padding,
            (Some(Border::Right), _) => options.node_spacing * 0.75,
            (_, Some(Border::Left)) => options.node_spacing * 0.75 + title_across(vb.cluster),
            _ => match (va.is_dummy, vb.is_dummy) {
                (true, true) => options.edge_spacing,
                (false, false) => options.node_spacing,
                _ => (options.node_spacing + options.edge_spacing) / 2.0,
            },
        };
        (va.width + vb.width) / 2.0 + gap
    };
    let place_layer = |vertices: &mut [Vertex], layer: &[usize], targets: &[f64], weights: &[f64]| {
        let mut offsets = Vec::with_capacity(layer.len());
        offsets.push(0.0);
        for position in 1..layer.len() {
            offsets.push(offsets[position - 1] + separation(vertices, layer[position - 1], layer[position]));
        }
        let shifted: Vec<f64> = targets.iter().zip(&offsets).map(|(t, o)| t - o).collect();
        let solved = isotonic(&shifted, weights);
        for (position, &vertex) in layer.iter().enumerate() {
            vertices[vertex].x = solved[position] + offsets[position];
        }
    };
    for layer in layers.iter().filter(|layer| !layer.is_empty()) {
        place_layer(&mut vertices, layer, &vec![0.0; layer.len()], &vec![1.0; layer.len()]);
    }
    let mut neighbour_xs: Vec<f64> = Vec::new();
    for iteration in 0..20 {
        let use_up = iteration % 2 == 0;
        let both = iteration >= 16;
        let ranks: Vec<usize> =
            if use_up { (0..layers.len()).collect() } else { (0..layers.len()).rev().collect() };
        for rank in ranks {
            let layer = &layers[rank];
            if layer.is_empty() {
                continue;
            }
            let mut targets = Vec::with_capacity(layer.len());
            let mut weights = Vec::with_capacity(layer.len());
            for &vertex in layer {
                if vertices[vertex].border.is_some() {
                    // Borders line up with themselves above and below: straight walls.
                    let neighbours: Vec<usize> = [border_up.get(&vertex), border_down.get(&vertex)]
                        .into_iter()
                        .flatten()
                        .copied()
                        .collect();
                    if neighbours.is_empty() {
                        targets.push(vertices[vertex].x);
                        weights.push(0.01);
                    } else {
                        let sum: f64 = neighbours.iter().map(|&n| vertices[n].x).sum();
                        targets.push(sum / neighbours.len() as f64);
                        weights.push(6.0);
                    }
                    continue;
                }
                neighbour_xs.clear();
                if both || use_up {
                    neighbour_xs.extend(up[vertex].iter().map(|&n| vertices[n].x));
                }
                if both || !use_up {
                    neighbour_xs.extend(down[vertex].iter().map(|&n| vertices[n].x));
                }
                if neighbour_xs.is_empty() {
                    targets.push(vertices[vertex].x);
                    weights.push(0.01);
                } else {
                    neighbour_xs.sort_by(f64::total_cmp);
                    targets.push(median(&neighbour_xs));
                    // Keep dummy chains (long edges) straight above all else.
                    let factor = if vertices[vertex].is_dummy { 4.0 } else { 1.0 };
                    weights.push(neighbour_xs.len() as f64 * factor);
                }
            }
            place_layer(&mut vertices, layer, &targets, &weights);
        }
    }

    // Square up: each cluster wall becomes one position shared by all its layers (the widest
    // point), then every layer's spacing is enforced in one longest-path pass over the
    // constraint graph. Clusters keep one order everywhere, so the graph is acyclic, and the
    // result has rectangular boxes with nothing intruding.
    let mut by_depth: Vec<usize> = (0..clusters.len()).filter(|&c| span[c].is_some()).collect();
    by_depth.sort_by(|a, b| chains[*b].len().cmp(&chains[*a].len()));
    let fold_min = |items: &[usize], vertices: &[Vertex]| {
        items.iter().map(|&v| vertices[v].x).fold(f64::INFINITY, f64::min)
    };
    let fold_max = |items: &[usize], vertices: &[Vertex]| {
        items.iter().map(|&v| vertices[v].x).fold(f64::NEG_INFINITY, f64::max)
    };
    if !by_depth.is_empty() {
        let vertex_count = vertices.len();
        let variable = |vertices: &[Vertex], vertex: usize| -> usize {
            match (vertices[vertex].border, vertices[vertex].cluster) {
                (Some(side), Some(cluster)) => {
                    vertex_count + cluster * 2 + if side == Border::Left { 0 } else { 1 }
                }
                _ => vertex,
            }
        };
        let count = vertex_count + clusters.len() * 2;
        let mut position = vec![0.0; count];
        for (index, vertex) in vertices.iter().enumerate() {
            if vertex.border.is_none() {
                position[index] = vertex.x;
            }
        }
        for &cluster in &by_depth {
            position[vertex_count + cluster * 2] = fold_min(&left_borders[cluster], &vertices);
            position[vertex_count + cluster * 2 + 1] = fold_max(&right_borders[cluster], &vertices);
        }
        let mut after: Vec<Vec<(usize, f64)>> = vec![Vec::new(); count];
        let mut indegree = vec![0usize; count];
        for layer in &layers {
            for pair in layer.windows(2) {
                let (a, b) = (variable(&vertices, pair[0]), variable(&vertices, pair[1]));
                after[a].push((b, separation(&vertices, pair[0], pair[1])));
                indegree[b] += 1;
            }
        }
        let mut ready: Vec<usize> = (0..count).filter(|&v| indegree[v] == 0).collect();
        let mut visited = 0;
        while let Some(current) = ready.pop() {
            visited += 1;
            for &(next, gap) in &after[current] {
                position[next] = position[next].max(position[current] + gap);
                indegree[next] -= 1;
                if indegree[next] == 0 {
                    ready.push(next);
                }
            }
        }
        if visited == count {
            for index in 0..vertex_count {
                vertices[index].x = position[variable(&vertices, index)];
            }
        }
    }
    let min_x = vertices.iter().map(|v| v.x - v.width / 2.0).fold(f64::INFINITY, f64::min);
    let min_x = if min_x.is_finite() { min_x } else { 0.0 };
    for vertex in &mut vertices {
        vertex.x -= min_x;
    }

    // 7. Layer positions down the page, with room for cluster padding and titles where
    // clusters open and close.
    let rank_spacing = if has_labels { options.rank_spacing / 2.0 } else { options.rank_spacing };
    let mut layer_height = vec![0.0f64; rank_count];
    for vertex in &vertices {
        layer_height[vertex.rank] = layer_height[vertex.rank].max(vertex.height);
    }
    let mut opening = vec![0.0; rank_count];
    let mut closing = vec![0.0; rank_count];
    for (cluster, ranks) in span.iter().enumerate() {
        if let Some((lo, hi)) = *ranks {
            opening[lo] += options.cluster_padding + title_along(cluster);
            closing[hi] += options.cluster_padding;
        }
    }
    let mut layer_y = vec![0.0; rank_count];
    let mut y = 0.0;
    for rank in 0..rank_count {
        y += if rank == 0 {
            opening[0] + layer_height[0] / 2.0
        } else {
            layer_height[rank - 1] / 2.0
                + closing[rank - 1]
                + rank_spacing
                + opening[rank]
                + layer_height[rank] / 2.0
        };
        layer_y[rank] = y;
    }
    let internal_height = y + layer_height[rank_count - 1] / 2.0 + closing[rank_count - 1];

    // Cluster boxes in internal coordinates, inner ones first so parents can wrap them.
    let mut internal_boxes: Vec<Option<Rect>> = vec![None; clusters.len()];
    for &cluster in &by_depth {
        let Some((lo, hi)) = span[cluster] else { continue };
        let left = fold_min(&left_borders[cluster], &vertices);
        let right = fold_max(&right_borders[cluster], &vertices);
        let mut top = layer_y[lo] - layer_height[lo] / 2.0;
        let mut bottom = layer_y[hi] + layer_height[hi] / 2.0;
        for child in 0..clusters.len() {
            if clusters[child].parent == Some(cluster)
                && let Some(child_box) = internal_boxes[child]
            {
                top = top.min(child_box.min_y());
                bottom = bottom.max(child_box.max_y());
            }
        }
        top -= options.cluster_padding + title_along(cluster);
        bottom += options.cluster_padding;
        internal_boxes[cluster] = Some(Rect::new(left, top, right - left, bottom - top));
    }
    let internal_width = vertices
        .iter()
        .map(|v| v.x + v.width / 2.0)
        .chain(internal_boxes.iter().flatten().map(Rect::max_x))
        .fold(0.0, f64::max);

    // 8. Back to the requested direction.
    let point = |x: f64, y: f64| match direction {
        LayerDirection::TopBottom => Point::new(x, y),
        LayerDirection::BottomTop => Point::new(x, internal_height - y),
        LayerDirection::LeftRight => Point::new(y, x),
        LayerDirection::RightLeft => Point::new(internal_height - y, x),
    };
    let center = |vertex: usize| point(vertices[vertex].x, layer_y[vertices[vertex].rank]);
    let rect = |r: Rect| {
        let a = point(r.min_x(), r.min_y());
        let b = point(r.max_x(), r.max_y());
        Rect::new(a.x.min(b.x), a.y.min(b.y), (a.x - b.x).abs(), (a.y - b.y).abs())
    };

    let frames: Vec<Rect> = nodes
        .iter()
        .enumerate()
        .map(|(index, node)| {
            let c = center(index);
            Rect::new(
                c.x - node.size.width / 2.0,
                c.y - node.size.height / 2.0,
                node.size.width,
                node.size.height,
            )
        })
        .collect();
    let mut routes = Vec::with_capacity(edges.len());
    for (index, edge) in edges.iter().enumerate() {
        if edge.from == edge.to && edge.from < nodes.len() {
            routes.push(self_loop(frames[edge.from], horizontal, edge.label_size));
            continue;
        }
        let Some(path) = chains_by_edge.get(&index) else {
            routes.push(Route::default());
            continue;
        };
        let mut points: Vec<Point> = path.iter().map(|&v| center(v)).collect();
        if reversed[index] {
            points.reverse();
        }
        let last = points.len() - 1;
        points[0] = clip(frames[edge.from], nodes[edge.from].boundary, points[1]);
        points[last] = clip(frames[edge.to], nodes[edge.to].boundary, points[last - 1]);
        routes.push(Route { points, label_center: label_vertex.get(&index).map(|&v| center(v)) });
    }

    let size = if horizontal {
        Size::new(internal_height, internal_width)
    } else {
        Size::new(internal_width, internal_height)
    };
    LayoutResult {
        frames,
        routes,
        cluster_frames: internal_boxes.into_iter().map(|b| b.map(rect)).collect(),
        size,
    }
}

// Crossing reduction

/// Barycenter ordering with cluster constraints: within every layer each cluster's vertices
/// stay together between its borders, and sibling clusters keep one relative order across all
/// layers so their boxes can't cross.
struct Ordering<'a> {
    vertices: &'a [Vertex],
    chains: &'a [Vec<usize>],
    up: &'a [Vec<usize>],
    down: &'a [Vec<usize>],
    /// `down` plus cluster walls, for counting crossings.
    walls: &'a [Vec<usize>],
}

impl Ordering<'_> {
    fn minimize(&self, initial: Vec<Vec<usize>>, sweeps: usize) -> Vec<Vec<usize>> {
        let mut keys = self.cluster_keys(&initial);
        let mut fraction = vec![0.0; self.vertices.len()];
        let mut layers: Vec<Vec<usize>> = initial
            .iter()
            .map(|layer| {
                let width = (layer.len().max(2) - 1) as f64;
                for (index, &vertex) in layer.iter().enumerate() {
                    fraction[vertex] = index as f64 / width;
                }
                self.arrange(layer, &fraction, &keys)
            })
            .collect();
        let mut best = layers.clone();
        let mut best_crossings = self.total_crossings(&layers);
        let mut position = vec![f64::NAN; self.vertices.len()];
        // A few sweeps even at zero crossings: clusters still need settling.
        for sweep in 0..sweeps {
            if !(best_crossings > 0 || sweep < 4) {
                continue;
            }
            let downward = sweep % 2 == 0;
            let ranks: Vec<usize> = if downward {
                (1..layers.len()).collect()
            } else {
                (0..layers.len().saturating_sub(1)).rev().collect()
            };
            keys = self.cluster_keys(&layers);
            for rank in ranks {
                // Every key is a 0…1 position, measured in the layer it comes from, so
                // vertices and whole clusters compare on the same scale.
                let fixed = if downward { &layers[rank - 1] } else { &layers[rank + 1] };
                let fixed_width = (fixed.len().max(2) - 1) as f64;
                let own_width = (layers[rank].len().max(2) - 1) as f64;
                for (index, &vertex) in fixed.iter().enumerate() {
                    position[vertex] = index as f64 / fixed_width;
                }
                for (index, &vertex) in layers[rank].iter().enumerate() {
                    let neighbours = if downward { &self.up[vertex] } else { &self.down[vertex] };
                    let mut sum = 0.0;
                    let mut count = 0usize;
                    for &n in neighbours {
                        if !position[n].is_nan() {
                            sum += position[n];
                            count += 1;
                        }
                    }
                    // Vertices without neighbours on that side keep their place.
                    fraction[vertex] =
                        if count == 0 { index as f64 / own_width } else { sum / count as f64 };
                }
                for &vertex in fixed {
                    position[vertex] = f64::NAN;
                }
                layers[rank] = self.arrange(&layers[rank], &fraction, &keys);
            }
            self.transpose(&mut layers);
            let crossings = self.total_crossings(&layers);
            if crossings < best_crossings || (crossings == best_crossings && sweep < 4) {
                best_crossings = crossings;
                best = layers.clone();
            }
        }
        best
    }

    fn chain(&self, cluster: Option<usize>) -> &[usize] {
        cluster.and_then(|c| self.chains.get(c)).map_or(&[], |chain| chain.as_slice())
    }

    /// Where each cluster sits on average over all its layers, as a fraction of layer width:
    /// one key per cluster, so siblings sort the same way everywhere.
    fn cluster_keys(&self, layers: &[Vec<usize>]) -> Vec<Option<f64>> {
        let mut sums = vec![(0.0, 0.0); self.chains.len()];
        for layer in layers {
            let width = (layer.len().max(2) - 1) as f64;
            for (index, &vertex) in layer.iter().enumerate() {
                if self.vertices[vertex].border.is_some() {
                    continue;
                }
                for &cluster in self.chain(self.vertices[vertex].cluster) {
                    sums[cluster].0 += index as f64 / width;
                    sums[cluster].1 += 1.0;
                }
            }
        }
        sums.into_iter().map(|(total, count)| (count > 0.0).then(|| total / count)).collect()
    }

    /// Sorts a layer by `fraction`, cluster by cluster: a cluster's members stay contiguous,
    /// flanked by its borders, and the cluster as a whole sorts by its global key.
    fn arrange(&self, layer: &[usize], fraction: &[f64], keys: &[Option<f64>]) -> Vec<usize> {
        self.items(None, layer, fraction, keys)
    }

    fn items(
        &self,
        parent: Option<usize>,
        members: &[usize],
        fraction: &[f64],
        keys: &[Option<f64>],
    ) -> Vec<usize> {
        let mut entries: Vec<(f64, Vec<usize>)> = Vec::new();
        // Clusters in the order they first appear, for a stable result.
        let mut children: Vec<(usize, Vec<usize>)> = Vec::new();
        let mut left_border = None;
        let mut right_border = None;
        let depth = parent.map_or(0, |p| self.chains[p].len());
        for &vertex in members {
            let v = &self.vertices[vertex];
            if let Some(side) = v.border
                && v.cluster == parent
            {
                if side == Border::Left {
                    left_border = Some(vertex);
                } else {
                    right_border = Some(vertex);
                }
                continue;
            }
            let owners = self.chain(v.cluster);
            if owners.len() > depth {
                let owner = owners[depth];
                match children.iter_mut().find(|(cluster, _)| *cluster == owner) {
                    Some((_, list)) => list.push(vertex),
                    None => children.push((owner, vec![vertex])),
                }
            } else {
                entries.push((fraction[vertex], vec![vertex]));
            }
        }
        for (cluster, list) in children {
            // Where the cluster sits overall, the same in every layer.
            let local = list.iter().map(|&v| fraction[v]).sum::<f64>() / list.len() as f64;
            let key = keys.get(cluster).copied().flatten().unwrap_or(local);
            entries.push((key, self.items(Some(cluster), &list, fraction, keys)));
        }
        // Stable, so ties keep their order.
        entries.sort_by(|a, b| a.0.total_cmp(&b.0));
        let mut result = Vec::with_capacity(members.len());
        result.extend(left_border);
        for (_, vertices) in entries {
            result.extend(vertices);
        }
        result.extend(right_border);
        result
    }

    /// Swaps neighbours in the same cluster wherever that removes crossings. Only the two
    /// vertices' own edges change when they swap, so each decision looks at just those.
    fn transpose(&self, layers: &mut [Vec<usize>]) {
        let mut positions = vec![0usize; self.vertices.len()];
        for layer in layers.iter() {
            for (index, &vertex) in layer.iter().enumerate() {
                positions[vertex] = index;
            }
        }
        let pair_crossings = |positions: &[usize], u: usize, v: usize, neighbours: &[Vec<usize>]| {
            let mut count = 0;
            for &a in &neighbours[u] {
                for &b in &neighbours[v] {
                    if positions[b] < positions[a] {
                        count += 1;
                    }
                }
            }
            count
        };
        let mut improved = true;
        let mut passes = 0;
        while improved && passes < 4 {
            improved = false;
            passes += 1;
            for layer in layers.iter_mut() {
                if layer.len() < 2 {
                    continue;
                }
                for index in 0..layer.len() - 1 {
                    let u = layer[index];
                    let v = layer[index + 1];
                    let (vu, vv) = (&self.vertices[u], &self.vertices[v]);
                    if vu.border.is_some() || vv.border.is_some() || vu.cluster != vv.cluster {
                        continue;
                    }
                    let now = pair_crossings(&positions, u, v, self.up)
                        + pair_crossings(&positions, u, v, self.down);
                    let swapped = pair_crossings(&positions, v, u, self.up)
                        + pair_crossings(&positions, v, u, self.down);
                    if swapped < now {
                        layer.swap(index, index + 1);
                        positions[u] = index + 1;
                        positions[v] = index;
                        improved = true;
                    }
                }
            }
        }
    }

    fn total_crossings(&self, layers: &[Vec<usize>]) -> usize {
        layers.windows(2).map(|pair| crossings(&pair[0], &pair[1], self.walls)).sum()
    }
}

/// Edge crossings between two adjacent layers: inversions among the lower endpoints, counted
/// with a Fenwick tree in O(E log V).
pub(crate) fn crossings(upper: &[usize], lower: &[usize], down: &[Vec<usize>]) -> usize {
    if lower.len() < 2 {
        return 0;
    }
    let position: HashMap<usize, usize> =
        lower.iter().enumerate().map(|(index, &vertex)| (vertex, index)).collect();
    let mut tree = vec![0usize; lower.len() + 1];
    let mut seen = 0;
    let mut count = 0;
    let mut ends: Vec<usize> = Vec::new();
    for &vertex in upper {
        ends.clear();
        ends.extend(down[vertex].iter().filter_map(|v| position.get(v).copied()));
        ends.sort_unstable();
        // Edges from the same vertex never cross each other: count, then insert.
        for &end in &ends {
            let mut index = end + 1;
            let mut at_most = 0;
            while index > 0 {
                at_most += tree[index];
                index &= index - 1;
            }
            count += seen - at_most;
        }
        for &end in &ends {
            let mut index = end + 1;
            while index <= lower.len() {
                tree[index] += 1;
                index += index.isolate_lowest_one();
            }
            seen += 1;
        }
    }
    count
}

// Geometry helpers

/// Weighted isotonic regression (pool adjacent violators): the non-decreasing sequence closest
/// to `targets`.
pub(crate) fn isotonic(targets: &[f64], weights: &[f64]) -> Vec<f64> {
    // (value, weight, count)
    let mut blocks: Vec<(f64, f64, usize)> = Vec::with_capacity(targets.len());
    for (&target, &weight) in targets.iter().zip(weights) {
        blocks.push((target, weight, 1));
        while blocks.len() > 1 && blocks[blocks.len() - 2].0 > blocks[blocks.len() - 1].0 {
            let (Some(last), Some(previous)) = (blocks.pop(), blocks.pop()) else { break };
            let total = previous.1 + last.1;
            blocks.push(((previous.0 * previous.1 + last.0 * last.1) / total, total, previous.2 + last.2));
        }
    }
    blocks.into_iter().flat_map(|(value, _, count)| std::iter::repeat_n(value, count)).collect()
}

fn median(sorted: &[f64]) -> f64 {
    let middle = sorted.len() / 2;
    if sorted.len() % 2 == 1 { sorted[middle] } else { (sorted[middle - 1] + sorted[middle]) / 2.0 }
}

/// Where the line from a node's center toward `target` leaves its outline.
pub fn clip(frame: Rect, boundary: Boundary, target: Point) -> Point {
    let center = frame.center();
    let dx = target.x - center.x;
    let dy = target.y - center.y;
    if dx == 0.0 && dy == 0.0 {
        return center;
    }
    let a = frame.width / 2.0;
    let b = frame.height / 2.0;
    let t = match boundary {
        Boundary::Rectangle => {
            let tx = if dx == 0.0 { f64::INFINITY } else { a / dx.abs() };
            let ty = if dy == 0.0 { f64::INFINITY } else { b / dy.abs() };
            tx.min(ty)
        }
        Boundary::Ellipse => 1.0 / ((dx / a) * (dx / a) + (dy / b) * (dy / b)).sqrt(),
        Boundary::Diamond => 1.0 / (dx.abs() / a + dy.abs() / b),
    };
    let t = if t.is_nan() { 0.0 } else { t.min(1.0) };
    Point::new(center.x + dx * t, center.y + dy * t)
}

fn self_loop(frame: Rect, horizontal: bool, label_size: Option<Size>) -> Route {
    // Out one side and back in, away from the flow direction.
    if horizontal {
        let top = frame.min_y();
        let left = frame.mid_x() - frame.width / 4.0;
        let right = frame.mid_x() + frame.width / 4.0;
        let points = vec![
            Point::new(left, top),
            Point::new(left, top - 22.0),
            Point::new(right, top - 22.0),
            Point::new(right, top),
        ];
        let label = label_size.map(|size| Point::new(frame.mid_x(), top - 22.0 - size.height / 2.0 - 2.0));
        return Route { points, label_center: label };
    }
    let right = frame.max_x();
    let upper = frame.mid_y() - frame.height / 4.0;
    let lower = frame.mid_y() + frame.height / 4.0;
    let points = vec![
        Point::new(right, upper),
        Point::new(right + 24.0, upper),
        Point::new(right + 24.0, lower),
        Point::new(right, lower),
    ];
    let label = label_size.map(|size| Point::new(right + 28.0 + size.width / 2.0, frame.mid_y()));
    Route { points, label_center: label }
}
