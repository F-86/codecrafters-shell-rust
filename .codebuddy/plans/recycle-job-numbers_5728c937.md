---
name: recycle-job-numbers
overview: 把单调递增的 next_job_id 计数器替换为「最小可用正整数」策略：基于 jobs_table 当前内容计算下一个 job id，实现编号回收（表空→1，存在 [1] 缺 2 的间隙→2）。
todos:
  - id: add-allocate-fn
    content: "在 src/builtins.rs 新增 pub fn allocate_job_id(jobs: &[Job]) -> u32 纯函数，线性扫描 (1u32..) 返回首个未占用正整数"
    status: completed
  - id: add-allocate-unit-tests
    content: 在 builtins.rs tests 模块新增 4 条单测：empty→1、[1,2]→3、[1,3]→2、[2,3]→1，验证分配算法核心契约
    status: completed
    dependencies:
      - add-allocate-fn
  - id: refactor-run-external
    content: 改造 src/exec.rs run_external：删除 next_job_id 参数，后台分支调 allocate_job_id 算 id，先借用算后再 borrow_mut push 避免 RefCell 双借
    status: completed
    dependencies:
      - add-allocate-fn
  - id: cleanup-main-counter
    content: 在 src/main.rs 删除 next_job_id 声明，同步移除 run_external 调用处的 &mut next_job_id 参数；可用 [subagent:code-explorer] 防御性扫描残留引用
    status: completed
    dependencies:
      - refactor-run-external
  - id: add-recycling-integration
    content: 在 tests/jobs_builtin.rs 新增 recycle_to_one_when_empty 与 reuse_two_with_one_remaining 两条端到端集成测试，复刻 tester 流程 A/B
    status: completed
    dependencies:
      - cleanup-main-counter
  - id: verify-test-clippy
    content: 运行 cargo build / cargo test 全绿；cargo clippy --all-targets 维持 11 条 pre-existing 警告零新增
    status: completed
    dependencies:
      - add-allocate-unit-tests
      - add-recycling-integration
---

## 用户需求

实现「Recycling Job Numbers」——后台作业编号不再单调递增，而是每次新作业 spawn 时分配**当前作业表中最小的可用正整数**。完成态作业被自动 reap 移除后，其编号即可被后续新作业复用。

## 核心功能

- **表空 → [1]**：作业表空时，新作业编号始终为 1。
- **间隙复用**：[1, 3] 时新作业为 2；[1, 2] 时新作业为 3；[2, 3] 时新作业为 1。
- **跨自动 reap 联动**：每轮 prompt 前自动 reap 移除 Done 项后，下一条后台命令立刻看到清理后的表，分配最小可用编号。
- **现有契约保持不变**：`[N] PID` 通知行格式、Job 入表 id、`jobs` 行的 `+/-/空格` marker 算法（基于 Vec 索引的 last/last-1 规则）、Done 行格式 24 宽 status、自动 reap 渲染路径——全部不动。

## 题面验收场景

- **流程 A（recycle to 1）**：`cat fifo &` ([1]) → 写 fifo 让 cat 退出 → `echo apple` 输出 + 下轮 prompt 前自动 reap 渲染 `[1]+ Done` → `sleep 100 &` 必须打印 `[1] <pid>`，`jobs` 仅含 `[1]+  Running                 sleep 100 &`。
- **流程 B（reuse 2）**：`sleep 100 &` ([1]) + `cat fifo &` ([2]) → 写 fifo 让 cat 退出 → `echo word` + 自动 reap 渲染 `[2]+ Done` → 表只剩 [1] → `sleep 50 &` 必须打印 `[2] <pid>`，`jobs` 含 `[1]-  Running                 sleep 100 &` 与 `[2]+  Running                 sleep 50 &`。

## Tech Stack

- 语言/工具链：Rust 2021、`std::process::Child`、`rustyline` REPL（沿用现有依赖，不引入新 crate）
- 测试：`cargo test` 单元 + 集成测试，集成测试用 `mkfifo` 命令 + `Command::spawn` 二进制驱动子进程（沿用 `tests/jobs_builtin.rs` 既有 `Cleanup` / `drain_until` 工具）

## 实现策略

### 核心算法：最小可用正整数分配（纯函数）

将「分配下一个 job id」从持久化的 `next_job_id: u32` 计数器改为**对当前作业表的纯函数查询**：扫描 `1..` 序列，返回首个不在 `jobs.iter().map(|j| j.id)` 集合中的正整数。

```rust
pub fn allocate_job_id(jobs: &[Job]) -> u32;
```

算法选择：直接 `(1u32..).find(|n| !jobs.iter().any(|j| j.id == *n)).unwrap()`。

