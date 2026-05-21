//! 内建命令实现集合 + PATH 可执行文件查找。
//!
//! 所有内建 runner 共享签名风格：`sink: &mut dyn Write` 写正常输出 +
//! `err_sink: &mut dyn Write` 写错误，由上层 REPL 根据重定向语义在调用前打开。
//!
//! 按「一 builtin 一文件」原则拆分，对外通过 `pub use` 重导出保持
//! `crate::builtins::{run_echo, Job, ...}` 等访问路径稳定。
//!
//! 详细的模块清单与公开 API 见 [docs/MODULES.md#builtins](../../docs/MODULES.md#builtins)。

pub mod cd;
pub mod complete;
pub mod declare;
pub mod echo;
pub mod history;
pub mod jobs;
pub mod path;
pub mod pwd;
pub mod type_cmd;

/// shell 内建命令清单，作为 `type` 命令查询的单一数据源。
/// 后续阶段新增内建（如 pwd/cd）时只需在此处追加。
pub const BUILTINS: &[&str] = &[
    "echo", "exit", "type", "pwd", "cd", "complete", "jobs", "history", "declare",
];

// ---- 公开 API 重导出，保持 `crate::builtins::xxx` 路径稳定 ----
pub use cd::run_cd;
pub use complete::run_complete;
pub use declare::run_declare;
pub use echo::run_echo;
pub use history::run_history;
pub use jobs::{
    advance_job_status, allocate_job_id, render_done_jobs, retain_running_jobs, run_jobs, Job,
    JobStatus,
};
pub use path::{find_in_path, list_path_executables};
pub use pwd::run_pwd;
pub use type_cmd::run_type;
