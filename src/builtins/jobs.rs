//! 后台作业管理：`Job` / `JobStatus` 类型 + 状态推进 / 渲染 / 移除三函数 + `run_jobs` + `allocate_job_id`。
//!
//! 详见 docs/DESIGN_DECISIONS.md#background-reaping。

use std::io::{self, Write};
use std::process::Child;

/// 后台作业状态。
///
/// 本阶段（codecrafters「Manage Jobs」/ 完成态回收）：
/// - `Running`：进程仍存活，`Child::try_wait()` 返回 `Ok(None)`；
/// - `Done`：进程已正常退出，`Child::try_wait()` 返回 `Ok(Some(_))`，或
///   防御性地把 `Err(_)`（极罕见，如已被外部 reap 导致 ECHILD）也视为 Done。
///
/// `Done` 在 `run_jobs` 渲染一次后立即从作业表中 `retain` 移除，与 bash 行为一致。
/// `Stopped` / 信号终止等状态不在本阶段范围内，故意保留 enum 扩展空间。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JobStatus {
    Running,
    Done,
}

impl JobStatus {
    /// 状态短串，用于 `jobs` 行内打印（左对齐到 24 字符宽）。
    /// 显式 `&'static str` 映射，避免依赖 `Display` impl，便于后续阶段为不同状态
    /// 加入额外字段时不破坏格式契约。
    pub fn as_str(&self) -> &'static str {
        match self {
            JobStatus::Running => "Running",
            JobStatus::Done => "Done",
        }
    }
}

/// 单个后台作业的元信息。
///
/// 字段对应 bash「Tracking Background Jobs」四要素：
/// - `id`：作业编号（`[N]` 中的 N），由 [`allocate_job_id`] 分配「最小可用正整数」
/// - `pid`：单进程作业的 PID；pipeline 作业为**最后一段** PID（与 bash `$!` 一致）
/// - `command`：命令字符串（pipeline 各段 argv 用 `" | "` 拼接）
/// - `status`：当前状态（`Running` / `Done`）
/// - `children`：单进程 `vec![child]`；pipeline `vec![child1, ...]` 按管道顺序排列
///
/// 状态推进：遍历所有 child，任一仍 `Running` 即整个 Job 保持 Running；全部退出才转 Done。
/// Job 从 Vec 中 `retain` 移除时所有 Child 一同 drop——`try_wait` 已完成 reap，无僵尸残留。
/// 详见 [docs/DESIGN_DECISIONS.md#background-reaping](../../docs/DESIGN_DECISIONS.md#background-reaping)。
///
/// # Examples
///
/// ```ignore
/// use std::process::Command;
/// let child = Command::new("sleep").arg("30").spawn().unwrap();
/// let job = Job {
///     id: 1,
///     pid: child.id(),
///     command: "sleep 30".to_string(),
///     status: JobStatus::Running,
///     children: vec![child],
/// };
/// // 后续由主循环 prompt 前 reap 三步推进 status → Done 并从 Vec 中 retain 移除
/// ```
pub struct Job {
    pub id: u32,
    #[allow(dead_code)]
    pub pid: u32,
    pub command: String,
    pub status: JobStatus,
    pub children: Vec<Child>,
}

/// 非阻塞推进作业表中所有 `Running` 项的状态。
///
/// 对每个 Running 项遍历 `children` 调 `try_wait()`：任一仍 `Ok(None)` → 保持 Running；
/// 全部 `Ok(Some(_))` 或 `Err(_)` → 转 Done（`Err` 防御性视为 Done 覆盖 ECHILD）。
///
/// 三函数原子拆分中的第一步：仅推进状态，不写字节、不改 Vec 长度。
/// 渲染由 [`render_done_jobs`]、移除由 [`retain_running_jobs`] 承担。
/// 详见 [docs/DESIGN_DECISIONS.md#background-reaping](../../docs/DESIGN_DECISIONS.md#background-reaping)。
pub fn advance_job_status(jobs: &mut [Job]) {
    for job in jobs.iter_mut() {
        if job.status != JobStatus::Running {
            continue;
        }
        // 遍历该 Job 的所有 child；任一仍 Running 即保持 Running，
        // 全部退出（Ok(Some) 或 Err）才转 Done。
        let mut all_finished = true;
        for child in job.children.iter_mut() {
            match child.try_wait() {
                Ok(Some(_)) => {}                  // 已退出并 reap
                Ok(None) => all_finished = false,  // 仍存活
                Err(_) => {}                       // 防御性：ECHILD，视为已结束
            }
        }
        if all_finished {
            job.status = JobStatus::Done;
        }
    }
}

