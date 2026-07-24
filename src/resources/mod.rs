//! Read-only resource projections for `lto get` / `lto describe`.
//!
//! These modules never write run state. They project untyped JSON into
//! stable, partial views for agent-friendly queries.

pub mod task;
