//! Knuth–Plass line breaking (Knuth & Plass, "Breaking Paragraphs into
//! Lines", 1981) — a faithful, deterministic implementation mirroring the JS
//! `src/layout/kp.ts` operation for operation.
//!
//! Items are boxes, glue, and penalties. The dynamic program finds the
//! minimum-demerit sequence of feasible breakpoints; demerits are
//! `(line_penalty + badness + penalty)²`, badness is `100·|ρ|³` where `ρ`
//! is the adjustment ratio, and lines carry fitness classes (0 tight,
//! 1 decent, 2 loose, 3 very loose). The paper's twice-around fitness pass
//! is implemented: if the optimal solution has adjacent lines whose fitness
//! classes differ by more than one (0↔3), a second pass forbids those
//! transitions and its result is used.
//!
//! The caller must end the item list with a feasible breakpoint (a glue or a
//! forced-break penalty) so the final line always terminates.
//!
//! Determinism: ties (equal demerits) resolve to the first active node in
//! document order; no randomness anywhere.

/// A line-breaking item.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum KpItem {
    /// An unbreakable box of the given width.
    Box {
        /// The box's width.
        width: f64,
    },
    /// A break opportunity with stretch and shrink.
    Glue {
        /// The glue's natural width.
        width: f64,
        /// The stretchability.
        stretch: f64,
        /// The shrinkability.
        shrink: f64,
    },
    /// A break opportunity with a penalty (`-inf` = forced, `+inf` =
    /// forbidden).
    Penalty {
        /// The penalty's width.
        width: f64,
        /// The penalty value.
        penalty: f64,
    },
}

/// One chosen line.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct KpLine {
    /// Index of the first item of the line (inclusive).
    pub start: usize,
    /// Index of the last item of the line (inclusive).
    pub end: usize,
    /// The adjustment ratio ρ (0 = perfectly set).
    pub ratio: f64,
    /// The line's badness.
    pub badness: f64,
    /// The fitness class (0 tight … 3 very loose).
    pub fitness: u8,
    /// Accumulated demerits up to and including this line.
    pub demerits: f64,
}

/// A node in the active list.
#[derive(Debug, Clone, Copy)]
struct ActiveNode {
    line: usize,
    /// The breakpoint index where this node was created (0 for the start).
    breakpoint_index: usize,
    sum_w: f64,
    sum_s: f64,
    sum_shrink: f64,
    sum_demerits: f64,
    fitness: u8,
}

/// The chosen line ending at a breakpoint.
#[derive(Debug, Clone, Copy)]
struct ChosenLine {
    start_item: usize,
    end_item: usize,
    ratio: f64,
    badness: f64,
    fitness: u8,
    demerits: f64,
    prev_break: usize,
}

const INF: f64 = f64::INFINITY;

fn fitness_of(ratio: f64) -> u8 {
    if ratio < -0.5 {
        0
    } else if ratio < 0.5 {
        1
    } else if ratio < 1.0 {
        2
    } else {
        3
    }
}