/// 仅渲染作业表中已 `Done` 项的标准 Done 行；不修改 Vec。
///
/// 行格式：`[<id><mark>  Done<20 空格><command>\n`（24 字符 status 字段，Done 不加尾 ` &`）。
/// marker 基于 Done+Running 全集索引计算（`last_idx → '+'`、`last_idx-1 → '-'`、其他 → ` `），
/// Running 项贡献索引基线但不写字节——复刻 bash 在 `sleep 5 &; sleep 100 &` 场景下
/// job 1 完成时显示 `[1]-  Done                    sleep 5` 的行为。
///
/// 调用方仅 REPL prompt 前自动 reap 路径（`run_jobs` 不调本函数以避免双写重复）。
pub fn render_done_jobs(sink: &mut dyn Write, jobs: &[Job]) -> io::Result<()> {
    if jobs.is_empty() {
        return Ok(());
    }
    let last_idx = jobs.len() - 1;
    for (idx, job) in jobs.iter().enumerate() {
        if job.status != JobStatus::Done {
            continue;
        }
        let mark = if idx == last_idx {
            '+'
        } else if idx + 1 == last_idx {
            '-'
        } else {
            ' '
        };
        writeln!(
            sink,
            "[{}]{}  {:<24}{}",
            job.id,
            mark,
            job.status.as_str(),
            job.command,
        )?;
    }
    Ok(())
}

/// 分配下一个后台作业编号：返回当前 `jobs` 表中**最小未被占用**的正整数。
///
/// 语义：表空 → 1；`[1, 2]` → 3；`[1, 3]` → 2（间隙优先复用）；`[2, 3]` → 1。
///
/// 纯函数（仅查询切片），算法是 `(1u32..).find(...)`，O(n²) 但 n ≤ 几十项可忽略。
/// 详见 [docs/DESIGN_DECISIONS.md#background-reaping](../../docs/DESIGN_DECISIONS.md#background-reaping)。
pub fn allocate_job_id(jobs: &[Job]) -> u32 {
    (1u32..)
        .find(|n| !jobs.iter().any(|j| j.id == *n))
        .expect("allocate_job_id: u32 range exhausted (unreachable)")
}

/// 一次性 retain 移除作业表中所有 `Done` 项。
///
/// `Child` 已被 [`advance_job_status`] 中的 `try_wait` 成功收尾，drop 即无僵尸残留。
/// 与渲染解耦——调用方需自行确保渲染（如有）已完成再调本函数；本阶段两条路径
/// （prompt 前自动 reap、`run_jobs`）都在渲染后才调用。
///
/// 空表场景下 `retain` 自然 no-op。
pub fn retain_running_jobs(jobs: &mut Vec<Job>) {
    jobs.retain(|j| j.status != JobStatus::Done);
}

