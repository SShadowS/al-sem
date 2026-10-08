//! The old `engine::l3::…` paths of the detector model. The legacy L3 engine
//! (al-sem's `src/resolve/` port: its own parse, call resolver and event-graph
//! builder) was deleted in engine-switch S9.6; the model it fed lives in
//! `program::model` and is built from the program engine. These aliases keep the
//! old paths compiling; they and every such path are removed in S9.7.

pub use crate::program::model::calls as call_resolver;
pub use crate::program::model::coverage;
pub use crate::program::model::event_param_temp;
pub use crate::program::model::events as event_graph;
pub use crate::program::model::extension_fields;
pub use crate::program::model::program_calls;
pub use crate::program::model::record_types;
pub use crate::program::model::symbol_table;
pub use crate::program::model::taxonomy;
pub use crate::program::model::workspace as l3_workspace;
