//! 命令执行：单命令外部进程（`external`）+ N 段管线（`pipeline`）。
//!
//! ## 模块组织
//!
//! - [`external`] — `run_external`：单命令外部进程的 spawn / wait / 后台分支
//! - [`pipeline`] — `run_pipeline` + `PrevOutput`：N 段命令以 `|` 串联的执行
//!
//! 两者共享：
//! - `crate::builtins::path::{find_in_path}` 做 PATH 解析
//! - `crate::builtins::jobs::{Job, JobStatus, allocate_job_id}` 维护后台作业表
//! - `crate::redirect::open_file_for_redirect` 统一文件打开模式
//!
//! 设计细节详见 docs/DESIGN_DECISIONS.md#pipeline-prev-output。

pub mod external;
pub mod pipeline;

pub use external::run_external;
pub use pipeline::run_pipeline;