- 时间复杂度 O(n^2) 最坏（n = 表长），但本阶段后台作业表典型 ≤ 几十项；常量因子极小，远快于引入 HashSet 的 alloc 开销。
- 线性扫描天然处理「[2,3] → 1」「[1,3] → 2」「[1,2] → 3」「空表 → 1」全部场景，无需排序。
- `unwrap` 安全：`u32` 上界 4G，作业表绝无可能填满；理论上 `(1u32..)` 是有限范围，但实践中不可达。

### 关键技术决策

- **彻底删除 `next_job_id` 计数器**：单调递增的状态字段在「最小可用」语义下完全多余——保留它会引入「计数器与表脱节」的隐性双源真理（single source of truth 原则）。新方案中 `jobs_table` 是唯一数据源，分配是它的派生函数。
- **为什么放在 `builtins.rs` 而非 `exec.rs`**：`allocate_job_id` 是关于 `Job` 集合的纯查询，与 `Job` / `JobStatus` 同模块更内聚；`exec.rs` 仅作为消费方调用，符合现有 `advance_job_status` / `render_done_jobs` / `retain_running_jobs` 的分层风格（数据原子在 builtins，编排在 main/exec）。
- **RefCell 借用管理**：在 `run_external` 后台分支内**先**取不可变借用算 id、**释放**后**再**取可变借用 push——避免 `Rc<RefCell>` 双借 panic：

```rust
let id = allocate_job_id(&jobs_table.borrow());  // 临时借用，表达式末尾即 drop
// ... writeln 通知 ...
jobs_table.borrow_mut().push(Job { id, ... });
```

Rust temporary 在语句末尾 drop，因此 `borrow()` 与 `borrow_mut()` 之间无重叠借用区间，安全。

- **签名简化**：`run_external` 移除 `next_job_id: &mut u32` 参数，调用端 `main.rs` 同步删除该参数与计数器声明。这是 blast radius 控制——一处 API 改动而非两处独立状态字段。
- **与自动 reap 的协作**：上一阶段已实现的 prompt 前自动 reap 链 `advance_job_status → render_done_jobs(stdout) → retain_running_jobs` 已经把 Done 项从表中移除，分配函数读到的就是「已清理表」，分配语义天然正确，无需任何改动。
- **保持通知行/入表使用同一 id**：现有 `exec.rs:155-172` 通知行 `writeln!(stdout, "[{}] {}", *next_job_id, pid)` 与 `Job { id: *next_job_id, ... }` 使用同一值的契约保持不变——在新方案中先 `let id = allocate_job_id(...)`，再 `writeln!("[{}] {}", id, pid)`，再 `push(Job { id, ... })`，三处共享同一 `id` 局部变量。

### 性能 & 可靠性

- 分配函数 O(n^2) 最坏；n ≤ 几十时（典型交互 shell 后台作业数），单次调用 < 1µs，远低于 `Command::spawn` 的毫秒级成本。
- 错误路径：`unwrap()` 在 u32 上界不可达，理论上无 panic 风险；保守做法可改 `unwrap_or(u32::MAX)` 但反而模糊语义，不采用。
- 边界 `jobs.is_empty()`：`(1u32..).find(...)` 首次循环 `n=1`、闭包 `!jobs.iter().any(...)` 在空表上恒为 `true` → 返回 1。无需特判。

### 避免技术债

- 沿用现有原子函数命名风格（`advance_job_status` / `render_done_jobs` / `retain_running_jobs`）：新增 `allocate_job_id`，单一职责、无副作用、易测试。
- 不引入 HashSet / BitVec 等优化结构——本阶段规模无需，YAGNI。

## Implementation Notes

- **logging**：分配路径无新增日志（与既有 spawn 通知保持一致；通知行 `[N] PID` 即唯一可观测信号）。
- **blast radius**：仅修改 `src/builtins.rs`、`src/main.rs`、`src/exec.rs` 三处；`Job` struct / `JobStatus` enum / `BUILTINS` / 解析器 / 重定向 / 补全 全部不动。
- **既有测试零回归**：`run_external` 唯一签名变化是「去掉 `next_job_id` 参数」，没有任何既有断言依赖该参数；既有 builtins 单测均传入手工构造 jobs，不经过 `run_external` 调用链；既有集成测试 `jobs_lists_single_running_background_job` / `jobs_done_then_removed` / `done_appears_before_next_prompt` 仅观察 stdout 输出，分配语义在「单作业」与「连续两作业 [1][2]」场景下与递增计数器**完全等价**，自然通过。
- **clippy baseline**：维持 11 条 pre-existing 警告，零新增。

## Architecture Design

