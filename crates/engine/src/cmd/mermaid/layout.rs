//! A small layered graph layout: cycle removal by depth-first search, longest-path ranking,
//! barycentre ordering with subgraph members kept together, fixed spacing.
//! shortcut: no dummy nodes for edges that span several ranks, so such an edge can cross a node
//! in between; upgrade to a Sugiyama crate if that shows up in real diagrams.

const RANK_GAP: f64 = 90.0;
const NODE_GAP: f64 = 44.0;
const GROUP_GAP: f64 = 80.0;
const SWEEPS: usize = 4;

/// Top-left corners for nodes of `sizes` (w, h) joined by `edges`. `groups[i]` is the chain of
/// subgraphs node i is in, outermost first. `vertical`: ranks run top to bottom (else left to
/// right); `reversed`: the other way round.
pub fn layout(sizes: &[(f64, f64)], edges: &[(usize, usize)], groups: &[Vec<usize>], vertical: bool, reversed: bool) -> Vec<(f64, f64)> {
    let n = sizes.len();
    let main_of = |s: &(f64, f64)| if vertical { s.1 } else { s.0 };
    let cross_of = |s: &(f64, f64)| if vertical { s.0 } else { s.1 };
    let dag = acyclic(n, edges);
    let rank = ranks(n, &dag);
    let depth = rank.iter().copied().max().map_or(0, |m| m + 1);
    let mut layers: Vec<Vec<usize>> = vec![Vec::new(); depth];
    for (i, r) in rank.iter().enumerate() {
        layers[*r].push(i);
    }
    for l in &mut layers {
        *l = contiguous(l, groups, 0);
    }
    let mut pos = vec![0.0f64; n];
    center(&layers, &mut pos);
    let (mut up, mut down) = (vec![vec![]; n], vec![vec![]; n]);
    for &(a, b) in &dag {
        down[a].push(b);
        up[b].push(a);
    }
    for sweep in 0..SWEEPS * 2 {
        let towards = if sweep % 2 == 0 { &up } else { &down };
        let order: Vec<usize> = if sweep % 2 == 0 { (0..depth).collect() } else { (0..depth).rev().collect() };
        for r in order {
            let Some(layer) = layers.get(r) else { continue };
            let mut keyed: Vec<(f64, usize)> = layer
                .iter()
                .map(|&v| {
                    let ns = &towards[v];
                    let k = if ns.is_empty() { pos[v] } else { ns.iter().map(|&u| pos[u]).sum::<f64>() / ns.len() as f64 };
                    (k, v)
                })
                .collect();
            keyed.sort_by(|a, b| a.0.total_cmp(&b.0));
            let sorted: Vec<usize> = keyed.into_iter().map(|(_, v)| v).collect();
            layers[r] = contiguous(&sorted, groups, 0);
            center(&layers[r..=r], &mut pos);
        }
    }
    // Coordinates.
    let mut main_off = Vec::with_capacity(depth);
    let mut main_ext = Vec::with_capacity(depth);
    let mut at = 0.0;
    for l in &layers {
        let ext = l.iter().filter_map(|&v| sizes.get(v)).map(main_of).fold(0.0, f64::max);
        main_off.push(at);
        main_ext.push(ext);
        at += ext + RANK_GAP;
    }
    let total_main = (at - RANK_GAP).max(0.0);
    let widths: Vec<f64> = layers
        .iter()
        .map(|l| {
            let mut w = 0.0;
            for (k, &v) in l.iter().enumerate() {
                w += sizes.get(v).map_or(0.0, cross_of);
                if let Some(&next) = l.get(k + 1) {
                    w += gap(groups, v, next);
                }
            }
            w
        })
        .collect();
    let widest = widths.iter().copied().fold(0.0, f64::max);
    let mut out = vec![(0.0, 0.0); n];
    for (r, l) in layers.iter().enumerate() {
        let mut c = (widest - widths.get(r).copied().unwrap_or(0.0)) / 2.0;
        for (k, &v) in l.iter().enumerate() {
            let s = sizes.get(v).copied().unwrap_or((0.0, 0.0));
            let ext = main_of(&s);
            let mut m = main_off[r] + (main_ext[r] - ext) / 2.0;
            if reversed {
                m = total_main - m - ext;
            }
            out[v] = if vertical { (c, m) } else { (m, c) };
            c += cross_of(&s);
            if let Some(&next) = l.get(k + 1) {
                c += gap(groups, v, next);
            }
        }
    }
    out
}

