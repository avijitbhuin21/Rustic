//! Host-independent command bodies shared by the desktop (`src-tauri`) and
//! server (`rustic-server`) hosts. Hosts keep only argument parsing,
//! transport glue and event emission.

pub mod agent_audio;
pub mod agent_history;
pub mod editor;
pub mod formatters;
pub mod library;
pub mod models;
pub mod openrouter;
pub mod rules;
pub mod settings;
pub mod skills;
pub mod stream_coalesce;
pub mod tasks;
pub mod workflows;
