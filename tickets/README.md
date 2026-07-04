# Tickets

Local stand-in for an issue tracker until we have a remote. One file per ticket,
`NNN-slug.md`. Status lives in the frontmatter line at the top.

Status values: `open` · `in-progress` · `blocked` · `done` · `wontfix`

| # | Title | Status | Priority |
|---|---|---|---|
| 001 | Parallel tempering with live solution exchange | done (superseded) | — |
| 002 | Meta-optimize search hyperparameters | in-progress (tuning transfers) | high |
| 003 | Hash-based tabu list | open | low |
| 004 | Eval hot-path performance | in-progress (3.4x) | high |
| 005 | Spec-level oracle (tolerance-band metric + independent certifier) | open (design) | high |

Provenance: 001–003 distilled from a survey of `reference/mlx86` (a friend's
x86-assembly superoptimizer) on 2026-06-22. The split of what transfers vs. what
doesn't is recorded in each ticket. 004 opened 2026-06-22 — eval speed became
the shared bottleneck for both scale and trustworthy meta-tuning. 005 opened
2026-07-04 — the cycle-exact reference oracle is the #1 human-bias source
(design doc; code blocked on the open questions at the bottom of the ticket).