```mermaid
flowchart LR
    A["REPL prompt 前自动 reap"] --> B["jobs_table 移除 Done 项"]
    B --> C["readline 拿到 cmd &"]
    C --> D["run_external 后台分支"]
    D --> E["allocate_job_id(&jobs)"]
    E --> F["writeln stdout '[N] PID'"]
    F --> G["jobs_table.push(Job{id:N,...})"]
    G --> A
```

数据流单向：jobs_table 是唯一权威；分配是它的派生查询；不存在第二份计数器状态需要同步。

## Directory Structure

```
project-root/
├── src/
│   ├── builtins.rs   # [MODIFY] 新增 pub fn allocate_job_id(jobs: &[Job]) -> u32:
│   │                 #   线性扫描 (1u32..) 返回首个未被任何 Job.id 占用的正整数；
│   │                 #   空表→1，间隙→最小可用。无副作用纯函数。
│   │                 # 在 tests 模块新增 4 条单测：
│   │                 #   - allocate_empty_table_returns_one
│   │                 #   - allocate_sequential_after_running_jobs（ids=[1,2] → 3）
│   │                 #   - allocate_reuse_smallest_gap（ids=[1,3] → 2）
│   │                 #   - allocate_reuse_after_first_removed（ids=[2,3] → 1）
│   │                 # 单测构造 Job 直接走 spawn_running_job，避免依赖 try_wait 状态。
│   ├── main.rs       # [MODIFY] 删除 `let mut next_job_id: u32 = 1;` 声明；
│   │                 # 调用 run_external 时同步删除 `&mut next_job_id` 参数。
│   │                 # 保留 jobs_table 与所有自动 reap 三步逻辑不动。
│   └── exec.rs       # [MODIFY] use 新增 allocate_job_id；
│                     # run_external 签名删除 `next_job_id: &mut u32` 参数；
│                     # 后台 spawn 成功分支：
│                     #   1. let id = allocate_job_id(&jobs_table.borrow()); （借用即 drop）
│                     #   2. let _ = writeln!(io::stdout(), "[{}] {}", id, pid);
│                     #   3. jobs_table.borrow_mut().push(Job { id, pid, command, status, child });
│                     #   4. 删除 *next_job_id += 1; 一行
│                     # 顶部模块 doc 中关于 next_job_id 的描述更新为「分配最小可用 id」。
└── tests/
    └── jobs_builtin.rs  # [MODIFY] 新增 2 条端到端集成测试：
                         #   - recycle_to_one_when_empty：tester 流程 A 复刻
                         #     cat fifo & ([1]) → 写 fifo + sleep 让自动 reap 触发
                         #     → echo apple → sleep 100 & 必须出现 `[1] ` 通知；
                         #     再 jobs 必须含 [1]+  Running                 sleep 100 &
                         #   - reuse_two_with_one_remaining：tester 流程 B 复刻
                         #     sleep 100 & ([1]) + cat fifo & ([2]) → 写 fifo 让 [2] 退出
                         #     → echo word + 自动 reap → sleep 50 & 必须出现 `[2] ` 通知；
                         #     jobs 必须同时含 [1]-  Running 与 [2]+  Running
                         # 沿用既有 Cleanup（fifos: Vec<PathBuf>）+ drain_until 工具；
                         # 用 END_SENTINEL_N 哨兵分割窗口，断言「[N] ` 子串出现于
                         # spawn 命令之后、下一哨兵之前」，避免误命中先前 [N] PID 通知。
```

## Key Code Structures

```rust
// src/builtins.rs 新增（仅签名层面）
/// 分配下一个后台作业编号：返回当前 jobs 表中**最小未被占用**的正整数。
/// 表空 → 1；[1,3] → 2；[1,2] → 3；[2,3] → 1。
/// 纯函数，O(n^2) 最坏，n 为表长（典型 ≤ 几十）。
pub fn allocate_job_id(jobs: &[Job]) -> u32;
```

`run_external` 后台分支调用契约（伪代码）：

```
let id = allocate_job_id(&jobs_table.borrow());     // 临时借用，语句末释放
writeln!(stdout, "[{}] {}", id, pid);
jobs_table.borrow_mut().push(Job { id, pid, command, status: Running, child });
```

## Agent Extensions

### SubAgent

- **code-explorer**
- Purpose: 计划执行阶段如需在 `src/exec.rs` / `src/main.rs` / `src/builtins.rs` 之外发现潜在间接调用 `next_job_id` 的位置（理论上不应存在，但用作防御性扫描），可调用其跨文件搜索能力。
- Expected outcome: 确认 `next_job_id` 标识符仅在 `main.rs:56` 声明、`main.rs:206` 传参、`exec.rs:86,93,155,166,172` 使用——共 6 个位置，无遗漏；删除/替换后全代码库零残留引用。