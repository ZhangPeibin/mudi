//! SessionMesh — local, read-only observability for Claude Code and Codex CLI sessions.

pub mod alerts;
pub mod app;
pub mod application;
pub mod attention;
pub mod clipboard;
pub mod collector;
pub mod config;
pub mod domain;
pub mod handoff;
pub mod handoff_artifact;
pub mod handoff_skill;
pub mod index;
pub mod managed_sessions;
pub mod models;
pub mod project;
pub mod providers;
pub mod read_model;
pub mod refresh;
pub mod session_labels;
pub mod signals;
pub mod skills;
pub mod successor;
pub mod tasks;
pub mod ui;
