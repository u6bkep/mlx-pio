# Tickets

Local stand-in for an issue tracker until we have a remote. One file per ticket,
`NNN-slug.md`. Status lives in the frontmatter line at the top.

Status values: `open` · `in-progress` · `blocked` · `done` · `wontfix`

| # | Title | Status | Priority |
|---|---|---|---|
| 001 | Parallel tempering with live solution exchange | in-progress | high |
| 002 | Meta-optimize search hyperparameters | open | medium |
| 003 | Hash-based tabu list | open | low |

Provenance: 001–003 distilled from a survey of `reference/mlx86` (a friend's
x86-assembly superoptimizer) on 2026-06-22. The split of what transfers vs. what
doesn't is recorded in each ticket.