/// `jobs` 内建：列出当前 shell 已知的后台作业。
///
/// 格式：`[{id}]{mark}  {status:<24}{cmd}{tail}\n`
/// - mark：`+` 最近、`-` 次新、` ` 其他
/// - status 字段总宽 24（`"Running"` + 17 空格 / `"Done"` + 20 空格）
/// - tail：Running 行追加 ` &`，Done 行不追加
///
/// 入口处再调一次 [`advance_job_status`] 兜底覆盖 prompt 间窗口；渲染后
/// [`retain_running_jobs`] 移除 Done。
///
/// 详见 [docs/DESIGN_DECISIONS.md#background-reaping](../../docs/DESIGN_DECISIONS.md#background-reaping)。
pub fn run_jobs(
    sink: &mut dyn Write,
    _err_sink: &mut dyn Write,
    _args: &[String],
    jobs: &mut Vec<Job>,
) -> io::Result<()> {
    // 入口处再做一次状态推进（兜底）：覆盖「prompt 前 reap → 立即 jobs」之间的
    // 边界窗口，与 REPL prompt 前自动 reap 路径共享同一原子函数，行为一致。
    advance_job_status(jobs);

    if jobs.is_empty() {
        return Ok(());
    }
    let last_idx = jobs.len() - 1;
    for (idx, job) in jobs.iter().enumerate() {
        // mark 计算：最近作业 `+`，次新 `-`，更早 ` ` 占位。
        // 渲染顺序使用「retain 之前」的索引，与 bash 在单作业完成场景下的输出一致。
        let mark = if idx == last_idx {
            '+'
        } else if idx + 1 == last_idx {
            '-'
        } else {
            ' '
        };
        // 一次性 writeln! 写整行——避免分多次 write 在 `>` 重定向下被其他写入
        // 穿插造成字节顺序问题。`{:<24}` 左对齐填充到 24 字符总宽。
        // Running 行追加 " &"（与 bash 行为一致），Done 行不追加。
        let tail = match job.status {
            JobStatus::Running => " &",
            JobStatus::Done => "",
        };
        writeln!(
            sink,
            "[{}]{}  {:<24}{}{}",
            job.id,
            mark,
            job.status.as_str(),
            job.command,
            tail,
        )?;
    }
    // 一次性移除所有 Done：Child 已 reap，drop 即无残留。
    // 共用 retain_running_jobs 与自动 reap 路径保持单一移除来源。
    retain_running_jobs(jobs);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---- Stage「Manage Jobs」：run_jobs 格式契约 + reap 行为用例 ----

    /// 构造一个仍在运行的 Job：spawn `sleep 30`，长存活足够覆盖测试窗口。
    /// 用例结束时由 RAII guard 兜底 `kill` + `wait`，避免子进程残留。
    fn spawn_running_job(id: u32, command: &str) -> Job {
        let child = std::process::Command::new("sleep")
            .arg("30")
            .spawn()
            .expect("spawn sleep 30");
        let pid = child.id();
        Job {
            id,
            pid,
            command: command.to_string(),
            status: JobStatus::Running,
            children: vec![child],
        }
    }

    /// 构造一个已退出但尚未 reap 的 Job：spawn `true`，主动 `wait()` 让进程退出。
    /// 注意：此处直接调 `wait` 让进程退出并 reap，但状态仍标记 Running——
    /// 这模拟 reap_finished_jobs 调用前的初始态。
    /// 由于 `wait` 会消费内部 ExitStatus，后续 `try_wait` 在已 reap 的子进程上
    /// 会返回 `Err(ECHILD)` —— 这正好命中「Err 视为 Done」防御性分支。
    /// 真实场景下走的是 `try_wait` 的 `Ok(Some(_))` 分支（进程退出但未被 wait），
    /// 此处用 `true` + `wait` 仅为单测可控性；reap 推进的语义等价。
    fn spawn_exited_job(id: u32, command: &str) -> Job {
        let mut child = std::process::Command::new("true")
            .spawn()
            .expect("spawn true");
        // 阻塞等子进程退出 + reap，确保 try_wait 之后必然返回 Err 或 Ok(Some)
        let _ = child.wait();
        let pid = child.id();
        Job {
            id,
            pid,
            command: command.to_string(),
            status: JobStatus::Running,
            children: vec![child],
        }
    }

    /// 杀掉 Running Job 的所有 child，避免测试遗留。Done Job 已被 reap，无需处理。
    fn kill_job(job: &mut Job) {
        for child in job.children.iter_mut() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }

    /// 跑 `run_jobs` 的薄封装：返回 (stdout, stderr) 字符串对，便于断言。
    fn invoke_jobs(jobs: &mut Vec<Job>) -> (String, String) {
        let mut sink: Vec<u8> = Vec::new();
        let mut err: Vec<u8> = Vec::new();
        run_jobs(&mut sink, &mut err, &[], jobs).expect("run_jobs");
        (
            String::from_utf8(sink).expect("utf8 stdout"),
            String::from_utf8(err).expect("utf8 stderr"),
        )
    }

    #[test]
    fn jobs_single_running_exact_format() {
        // 题面 tester 场景：单作业、Running、`+` 标记、24 宽 status 填充、尾 `&`
        let mut jobs = vec![spawn_running_job(1, "sleep 10")];
        let (out, err) = invoke_jobs(&mut jobs);
        // 完整逐字节匹配：`[1]+` + 2 空格 + "Running" + 17 空格 + "sleep 10 &\n"
        assert_eq!(out, "[1]+  Running                 sleep 10 &\n");
        assert!(err.is_empty(), "jobs 不向 stderr 写字节");
        // Running 不被 retain 移除
        assert_eq!(jobs.len(), 1);
        for j in &mut jobs {
            kill_job(j);
        }
    }

    #[test]
    fn jobs_status_field_padded_to_24_chars() {
        // 显式验证：mark 后的 2 空格分隔之后，status + 填充共 24 字符宽，紧接 cmd
        let mut jobs = vec![spawn_running_job(1, "x")];
        let (out, _) = invoke_jobs(&mut jobs);
        // 行格式：`[1]+  ` (6 字节) + status_field (24 字节) + "x &\n"
        let prefix = "[1]+  ";
        let suffix = "x &\n";
        assert!(out.starts_with(prefix));
        assert!(out.ends_with(suffix));
        let status_field = &out[prefix.len()..out.len() - suffix.len()];
        assert_eq!(status_field.len(), 24, "status 字段总宽必须为 24");
        assert!(status_field.starts_with("Running"));
        // 7 字符 "Running" + 17 空格 = 24
        assert_eq!(&status_field[7..], &" ".repeat(17));
        for j in &mut jobs {
            kill_job(j);
        }
    }

    #[test]
    fn jobs_empty_table_no_output() {
        // 空作业表：sink / err_sink 均零字节，Ok(())
        let mut jobs: Vec<Job> = Vec::new();
        let (out, err) = invoke_jobs(&mut jobs);
        assert!(out.is_empty(), "空表不写 stdout");
        assert!(err.is_empty(), "空表不写 stderr");
    }

    #[test]
    fn jobs_done_renders_without_trailing_amp_and_retained_out() {
        // 已退出作业：reap 推进至 Done，渲染一次（无尾 `&`、24 宽 Done），
        // 渲染后从 Vec 中移除——再调一次 invoke_jobs 即空输出。
        let mut jobs = vec![spawn_exited_job(1, "true")];
        let (out, err) = invoke_jobs(&mut jobs);
        // Done 行：`[1]+` + 2 空格 + "Done" + 20 空格 + "true\n"（无尾 `&`）
        assert_eq!(out, "[1]+  Done                    true\n");
        assert!(err.is_empty());
        // retain 后 Vec 为空
        assert!(jobs.is_empty(), "Done 项渲染后必须被一次性移除");
        // 再次调用：无任何输出
        let (out2, err2) = invoke_jobs(&mut jobs);
        assert!(out2.is_empty(), "二次 jobs 不再列出已 Done 项");
        assert!(err2.is_empty());
    }

    #[test]
    fn jobs_mixed_running_and_done_retain_only_running() {
        // 混合：一条 Running + 一条 Done。渲染两行，retain 后仅留 Running。
        // 注意构造顺序：先 Running 再 Done，则 last_idx 指向 Done（idx=1 → `+`），
        // Running（idx=0）为次新 → `-`。
        let mut jobs = vec![
            spawn_running_job(1, "sleep 10"),
            spawn_exited_job(2, "true"),
        ];
        let (out, _) = invoke_jobs(&mut jobs);
        let lines: Vec<&str> = out.lines().collect();
        assert_eq!(lines.len(), 2);
        // idx=0 Running → `-`、尾 ` &`
        assert_eq!(lines[0], "[1]-  Running                 sleep 10 &");
        // idx=1 Done → `+`、无尾 `&`
        assert_eq!(lines[1], "[2]+  Done                    true");
        // retain 后仅剩 Running
        assert_eq!(jobs.len(), 1);
        assert_eq!(jobs[0].id, 1);
        assert_eq!(jobs[0].status, JobStatus::Running);
        for j in &mut jobs {
            kill_job(j);
        }
    }

    // ---- Stage「Reaping Before Each Prompt」：三函数原子拆分契约用例 ----

    #[test]
    fn advance_job_status_only_promotes_running_to_done_no_len_change() {
        // 契约：advance_job_status 只对 Running 项调 try_wait 推进状态；
        // 不渲染、不修改 Vec 长度；Done 项保持 Done。
        let mut jobs = vec![
            spawn_running_job(1, "sleep 30"),  // 仍 Running
            spawn_exited_job(2, "true"),       // 已退出，进入函数前 status 仍是 Running
        ];
        let len_before = jobs.len();
        advance_job_status(&mut jobs);
        // len 不变
        assert_eq!(jobs.len(), len_before);
        // job 1 仍 Running（sleep 30 远未结束）
        assert_eq!(jobs[0].status, JobStatus::Running);
        // job 2 被推进为 Done（spawn_exited_job 已 wait 过，try_wait Err 命中 Done 分支）
        assert_eq!(jobs[1].status, JobStatus::Done);
        // 清理 Running
        for j in &mut jobs {
            kill_job(j);
        }
    }

    #[test]
    fn render_done_jobs_marker_uses_union_view() {
        // 契约：render_done_jobs 的 marker 基于「Done + Running 全集」索引计算，
        // 仅对 Done 项 writeln，Running 项贡献索引基线但不输出。
        //
        // 题面示例 1 关键场景：jobs = [Done(id=1, "sleep 5"), Running(id=2, "sleep 100")]
        // last_idx = 1 → Running；idx=0 (Done) 是 last-1 → mark 为 `-`。
        // 期望仅渲染 1 行（Done 的）：`[1]-  Done                    sleep 5\n`
        let mut jobs = vec![
            spawn_exited_job(1, "sleep 5"),
            spawn_running_job(2, "sleep 100"),
        ];
        // 必须先推进，否则 spawn_exited_job 的 status 还是 Running
        advance_job_status(&mut jobs);
        assert_eq!(jobs[0].status, JobStatus::Done);
        assert_eq!(jobs[1].status, JobStatus::Running);

        let mut sink: Vec<u8> = Vec::new();
        render_done_jobs(&mut sink, &jobs).expect("render_done_jobs");
        let out = String::from_utf8(sink).expect("utf8");
        // 仅 1 行 Done，marker 是 `-`（不是 `+`），无尾 ` &`
        assert_eq!(out, "[1]-  Done                    sleep 5\n");
        // 不修改 Vec 长度
        assert_eq!(jobs.len(), 2);
        for j in &mut jobs {
            kill_job(j);
        }
    }

    #[test]
    fn render_done_jobs_empty_or_all_running_no_output() {
        // 边界：空表 / 全 Running 时 render_done_jobs 不写任何字节。

        // 空表
        let jobs_empty: Vec<Job> = Vec::new();
        let mut sink: Vec<u8> = Vec::new();
        render_done_jobs(&mut sink, &jobs_empty).expect("empty ok");
        assert!(sink.is_empty(), "空表不写任何字节");

        // 全 Running
        let mut jobs = vec![
            spawn_running_job(1, "sleep 30"),
            spawn_running_job(2, "sleep 30"),
        ];
        let mut sink: Vec<u8> = Vec::new();
        render_done_jobs(&mut sink, &jobs).expect("all running ok");
        assert!(sink.is_empty(), "全 Running 不写任何字节");
        for j in &mut jobs {
            kill_job(j);
        }
    }

    #[test]
    fn retain_running_jobs_removes_all_done_in_one_pass() {
        // 契约：retain_running_jobs 一次性删除所有 Done 项；保留所有非 Done。
        let mut jobs = vec![
            spawn_exited_job(1, "true"),
            spawn_running_job(2, "sleep 30"),
            spawn_exited_job(3, "true"),
        ];
        // 先推进，让 1/3 进入 Done
        advance_job_status(&mut jobs);
        assert_eq!(jobs[0].status, JobStatus::Done);
        assert_eq!(jobs[1].status, JobStatus::Running);
        assert_eq!(jobs[2].status, JobStatus::Done);

        retain_running_jobs(&mut jobs);
        // 仅 id=2 仍在
        assert_eq!(jobs.len(), 1);
        assert_eq!(jobs[0].id, 2);
        assert_eq!(jobs[0].status, JobStatus::Running);

        // 再次调用：no-op
        retain_running_jobs(&mut jobs);
        assert_eq!(jobs.len(), 1);

        for j in &mut jobs {
            kill_job(j);
        }
    }

    // ---- Stage「Recycling Job Numbers」：allocate_job_id 最小可用分配契约 ----

    #[test]
    fn allocate_empty_table_returns_one() {
        // 表空 → 1：`(1u32..).find(...)` 首次循环 n=1，空表上 any() 恒为 false，
        // 闭包返回 true，命中。
        let jobs: Vec<Job> = Vec::new();
        assert_eq!(allocate_job_id(&jobs), 1);
    }

    #[test]
    fn allocate_sequential_after_running_jobs() {
        // [1, 2] → 3：无间隙时返回「下一个」编号，与单调递增计数器在该场景下等价。
        let mut jobs = vec![
            spawn_running_job(1, "sleep 30"),
            spawn_running_job(2, "sleep 30"),
        ];
        assert_eq!(allocate_job_id(&jobs), 3);
        for j in &mut jobs {
            kill_job(j);
        }
    }

    #[test]
    fn allocate_reuse_smallest_gap() {
        // [1, 3] → 2：中间间隙优先复用，复刻题面示例「if you have jobs [1] and [3]
        // running, the next job gets [2], not [4]」。
        let mut jobs = vec![
            spawn_running_job(1, "sleep 30"),
            spawn_running_job(3, "sleep 30"),
        ];
        assert_eq!(allocate_job_id(&jobs), 2);
        for j in &mut jobs {
            kill_job(j);
        }
    }

    #[test]
    fn allocate_reuse_after_first_removed() {
        // [2, 3] → 1：首位空缺也回收。复刻 tester 流程 A 中
        // 「全部完成 + reap 移除 + 表空」之外的另一边界——表非空但 1 号空缺。
        let mut jobs = vec![
            spawn_running_job(2, "sleep 30"),
            spawn_running_job(3, "sleep 30"),
        ];
        assert_eq!(allocate_job_id(&jobs), 1);
        for j in &mut jobs {
            kill_job(j);
        }
    }
}
