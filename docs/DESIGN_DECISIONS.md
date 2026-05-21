# 设计决策与技术选型

> 本文档以「问题 / 选择 / 原因 / 代价 / 备选方案」五段式记录关键设计决策。
> 代码中带 `详见 docs/DESIGN_DECISIONS.md#<anchor>` 的交叉链接全部指向本文锚点；
> 锚点 ID 一经确定不再变更。

## 目录

- [1. 依赖选择](#1-依赖选择) — `deps-choice`
- [2. 并发与所有权模型](#2-并发与所有权模型) — `rc-refcell-vs-arc-mutex`
- [3. 解析器架构](#3-解析器架构) — `parser-architecture`
- [4. 后台作业与 reaping](#4-后台作业与-reaping) — `background-reaping`
- [5. Pipeline 实现](#5-pipeline-实现) — `pipeline-prev-output`
- [6. 重定向 sink 抽象](#6-重定向-sink-抽象) — `redirect-sink`
- [7. TAB 补全状态机](#7-tab-补全状态机) — `completion-state-machine`
- [8. 测试策略](#8-测试策略) — `testing-strategy`

---

<a id="deps-choice"></a>

## 1. 依赖选择

### 问题

shell 必须提供交互式行编辑（光标移动、历史回溯、TAB 补全），并且要尽量保持依赖最小化。
仓库初始模板曾列入 `anyhow` / `thiserror` / `bytes` 三个常见库，但全代码搜索后发现实际并无 `use` 引用。

### 选择

唯一运行时依赖保留 **`rustyline = "14"`**，移除 `anyhow` / `thiserror` / `bytes`。
错误类型用手写 `enum ParseError + impl Display, Error`（见 [parser/mod.rs](../src/parser/mod.rs)），不引入派生宏。

### 原因

- **rustyline** 是 Rust 社区事实标准 readline 实现，14.x 版本提供：
  - 行编辑（光标移动、Backspace / Ctrl-A/E、history 上下翻）
  - 持久 history 栈（与本仓库 `history_io` 模块协作）
  - `Completer` / `Hinter` / `Highlighter` / `Validator` / `Helper` 五件套 trait —— 仅需实现 `Completer`，其它三 trait 默认 impl 即可
  - Ctrl-C / Ctrl-D 的标准 ReadlineError 分支，主循环按 `Eof` / `Interrupted` 显式处理
- **`anyhow`** 适合「应用顶层错误聚合」，但本项目错误路径都是局部 `io::Result` 或自定义 `ParseError`，多数 builtin 路径用「打印到 err_sink 后吞错」策略，根本不构造 trait object error，引入 `anyhow` 只增编译时间不增收益。
- **`thiserror`** 派生宏的价值在节省 `impl Display for ParseError` 样板代码，但本项目 `ParseError` 只有 7 个 variant，手写 `match` 反而更直白且零宏展开成本。
- **`bytes`** 提供 `Bytes` / `BytesMut` 零拷贝 buffer，但 shell 的字符串路径都是 owned `String` + `&str`，无 zero-copy slicing 场景。

### 代价

- **手写 readline 不在考虑范围**：标准 termios + raw mode + 屏幕重绘 ≥ 1000 行代码，且要处理 SIGWINCH / 多字节字符宽度等细节；与 codecrafters shell 课程目标（学习 shell 概念而非 readline 实现）背离。
- **rustyline 14 的局限**：`Completer::complete` 签名锁死 `&self`，状态字段必须用 `Cell` / `RefCell` 内部可变性承载；详见 [completion-state-machine](#7-tab-补全状态机) 章。
- **手写 ParseError**：未来新增 variant 时需同步更新 `impl Display`（4 处文字），轻微维护成本。

### 备选方案

| 方案 | 评估 |
|---|---|
| 手写 readline + termios | 工作量大 1 个数量级，无项目目标收益。否决。 |
| `linefeed` crate | 不维护，trait 设计不如 rustyline 清晰。否决。 |
| `reedline` (nushell) | 设计更现代但接口绑定较强，适合更复杂场景。本项目暂无需求。延后。 |
| 保留 `anyhow` + `thiserror` 备用 | 增加编译时间与二进制体积，无实际收益。否决。 |

---

<a id="rc-refcell-vs-arc-mutex"></a>

## 2. 并发与所有权模型

### 问题

REPL 主循环中三类共享状态需要跨多个调用点持有：

1. **`completions`**：`complete -C` 注册的命令名 → 补全脚本路径表
   - 写端：dispatch `"complete"` arm
   - 读端：`ShellHelper` 的 `Completer::complete` 路径
2. **`jobs_table`**：后台作业表
   - 写端：`exec::run_external` / `exec::run_pipeline` 后台分支
   - 读端：`builtins::jobs` 的 `run_jobs` + 主循环 prompt 前 reap 三步
3. **`shell_vars`**：`declare NAME=VALUE` 写入的变量后端
   - 写端：dispatch `"declare"` arm
   - 读端：`parser::parse_pipeline` 的 `$VAR` 展开

REPL 是单线程的，但 `ShellHelper::complete` 签名锁死 `&self`，需要内部可变性。

### 选择

**`Rc<RefCell<T>>`** 用于全部三类共享状态。

```rust
let completions: Rc<RefCell<HashMap<String, String>>> = Rc::new(RefCell::new(HashMap::new()));
let jobs_table: Rc<RefCell<Vec<Job>>> = Rc::new(RefCell::new(Vec::new()));
let shell_vars: Rc<RefCell<HashMap<String, String>>> = Rc::new(RefCell::new(HashMap::new()));
```

写端用 `borrow_mut()`，读端用 `borrow()`；借用作用域严格收敛在表达式 / arm 内，避免跨 readline 阻塞调用持有借用。

### 原因

- **REPL 单线程**：rustyline `readline` 是阻塞同步调用；后台子进程通过 `Command::spawn` 启动后由 OS 调度，**父进程内** 没有其它 Rust 线程接触这三张表。
- **零同步开销**：`Rc<RefCell<>>` 在单线程下没有锁/原子开销，比 `Arc<Mutex<>>` 快一个数量级。
- **借用检查器静态保证**：所有借用模式都被 `RefCell` 运行时校验；本项目设计上「短借用 + 不跨阻塞调用」的纪律使得 RuntimeError 永远不会触发。
- **`Rc::clone` 廉价**：仅原子计数器自增；可以放心地传给 `ShellHelper::new(completions.clone())`。

### 代价

- **不能跨线程**：若未来引入异步 reaper 线程（信号驱动 SIGCHLD 回收子进程），需要把 `Rc` 升级为 `Arc`、`RefCell` 升级为 `Mutex` 或 `RwLock`。
- **运行时 borrow panic 风险**：嵌套借用（同时 `borrow()` 与 `borrow_mut()`）会导致 panic。本项目通过纪律避免——所有 `borrow()` 表达式都是单语句临时借用。

### 备选方案

| 方案 | 评估 |
|---|---|
| `Arc<Mutex<T>>` | 单线程下纯开销，无收益。延后到引入异步 reaper 时再切。 |
| `Cell<T>` + `Copy` 类型 | `HashMap` / `Vec` 不 Copy，不适用。 |
| 全局静态（`thread_local!`） | 隐藏依赖关系，使测试与 dispatch 不可见，反模式。 |

---

<a id="parser-architecture"></a>

## 3. 解析器架构

### 问题

shell 输入需要完成 7 类语义转换：

1. 单引号字面保留
2. 双引号 + 4 字符转义（`\\` `\"` `\$` `` \` ``）
3. 引号外反斜杠转义 + 行尾孤立反斜杠报错
4. `$VAR` / `${VAR}` 变量展开（双引号内 + 引号外触发；单引号字面）
5. `>` / `1>` / `2>` / `>>` / `1>>` / `2>>` 六类重定向算子识别
6. `|` 管道切分 + 段末 `&` 后台标志
7. null word removal（bash 行为：unquoted 展开全空 word 丢弃）

### 选择

**手写两段式解析器**：

- `parser::tokenize`（382 行）：字符级状态机（`Normal` / `InSingleQuote` / `InDoubleQuote` 三态），单次线性扫描完成 1-4 + 5 的 token 拼接。
- `parser::parse::collect_redirects` + `parse_pipeline`：在 token 序列上做语法识别，组装 `ParsedCommand` / `Pipeline` 结构。

错误类型用手写 `enum ParseError`（7 个 variant）+ `impl Display + Error`。

### 原因

- **状态机能精确表达 shell 引号语义**：引号嵌套并不是上下文无关文法，传统 LL/LR 解析器表达成本高。状态机三态加上 `look_ahead` 处理 `\X` 转义是最直白的写法。
- **单次线性扫描**：tokenize 是 O(n)，无回溯，性能足够。
- **null word removal 不需要二次扫描**：在 tokenize 内部 flush token 时用「本 word 是否仅由 unquoted 未命中展开贡献」标志位即可实现。
- **错误位置精确**：手写状态机能在每个错误分支返回更具体的 variant（`UnterminatedSingleQuote` / `UnterminatedDoubleQuote` / `TrailingBackslash` / `BadSubstitution` / `UnterminatedBraceExpansion` / `MissingRedirectTarget` / `EmptyPipelineSegment`）。
- **NAME 字符级 helper 跨模块同源**：`tokenize::is_name_start` / `is_name_cont` 同时被 `$VAR` 展开扫描和 `builtins::declare::is_valid_identifier` 复用，跨 stage 100% 同源——避免「declare 校验」与「展开 NAME 扫描」字符集分裂的隐患。

### 代价

- **维护成本**：新增引号语义（如 ANSI-C `$'...'`）需要在状态机加 variant + 状态转换边。
- **不复用社区组合子**：解析器组合子库（nom / chumsky）的「同样的 grammar 写法 vs 不同的 input 类型」抽象不可达。

### 备选方案

| 方案 | 评估 |
|---|---|
| **nom**（解析器组合子） | 表达 shell 引号嵌套很别扭，需要 `delimited` + `escaped` + 大量 lifetime 标注。否决。 |
| **chumsky** | 错误恢复机制丰富，但语法定义需要 boxed combinator，编译时间显著增加。否决。 |
| **pest**（PEG） | 需要外部 `.pest` 语法文件 + 派生宏，部署更复杂；shell 语法不规则也不适合 PEG。否决。 |
| **lalrpop**（LALR） | shell 引号语义不在 LALR 文法表达力范围内。否决。 |

---

<a id="background-reaping"></a>

## 4. 后台作业与 reaping

### 问题

后台进程（`&` 触发）spawn 后必须：

1. 子进程退出时 reap（避免僵尸残留 zombie 进程占用 PID 表）
2. 在用户下次 prompt 前显示 `Done` 通知行
3. `jobs` builtin 能列出当前所有 Running / Done 项
4. 同时不能在子进程仍 Running 时阻塞 REPL

bash 真实实现走 SIGCHLD 信号处理器 + 信号驱动状态机。

### 选择

**非阻塞 `try_wait()` + 每轮 prompt 前 reap 三步原子操作**：

```rust
// 主循环每轮 prompt 前：
{
    let mut tbl = jobs_table.borrow_mut();
    advance_job_status(&mut tbl);   // 对每个 Running 项调 try_wait
    let mut out = io::stdout().lock();
    let _ = render_done_jobs(&mut out, &tbl);  // 写 Done 行
    let _ = out.flush();
    retain_running_jobs(&mut tbl);  // 一次性移除所有 Done
}
```

`run_jobs` 入口也调一次 `advance_job_status` 兜底覆盖 prompt 间窗口。

### 原因

- **`try_wait()` = `waitpid(WNOHANG)`**：非阻塞查询子进程状态，存活时返回 `Ok(None)`，已退出时返回 `Ok(Some(_))` 并完成 reap（内核释放 PID 表项）。
- **三函数原子拆分**：状态推进 / 渲染 / 移除三个职责独立。`run_jobs` 渲染含 Running 行（带 `&` 尾），自动 reap 路径只渲染 Done 行——两条调用路径共用 `advance_job_status` + `retain_running_jobs`，仅渲染策略不同。
- **不需要信号处理器**：本项目不引入 `signal-hook` / `signalfd` 依赖；prompt 间隔（用户思考输入下一条命令的时间）远长于子进程退出延迟，自然 reap 时机良好。
- **pipeline 多 Child 支持**：`Job.children` 是 `Vec<Child>`，`advance_job_status` 遍历全部 child，任一仍 `Running` 即整个 Job 保持 `Running`；与 bash `wait` pipeline 行为一致。
- **静默失败原则**：写 stdout 失败 / try_wait Err（极罕见的 ECHILD 已被外部 reap）一律静默吞掉或视为 Done，保证 REPL 永不中断。

### 代价

- **Done 通知有延迟**：用户必须按 Enter 触发 readline 返回才会看到 `[1]+ Done`；bash 因为有 SIGCHLD 信号处理器可以在子进程退出瞬间打印（虽然多数 shell 也是 prompt 前打印）。
- **不支持 `Stopped`（SIGSTOP / SIGTSTP）**：本阶段 `JobStatus` 仅有 `Running` / `Done` 两态，未实现 Ctrl-Z 暂停 / `fg` / `bg`。

### 备选方案

| 方案 | 评估 |
|---|---|
| `signal-hook` + SIGCHLD 异步处理 | 跨平台一致性差（Windows 无信号）；引入信号处理器需要小心可重入；prompt 前 reap 已满足需求。延后。 |
| `signalfd`（Linux 专属） | 与 rustyline 主循环阻塞 readline 冲突，需要 epoll/select 重写主循环。改造成本高。否决。 |
| 独立 reaper 线程 | 引入 `Arc<Mutex<Vec<Job>>>`、跨线程 `Child` 句柄分享复杂。否决。 |
| 不做 reap（依赖 Drop） | `Child::drop` 默认不 wait，子进程永远变成僵尸。**否决**（严重 bug）。 |

---

<a id="pipeline-prev-output"></a>

## 5. Pipeline 实现

### 问题

N 段命令以 `|` 串联时，每段 stdout → 下一段 stdin。Rust 标准库的 `Command` API 提供 `Stdio::piped()` / `Stdio::from(child_stdout)`，但有一个严重坑：

**如果父进程持有任何 `ChildStdout` 句柄不 drop，下游进程永远收不到 EOF**。
典型表现：`tail -f file | head -n 5` 中 head 满 5 行后退出，但父进程残留 tail 的 ChildStdout → 内核不发 SIGPIPE → tail 永远不退出 → pipeline 永远等待。

同时，本项目允许 builtin（`echo` / `pwd` / `type`）出现在 pipeline 段内，但 builtin 是父进程同步执行的——builtin 的输出没有 OS pipe 写端，需要另一种方式喂入下游 stdin。

### 选择

**`PrevOutput` 三态枚举驱动段间数据流**：

```rust
enum PrevOutput {
    None,                    // 首段，或上一段输出被丢弃
    Buffer(Vec<u8>),         // 上一段是 builtin：输出缓冲在内存
    ChildPipe(ChildStdout),  // 上一段是 external：走真正的 OS pipe
}
```

每段处理完后 `prev = ...` 更新；下一段开始时 `std::mem::replace(&mut prev, PrevOutput::None)` 强制 move 出值，绝不保留任何 ChildStdout 引用。

Pipeline 中的 builtin 子集限定为「纯输出 + 无 shell 状态副作用」的 `echo` / `pwd` / `type`；`cd` / `exit` / `complete` / `jobs` / `history` / `declare` 按 `command not found` 处理（避免污染父 shell 状态）。

### 原因

- **三态精确刻画段间数据流的三种来源**：
  - `None` = 不需要 stdin（首段从父继承 stdin）
  - `Buffer` = builtin 输出在内存（下一段需要 `Stdio::piped()` + 父进程写入）
  - `ChildPipe` = external 输出走 OS pipe（下一段直接 `Stdio::from(cs)`，零拷贝）
- **强制 move 保证 fd 关闭**：`std::mem::replace(&mut prev, PrevOutput::None)` 把旧值取出后立即传给 `Stdio::from(child_stdout)` 让 Command 接管所有权——父进程进程内不再有该 ChildStdout 句柄，dup2 完成后 OS 自动关闭父进程那一端，下游 EOF 链正常触发。
- **builtin 父进程同步执行的避坑**：把 builtin 在子进程内执行会引入 `fork + dup2` 的复杂性；本项目选择「builtin 输出缓冲到 `Vec<u8>`、下一段 spawn 后向 child.stdin write_all」，避免 fork、绕过 `Send` 约束。
- **段内重定向优先级 > pipe**：`cmd1 > out | cmd2` 中 cmd1 走 File，cmd2 收到 EOF 是正确 bash 行为。
- **SIGPIPE 自然回收**：`tail -f | head -n 5` 中 head 退出关闭 pipe 读端 → tail 下次 write 收 SIGPIPE 默认终止；本函数仅 `wait` 全部子进程，依赖内核默认信号语义即可。

### 代价

- **Builtin 缓冲限制**：`echo $very_large_var | wc -c` 这类大缓冲会占用父进程内存；当前 shell 用法下 builtin 输出 ≤ 几 KB，无实际问题。
- **不支持 builtin 流式输出**：bash 在 pipeline 段内对 builtin 也开子 shell（fork），输出走真正 pipe；本项目简化为缓冲。

### 备选方案

| 方案 | 评估 |
|---|---|
| Builtin 内置 fork + 在子进程内执行 | 复杂度激增；本项目 builtin 触发的输出量都很小，缓冲方案足够。否决。 |
| 用 `os_pipe` crate 显式创建 fd 对 | 与标准库 `Command::stdout(Stdio::piped())` 等价，但需要更多手动 dup2。无收益。 |
| 不支持 pipeline 中的 builtin | 题面要求 `echo hello | wc -c` 等组合可用。否决。 |

---

<a id="redirect-sink"></a>

## 6. 重定向 sink 抽象

### 问题

shell 内建命令（`echo` / `pwd` / `type` / ...）和外部命令都需要把 stdout / stderr 输出到：

- 默认：父进程的 stdout / stderr（终端）
- 显式重定向：`> file` / `>> file`（stdout）/ `2> file` / `2>> file`（stderr）

builtin 在父进程内执行，用 `dyn Write`；外部命令在子进程，用 `process::Stdio`。两个 API 差异大但语义一致。

### 选择

**`Box<dyn Write>` 统一 builtin sink；外部命令分支独立物化 `Stdio`，二者共享同一打开模式 helper**：

```rust
// builtin 用：Box<dyn Write>
pub fn open_sink(stdout_redirect: Option<&str>, append: bool) -> io::Result<Box<dyn Write>>;
pub fn open_err_sink(stderr_redirect: Option<&str>, append: bool) -> io::Result<Box<dyn Write>>;

// 外部命令用：File（再转 Stdio::from）
pub fn open_file_for_redirect(path: &str, append: bool) -> io::Result<File>;
```

三函数都在 `src/redirect.rs`，共享同一 `OpenOptions` 模式：`append=true` → `create + append`；`append=false` → `File::create`（O_WRONLY|O_CREAT|O_TRUNC）。

### 原因

- **`Box<dyn Write>` 让 builtin 调用点零分支**：runner 拿到 `&mut dyn Write` 不需要知道目标是终端还是文件；`writeln!(sink, ...)` 一致。
- **外部命令二次物化的取舍**：sink 已经是 `Box<dyn Write>`，运行时无法把它拆回 `File` 给子进程 `Stdio::from`；故对外部命令重新调 `open_file_for_redirect` 物化一次 `File`。代价是两个 fd 指向同一文件，但 `OpenOptions::append` 模式下 kernel 保证两个 fd 写入都原子追加到末尾，不会互相覆盖；`File::create` 模式下截断是幂等的（两次 truncate 仍是空）。
- **共享 helper 避免漂移**：`builtin sink 用 append=true` 与 `外部命令 stdio 用 append=false` 这类不一致是潜在 bug；用同一个 `open_file_for_redirect` 集中模式选择，新增模式（如 `>>+ noclobber`）改动收敛。
- **错误信道**：写 sink 失败由各 runner 决定行为（通常打印到 err_sink）；err_sink 自身写失败兜底 `eprintln!` 到父进程 stderr，避免 REPL 中断。

### 代价

- **`Box<dyn Write>` 一次堆分配**：每条命令一次 `Box::new(io::stdout())` 或 `Box::new(File)`，热路径上有少量分配开销；shell 交互场景每秒 ≤ 10 条命令，无实际影响。
- **fd 双开**：外部命令物化时再开一个 fd，理论上是浪费。可接受的工程化退化。

### 备选方案

| 方案 | 评估 |
|---|---|
| `enum Sink { Stdout, File(File) }` 替换 `Box<dyn Write>` | 调用点必须 match 分支，增加 boilerplate。否决。 |
| sink 用 `&mut impl Write` 泛型 | builtin 函数必须泛型化，与 dispatch 的统一 dyn 调用冲突。否决。 |
| 自定义 trait `ShellSink: Write` | 无新方法可加，纯命名学。否决。 |

---

<a id="completion-state-machine"></a>

## 7. TAB 补全状态机

### 问题

实现「bash 风格」的 TAB 补全需要覆盖三种语境：

1. **命令名补全**（首词位置）：候选源 = builtin 列表 + PATH 中可执行文件
2. **参数路径补全**（参数位置）：候选源 = cwd / 嵌套目录 entry
3. **命令级脚本补全**（`complete -C <path> <cmd>` 注册）：候选源 = 用户脚本 stdout

每种语境都要支持「双 TAB」节奏（无候选响铃、多候选首次 BEL → 二次列出 + 重画提示符）。
rustyline `Completer::complete` 签名为 `&self`——但状态机的「上次 TAB key」必须在两次调用间持久。

### 选择

**三套独立 `Cell<Option<...>>` 状态字段，互斥清空 + LCP 首末项算法**：

```rust
pub struct ShellHelper {
    path_executables: Vec<String>,
    last_tab_prefix:      Cell<Option<String>>,                          // 命令名分支
    last_tab_arg_key:     Cell<Option<(String, String)>>,                // 参数路径分支
    last_tab_script_key:  Cell<Option<(String, String, String)>>,        // 命令级脚本分支
    completions:          Rc<RefCell<HashMap<String, String>>>,
}
```

任一分支被触发时清掉对侧两个 Cell，避免节奏污染。
LCP 算法：候选已字典序排序后，**首末两项的公共前缀 == 全集 LCP**。O(n + L) 优于朴素 O(n·L)。

### 原因

- **`&self` 锁死可变性**：rustyline 14 `Completer` trait 把 `complete` 签名定为 `fn complete(&self, ...)`，无法持有 `&mut self`。`Cell<Option<String>>` 在 `&self` 下提供 take/set 模式，是 stable Rust 中最轻量的内部可变性方案。
- **三套独立 key 类型**：命令名 key 是单 `String`（整行 `line[..pos]`）、参数 key 是 `(dir_part, name_prefix)`、脚本 key 是 `(cmd, current, prev)`——key 形态决定了「同一节奏」的边界，强行合并会产生「在命令名 BEL 一次 → 切到参数立即列出」的污染。
- **互斥清空**：任一分支返回路径都清掉对侧两套 Cell。这是一个简单的不变量，能阻断所有交叉污染。
- **LCP 首末项性质**：在字典序排序后的候选数组中，介于首末项之间的任何字符串其各位置字符都落在首末项相应位置之间或相等——故全集 LCP 与首末项 LCP 相等。这避免了朴素的 O(n·L) 两两比较。
- **`extract_arg_prefix` 复用 tokenize**：参数补全要剥引号、按 `/` 切分；让 `tokenize`（合并）+ `split_dir_and_name`（拆）形成清晰链路。

### 代价

- **三套 Cell 状态字段**：`ShellHelper` 结构体比单 Cell 略胖（24 字节 vs 8 字节，与 `Vec<String>` 相比可忽略）。
- **`Cell::take()/set()` 模式**：每次调用都要 take → 判断 → set，比 `&mut self` 直接读写多一层。可读性略降。
- **`print!` + `flush` 出口**：双 TAB 路径返回 `(pos, vec![])` 让 rustyline 不触碰 line buffer，自己 print BEL / 列表 / 重画提示符。提示符常量 `"$ "` 必须与 `main.rs::editor.readline("$ ")` 字面一致——耦合点已在两处注释标注。

### 备选方案

| 方案 | 评估 |
|---|---|
| `&mut self` 状态 + 把状态外置到 `Rc<RefCell<>>` 让 helper 通过 Rc 持有 | 多一层间接 + 多一次 `borrow_mut`，无收益。否决。 |
| 单 Cell + enum tag | 仍然要 take/set；类型不安全。否决。 |
| 完全无状态（每次 TAB 重新计算）| 双 TAB 节奏丢失（首次 BEL / 二次列出）的语义无法实现。否决。 |
| 朴素 LCP O(n·L) | 候选数 ≤ 几十时差异可忽略；保留首末项算法因为更优雅。 |

---

<a id="testing-strategy"></a>

## 8. 测试策略

### 问题

shell 项目需要覆盖：

- **格式契约**：`jobs` 行宽度 / `history` 编号对齐 / `declare -p` 转义规则等
- **解析正确性**：60+ 引号 / 转义 / 变量展开 / 重定向 / pipeline 组合
- **进程级语义**：pipeline EOF 链 / 后台 stdio 继承 / SIGPIPE 回收
- **本地快速反馈** + **codecrafters 远端 grading** 双轨

### 选择

**三层金字塔**：

1. **`src/parser/tests.rs`**（60+ 测试）：tokenizer / parser 单元测试，集中存放可访问私有 API。
2. **`src/builtins/*.rs` 内 `#[cfg(test)] mod tests`**（~30 测试）：每个 builtin 跟着源码走，测试 sink 输出格式 / 状态机契约。
3. **`tests/` 集成测试**（4 文件 ~1500 行）：spawn shell 二进制 + 真实 pipe / FIFO 验证进程级语义：
   - `pipeline_basic.rs` — N 段管线基础功能
   - `pipeline_builtin.rs` — pipeline 中 builtin 段（缓冲喂入）
   - `jobs_builtin.rs` — `jobs` 列表 + 自动 reap
   - `background_stdio.rs` — 后台进程 stdio 继承（FIFO 验证）

### 原因

- **单元测试集中在源码同目录**：拆分后 `builtins/jobs.rs` / `builtins/declare.rs` / `builtins/history.rs` / `builtins/complete.rs` / `completion/helpers.rs` / `completion/script.rs` 各自带 `#[cfg(test)] mod tests`，能访问私有函数（避免改成 `pub(crate)` 只为测试）。
- **集成测试用 FIFO 验证 stdio 继承**：`cat /tmp/fifo &` 后台后向 fifo 写入数据，断言数据出现在 shell 的 stdout——这是「后台进程通过 dup2 继承父 shell 终端 fd」的最直接验证。普通 pipe 也可以但 FIFO 更接近真实交互场景。
- **`tests/common/mod.rs` spawn helper**：4 个集成测试共享同一 spawn shell + 收集 stdout/stderr 的辅助函数，避免重复样板。
- **格式契约用 `format!` 期望字符串逐字节匹配**：例如 `assert_eq!(out, "[1]+  Running                 sleep 10 &\n")` 直接锁死 24 字符宽 + 2 空格分隔 + 尾 `&` 等所有不变量。

### 代价

- **`spawn_running_job` 测试需要真实 `sleep 30` 进程**：每次跑测试创建若干短命子进程，CI 上略增开销；用例都有 `kill_job` 兜底回收，无残留。
- **FIFO 测试 Linux 专属**：`mkfifo` 在 Windows 上不可用；本项目 codecrafters 平台是 Linux，无 portability 问题。

### 备选方案

| 方案 | 评估 |
|---|---|
| 全集成测试（no unit） | 反馈太慢，无法精确定位格式回归。否决。 |
| 全单元测试（no integration） | 进程级 fd 继承 / SIGPIPE 等无法在单元测试覆盖。否决。 |
| 用 `assert_cmd` crate | 引入第三方依赖；本项目 spawn helper 60 行即可，无需额外依赖。延后。 |
