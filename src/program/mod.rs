//! Whole-program semantic graph built from a parsed `AppSetSnapshot`
//! (charter §3). Plan 1B.1 = nodes + app-qualified identity + topology index.

pub mod abi_ingest;
pub mod attributes;
pub mod body;
pub mod build;
pub mod dep_cache;
pub mod dep_summary;
pub mod graph;
pub mod graphify_export;
pub mod integration_report;
pub mod model;
pub mod node;
pub mod node_extract;
pub mod node_set;
pub mod pack;
pub mod physical;
pub mod profile;
pub mod resolve;
pub mod sig_fp;
pub mod topology;

pub use build::{
    DepLayer, assemble_program_graph, build_dep_layer, build_program_graph,
    build_program_graph_from_parsed,
};
pub use graph::{ObjectIndex, ProgramGraph};
pub use node::{AppRef, AppRegistry, ObjKey, ObjectNodeId, RoutineNodeId};
pub use node_extract::{Access, ObjectNode, RoutineNode, extract_nodes};
pub use sig_fp::source_routine_node_id;
pub use topology::DependencyGraph;