/// One pass of the dynamic program with the given fitness-transition
/// penalty; returns the chosen lines and whether any adjacent line pair has
/// a fitness gap greater than one.
///
/// The `items[j]` / prefix-sum accesses are provably in bounds: `j` runs
/// `1..n`, `k` runs `0..n`, and the reconstruction follows `prev_break`
/// pointers that always decrease toward 0 (scoped allow).
#[allow(clippy::indexing_slicing)]
fn run(
    items: &[KpItem],
    line_width: f64,
    tolerance: f64,
    line_penalty: f64,
    fitness_penalty: f64,
) -> (Vec<KpLine>, bool) {
    let n = items.len();

    // Prefix sums of widths / stretch / shrink over items[0..k].
    let mut sum_w = vec![0.0_f64; n + 1];
    let mut sum_s = vec![0.0_f64; n + 1];
    let mut sum_h = vec![0.0_f64; n + 1];
    for (k, item) in items.iter().enumerate() {
        sum_w[k + 1] = sum_w[k] + item.width();
        match item {
            KpItem::Glue {
                stretch, shrink, ..
            } => {
                sum_s[k + 1] = sum_s[k] + stretch;
                sum_h[k + 1] = sum_h[k] + shrink;
            }
            _ => {
                sum_s[k + 1] = sum_s[k];
                sum_h[k + 1] = sum_h[k];
            }
        }
    }

    let is_breakpoint = |j: usize| -> bool {
        match items[j] {
            KpItem::Glue { .. } => true,
            KpItem::Penalty { penalty, .. } => penalty < INF,
            KpItem::Box { .. } => false,
        }
    };

    let mut active: Vec<ActiveNode> = vec![ActiveNode {
        line: 0,
        breakpoint_index: 0,
        sum_w: 0.0,
        sum_s: 0.0,
        sum_shrink: 0.0,
        sum_demerits: 0.0,
        fitness: 1,
    }];
    // chosen[j] = the best line ending at breakpoint j.
    let mut chosen: Vec<Option<ChosenLine>> = vec![None; n + 1];

    for j in 1..n {
        if !is_breakpoint(j) {
            continue;
        }
        let (penalty, forced) = match items[j] {
            KpItem::Penalty { penalty, .. } => (penalty, penalty == -INF),
            _ => (0.0, false),
        };

        let mut new_nodes: Vec<ActiveNode> = Vec::new();
        let mut remaining: Vec<ActiveNode> = Vec::new();
        for node in &active {
            let width = sum_w[j] - node.sum_w - items[j].width();
            let stretch = sum_s[j] - node.sum_s;
            let shrink = sum_h[j] - node.sum_shrink;
            let ratio: f64;
            if forced {
                // A forced break always breaks; an overflowing line pays an
                // emergency-stretch badness (TeX \emergencystretch), so paths
                // that avoid overflow are preferred while the paragraph still
                // ends.
                ratio = if width <= line_width + 1e-9 {
                    0.0
                } else {
                    (width - line_width) / 10.0
                };
            } else if width <= line_width + 1e-9 {
                ratio = if stretch > 0.0 {
                    (line_width - width) / stretch
                } else {
                    0.0
                };
            } else {
                if shrink <= 0.0 {
                    continue;
                }
                ratio = (line_width - width) / shrink;
            }
            if !forced && (ratio < -1.0 || ratio > tolerance) {
                continue;
            }

            let badness = if ratio == 0.0 {
                0.0
            } else {
                100.0 * ratio.abs().powi(3)
            };
            let effective_penalty = if forced { 0.0 } else { penalty };
            let demerits = (line_penalty + badness + effective_penalty).powi(2);
            let fitness = fitness_of(ratio);
            let fit_penalty = if fitness.abs_diff(node.fitness) > 1 {
                fitness_penalty
            } else {
                0.0
            };
            let total = node.sum_demerits + demerits + fit_penalty;

            match chosen[j] {
                None => {
                    chosen[j] = Some(ChosenLine {
                        start_item: if node.breakpoint_index == 0 {
                            0
                        } else {
                            node.breakpoint_index + 1
                        },
                        end_item: j,
                        ratio,
                        badness,
                        fitness,
                        demerits: total,
                        prev_break: node.breakpoint_index,
                    });
                }
                Some(current) => {
                    if total < current.demerits {
                        chosen[j] = Some(ChosenLine {
                            start_item: if node.breakpoint_index == 0 {
                                0
                            } else {
                                node.breakpoint_index + 1
                            },
                            end_item: j,
                            ratio,
                            badness,
                            fitness,
                            demerits: total,
                            prev_break: node.breakpoint_index,
                        });
                    }
                }
            }
            new_nodes.push(ActiveNode {
                line: node.line + 1,
                breakpoint_index: j,
                sum_w: sum_w[j],
                sum_s: sum_s[j],
                sum_shrink: sum_h[j],
                sum_demerits: total,
                fitness,
            });
            // The node stays active: its sequence can still extend further.
            remaining.push(*node);
        }
        // The paper keeps at most one active node per (breakpoint, fitness)
        // class: the future cost of a line starting at this breakpoint depends
        // only on its prefix sums and fitness class, so only the minimal-
        // demerits path to each class can ever win. Without this deduplication
        // every feasible node would spawn a copy at every later breakpoint and
        // the active list would double per breakpoint (exponential blowup).
        //
        // The JS keeps a `Map<fitness, node>`: insertion order is the order
        // new nodes were generated, a later node of the same fitness replaces
        // the earlier only when strictly better, and the surviving values
        // keep first-occurrence order. The order of the active list is part
        // of tie-breaking at later breakpoints (equal totals prefer the first
        // node in document order), so it must match exactly.
        let mut best: Vec<ActiveNode> = Vec::new();
        for node in new_nodes {
            match best.iter_mut().find(|n| n.fitness == node.fitness) {
                Some(existing) => {
                    if node.sum_demerits < existing.sum_demerits {
                        *existing = node;
                    }
                }
                None => best.push(node),
            }
        }
        remaining.extend(best);
        active = remaining;
    }

    // The last item is a feasible breakpoint (caller guarantees it).
    let final_break = n - 1;
    if chosen.get(final_break).and_then(Option::as_ref).is_none() {
        // Fallback (should not happen with a forced final break): one line.
        return (
            vec![KpLine {
                start: 0,
                end: final_break,
                ratio: 0.0,
                badness: 0.0,
                fitness: 1,
                demerits: 0.0,
            }],
            false,
        );
    }

    // Reconstruct backwards.
    let mut lines: Vec<KpLine> = Vec::new();
    let mut cursor = final_break;
    let mut bad_transitions = false;
    let mut prev_fitness: u8 = 1;
    while cursor > 0 {
        let Some(c) = chosen.get(cursor).and_then(Option::as_ref) else {
            break;
        };
        lines.push(KpLine {
            start: c.start_item.min(c.end_item),
            end: c.end_item,
            ratio: c.ratio,
            badness: c.badness,
            fitness: c.fitness,
            demerits: c.demerits,
        });
        if c.fitness.abs_diff(prev_fitness) > 1 {
            bad_transitions = true;
        }
        prev_fitness = c.fitness;
        cursor = c.prev_break;
    }
    lines.reverse();
    (lines, bad_transitions)
}

impl KpItem {
    /// The item's width.
    #[must_use]
    pub const fn width(&self) -> f64 {
        match self {
            KpItem::Box { width } | KpItem::Glue { width, .. } | KpItem::Penalty { width, .. } => {
                *width
            }
        }
    }
}

/// Compute the optimal line breaks for `items` given a target `line_width`.
/// `items` must end with a feasible breakpoint.
///
/// Mirrors the JS defaults: `tolerance = 100`, `line_penalty = 10`.
#[must_use]
pub fn line_break(
    items: &[KpItem],
    line_width: f64,
    tolerance: f64,
    line_penalty: f64,
) -> Vec<KpLine> {
    if items.is_empty() {
        return Vec::new();
    }
    let (first_lines, bad_transitions) = run(items, line_width, tolerance, line_penalty, 1.0);
    if !bad_transitions {
        return first_lines;
    }
    let (second_lines, _) = run(
        items,
        line_width,
        tolerance,
        line_penalty,
        3000.0_f64.powi(2),
    );
    second_lines
}
