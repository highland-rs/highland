# ADR-0001: Record architecture decisions

- Status: accepted
- Date: 2026-09-27

## Context

Highland makes decisions that are expensive to reverse: the async runtime, the
Netlink library, the control-socket transport, the state-machine shape. Without
a record, those decisions live in commit messages and in someone's memory.

## Decision

Architecture decision records live in `docs/adr/`, one file per decision, named
`ADR-NNNN-title.md`. A record has a status (`proposed`, `accepted`, `superseded
by ADR-NNNN`), a date, the context, the decision, and the consequences.

A decision is recorded before the work that depends on it starts. `SPEC.md`
Appendix B lists the decisions that are still open; each one moves out of that
list when its ADR is accepted.

## Consequences

- A reviewer can see why a dependency exists.
- A superseded decision stays readable, so the history of the design is not
  lost.
- Recording a small decision is cheap, so there is no excuse for not recording
  one.
