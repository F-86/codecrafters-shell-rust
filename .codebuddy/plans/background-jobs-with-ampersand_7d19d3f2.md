---
name: background-jobs-with-ampersand
overview: 支持以行尾 `&` 启动后台任务：解析层识别 `&` token 并设置 `ParsedCommand.background`，执行层用 `Command::spawn()` 不阻塞地启动子进程，立即打印 `[<job>] <pid>` 后返回提示符。
todos:
  - id: tokenizer-amp
    content: 在 `src/parser/tokenize.rs` Normal 态新增 `&` 独立成 token 的分支，模板照搬 `>`
    status: completed
  - id: parsed-command-bg
    content: 在 `src/parser/parse.rs` 给 `ParsedCommand` 新增 `background` 字段，并在 `parse()` 中识别并剥离尾部 `&`
    status: completed
    dependencies:
      - tokenizer-amp
  - id: parser-tests
    content: 在 `src/parser/tests.rs` 与 `mod.rs` 头注释补充 `&` 语义说明，新增覆盖空格 / 无空格 / 引号字面量 / 重定向共存的单元测试
    status: completed
    dependencies:
      - parsed-command-bg
  - id: exec-background
    content: "改造 `src/exec.rs` `run_external`：新增 `next_job_id: &mut u32` 参数，按 `background` 分流为 `spawn` 不 wait + 打印 `[N] PID`，前台分支保持 `.status()` 不变"
    status: completed
    dependencies:
      - parsed-command-bg
  - id: main-wire-jobid
    content: "在 `src/main.rs` 持有 `let mut next_job_id: u32 = 1;` 并在 dispatch `_` 分支传入 `run_external`，运行 `cargo test` + 手动 `sleep 500 &` 验收"
    status: completed
    dependencies:
      - exec-background
---

## 产品概述

为 Rust 实现的 shell 增加「后台执行」能力：用户在命令末尾追加 `&` 时，shell 启动子进程后立即返回提示符，并打印一行 `[<job>] <pid>` 通知信息。

## 核心功能

- 命令行末尾的 `&` 作为独立 token 被切分，无论前是否有空格（`sleep 30 &` 与 `sleep 30&` 等价）
- 引号内的 `&` 仍按字面量保留（`echo "&"` 输出 `&`，不进入后台）
- 解析阶段识别尾部 `&` 并从 argv 中剥离，结构化命令带 `background` 标志
- 外部命令命中 `background=true` 时：spawn 子进程后不 wait，向终端 stdout 打印 `[<job>] <pid>` 通知，立刻回到提示符
- 作业号从 `1` 递增，跨 REPL 循环存活（本阶段实测仅触发 `[1]`）
- builtin 行的 `&` 本阶段忽略后台语义（仍前台执行，留作未来扩展点）
- 后台任务通知信息走父进程 stdout，不受 `>` / `1>` 用户重定向影响（与 bash 行为一致）

## 技术栈

- 语言：Rust（沿用现有 `Cargo.toml` 工程，无新增依赖）
- 进程模型：`std::process::Command::spawn()` 拿 `Child`，后台分支不调 `wait`
- I/O：复用既有 `&mut dyn Write` sink / err_sink；后台通知专用通道 = `println!` / `io::stdout()`

## 实现策略

最小侵入分三层切入：词法层让 `&` 像 `>` 一样独立成 token；语法层在 tokenize 后检查尾部 token 是否为 `"&"`，若是则弹出并置 `ParsedCommand.background=true`；执行层在 `run_external` 内根据 `background` 分流为 `spawn`（无 wait）或 `status`（同步等待）。

### 关键技术决策

1. **`&` 在词法层独立成 token，与 `>` 对称**：在 `tokenize.rs` Normal 态新增 `'&'` 分支，逻辑模板照搬 `>`：若 `in_token` 为真则先 flush `current`，再 push `"&"`，置 `in_token=false`。这样 `sleep 30&` / `sleep 30 &` 等价，且引号内 `&` 因不进 Normal 态分支天然保持字面量。**不在 parse 层做字符串末尾切分**——避免双重数据源、避免引号内 `&` 误判。