/// Space between neighbours: wider when they are in different subgraphs.
fn gap(groups: &[Vec<usize>], a: usize, b: usize) -> f64 {
    if groups.get(a) == groups.get(b) { NODE_GAP } else { GROUP_GAP }
}

/// Each layer's nodes on a centred line, positions in `pos` (index minus the middle).
fn center(layers: &[Vec<usize>], pos: &mut [f64]) {
    for l in layers {
        let mid = (l.len() as f64 - 1.0) / 2.0;
        for (k, &v) in l.iter().enumerate() {
            pos[v] = k as f64 - mid;
        }
    }
}

/// `nodes` reordered so members of one subgraph (at each nesting depth) sit together, at the place
/// of the group's first member.
fn contiguous(nodes: &[usize], groups: &[Vec<usize>], depth: usize) -> Vec<usize> {
    let key = |v: usize| groups.get(v).and_then(|g| g.get(depth)).copied();
    let mut out = Vec::with_capacity(nodes.len());
    let mut done = vec![false; nodes.len()];
    for i in 0..nodes.len() {
        if done[i] {
            continue;
        }
        let Some(k) = key(nodes[i]) else {
            done[i] = true;
            out.push(nodes[i]);
            continue;
        };
        let members: Vec<usize> = (i..nodes.len())
            .filter(|&j| {
                !done[j] && key(nodes[j]) == Some(k) && groups.get(nodes[j]).map(|g| &g[..=depth]) == groups.get(nodes[i]).map(|g| &g[..=depth])
            })
            .collect();
        let vs: Vec<usize> = members.iter().map(|&j| nodes[j]).collect();
        for j in members {
            done[j] = true;
        }
        out.extend(contiguous(&vs, groups, depth + 1));
    }
    out
}

/// The edges without self loops, with the ones closing a cycle turned round.
fn acyclic(n: usize, edges: &[(usize, usize)]) -> Vec<(usize, usize)> {
    let mut adj = vec![Vec::new(); n];
    for &(a, b) in edges {
        if a != b && a < n && b < n {
            adj[a].push(b);
        }
    }
    let mut state = vec![0u8; n];
    let mut dag = Vec::new();
    for s in 0..n {
        if state[s] != 0 {
            continue;
        }
        // Iterative depth-first search: (node, next child index).
        let mut stack = vec![(s, 0usize)];
        state[s] = 1;
        while let Some((v, k)) = stack.pop() {
            if let Some(&w) = adj[v].get(k) {
                stack.push((v, k + 1));
                match state[w] {
                    0 => {
                        state[w] = 1;
                        dag.push((v, w));
                        stack.push((w, 0));
                    }
                    1 => dag.push((w, v)),
                    _ => dag.push((v, w)),
                }
            } else {
                state[v] = 2;
            }
        }
    }
    dag
}

/// Longest path from the sources.
fn ranks(n: usize, dag: &[(usize, usize)]) -> Vec<usize> {
    let mut indeg = vec![0usize; n];
    let mut out = vec![Vec::new(); n];
    for &(a, b) in dag {
        indeg[b] += 1;
        out[a].push(b);
    }
    let mut rank = vec![0usize; n];
    let mut queue: Vec<usize> = (0..n).filter(|&v| indeg[v] == 0).collect();
    let mut head = 0;
    while let Some(&v) = queue.get(head) {
        head += 1;
        for &w in &out[v] {
            rank[w] = rank[w].max(rank[v] + 1);
            indeg[w] -= 1;
            if indeg[w] == 0 {
                queue.push(w);
            }
        }
    }
    rank
}