2. **`background` 字段在 `ParsedCommand` 上**：与 `stdout_redirect` / `stderr_redirect` 等元信息字段并列，`parse()` 在线性扫描前优先做一次「尾 token 是否为 `&`」检查并 `pop`。这避免了在主扫描循环里加 `&` 分支并提前 break 的复杂度，且语义清晰：`&` 只能出现在末尾，否则按字面量普通参数处理（与 bash 末尾 `&` 控制操作符语义对齐，本阶段不支持 `cmd1 & cmd2` 复合形式）。

3. **`run_external` 后台分支用 `spawn()` 不 wait**：

- 复用既有 stdio 物化逻辑（`open_file_for_redirect` + `Stdio::inherit`）；
- `Command::spawn()` 返回 `Child`，立即 `let pid = child.id();`，**不持有 `child` 的所有权**（让其 drop——Rust `Child` 默认 drop 不 wait，子进程继续运行，符合题面 C 示例「fork+exec 不 waitpid」的语义）；
- 后台通知 `println!("[{}] {}", job_id, pid)` 直接走父进程 stdout，不复用 `sink`（用户的 `>` 重定向不应捕获 shell 控制信息，与 bash 一致）；
- 提前 `drop(sink); drop(err_sink);` 仍需做，避免父进程残留对重定向文件的写句柄。

4. **`next_job_id: u32` 由 main 持有**：单线程串行 REPL 不需要 `Rc<RefCell<...>>`，直接 `&mut u32` 形参传给 `run_external`。后台 spawn 成功后 `*next_job_id += 1`；spawn 失败不递增（job 号在 bash 里也只在成功后台启动后分配）。

5. **builtin 分支忽略 `background`**：`echo hi &` / `pwd &` 等本阶段仍前台执行。理由：builtin 在父进程内同步运行，"后台 builtin"需要 fork 子 shell（bash 真实行为），违反 YAGNI 且超出题面范围。在 dispatch 注释里明确该行为是临时简化、留作未来扩展。

### 性能与可靠性

- 词法 / 语法层改动均为 O(1) 新增分支与 O(1) 尾 pop，无热路径性能影响；
- `Child` drop 不 wait 会留下僵尸进程，但 codecrafters 测试只验证「立即返回 + PID 存在 + 进程实际跑起来」，不验证 reaper 行为；本阶段不主动 reap，避免 SIGCHLD handler 引入的复杂度。下一阶段实现 `jobs` 列表时再统一处理回收；
- 后台通知失败（极罕见，stdout 被关闭）通过 `let _ = writeln!(...)` 静默吞掉，不阻断 REPL；
- 跨阶段兼容：`ParsedCommand` 新增字段不影响既有重定向 / builtin 语义；现有测试 `_ => argv.push(tok)` 路径行为不变。

## 实现注意事项

- **不要 `wait()` / `try_wait()`**：题面明确「不等子进程结束」；调用了反而会阻塞或语义偏离。
- **后台 `spawn` 失败处理**：与现有 `.status().is_err()` 兜底一致，向 stderr 写 `command not found` 并 return；不递增 `next_job_id`。
- **后台通知行格式严格 `[{job}] {pid}\n`**：方括号紧贴数字，单空格分隔，不要 trailing 空格；测试以正则 / 行匹配验证。
- **重定向与后台共存**：`sleep 30 > out.log &` 应仍能识别——tokenize 已先把 `&` 独立切出，parse 层先剥末尾 `&`、再走重定向扫描，互不干扰；测试新增一条覆盖该组合。
- **引号内 `&` 字面量**：`echo "&"` / `echo '&'` / `echo \&` 三种形式都不应触发后台——前两种由 InSingle/Double 分支天然保留，第三种由 Normal 态 `\\` 分支转义保留，无需额外代码，但需测试锁定。
- **`&` 出现在中间或重定向之后**：本阶段按"非末尾 token = 普通字面量"处理（仅 parse 层尾 pop 检查命中末尾才剥离）。`echo & hi` 会把 `&` 当作普通参数传给 `echo`——与 bash 真实"后台分隔符"语义偏离，但题面 Notes 明确"only one background job"且都是末尾，故可接受。在 `parse()` doc comment 注明该简化。
- **doc comment 同步更新**：`parser/mod.rs` 头注释、`ParsedCommand` 文档、`run_external` 文档都要补一段说明 `&` / `background` 语义与本阶段限制。

## 目录结构

仅修改 5 个文件，不新增 / 删除文件。

```
codecrafters-shell-rust/
└── src/
    ├── parser/
    │   ├── mod.rs        # [MODIFY] 头部模块文档注释新增 `&` 语义段：词法独立 token、
    │   │                 #          末尾出现触发 background、引号内字面量、本阶段限制
    │   ├── tokenize.rs   # [MODIFY] Normal 态 `ch` match 新增 `'&'` 分支，模板照搬 `>`：
    │   │                 #          flush current（若 in_token）→ push "&" → 置 in_token=false
    │   │                 #          引号内分支无需改动（天然字面量）
    │   ├── parse.rs      # [MODIFY] (1) ParsedCommand 新增 `pub background: bool` 字段 + doc
    │   │                 #          (2) parse() 在 tokenize 后先检查并 pop 末尾 "&" token，
    │   │                 #              置 background=true；非末尾的 "&" token 留给主扫描
    │   │                 #              当作 argv（未来阶段再处理复合后台语义）
    │   │                 #          (3) 构造 ParsedCommand 时填充 background 字段
    │   └── tests.rs      # [MODIFY] 新增 4-6 条覆盖：
    │                     #          - `sleep 30 &` → argv=[sleep,30] background=true
    │                     #          - `sleep 30&` 无空格同上
    │                     #          - `echo hi` → background=false
    │                     #          - `echo "&"` / `echo '&'` / `echo \&` → 字面量 & 不触发 bg
    │                     #          - `sleep 30 > out &` → 重定向 + 后台共存
    ├── exec.rs           # [MODIFY] run_external 签名新增 `next_job_id: &mut u32`：
    │                     #          - 在 spawn / status 分支前根据 parsed.background 分流
    │                     #          - 后台分支：.spawn() 拿 Child → child.id() → drop(child)
    │                     #            → println!("[{}] {}", *next_job_id, pid) → *next_job_id+=1
    │                     #          - spawn 失败：写 err_sink "command not found"，不递增 job_id
    │                     #          - 前台分支：保持现有 .status() 语义不变
    │                     #          - drop(sink)/drop(err_sink) 时序保持现状
    └── main.rs           # [MODIFY] (1) main 函数内 `let mut next_job_id: u32 = 1;` 跨循环持有
                          #          (2) dispatch `_` 分支调用 run_external 时传入 &mut next_job_id
                          #          (3) builtin 分支不读 parsed.background（注释说明本阶段简化）
```

## 关键代码结构（仅 1 处接口契约）

```rust
// src/parser/parse.rs
pub struct ParsedCommand {
    pub argv: Vec<String>,
    pub stdout_redirect: Option<String>,
    pub stdout_append: bool,
    pub stderr_redirect: Option<String>,
    pub stderr_append: bool,
    /// 末尾 `&` 触发的后台执行标志：true 表示外部命令需 spawn 而不 wait。
    /// 仅当 tokenize 后的最后一个 token 严格为 "&" 时置位；引号内 `&` 与
    /// 非末尾位置的 `&` 均按字面量留在 argv（本阶段简化，未来阶段再支持
    /// 复合后台分隔语义）。builtin 分支当前忽略此字段。
    pub background: bool,
}
```

## 验证策略

- `cargo test` 全绿（既有 114 条 + 新增 parser 单元测试）
- 手动验证：`sleep 500 &` 输入后 stdout 含 `[1] <pid>` 一行、提示符立即返回、`ps -p <pid>` 可见进程
- 重定向共存：`sleep 5 > /tmp/out &` → stdout 仅显示 `[1] <pid>`、`/tmp/out` 文件创建
- 既有重定向 / builtin / 补全测试无回归