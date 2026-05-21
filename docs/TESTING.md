# 测试指南

> 项目测试组织、运行方式与扩展指引。

## 目录

- [1. 测试组织](#1-测试组织)
- [2. 单元测试](#2-单元测试)
- [3. 集成测试](#3-集成测试)
- [4. FIFO / 管道验证 stdio 继承](#4-fifo--管道验证-stdio-继承)
- [5. 新增测试指引](#5-新增测试指引)
- [6. 常用命令](#6-常用命令)
- [7. 与 codecrafters 远端测试的关系](#7-与-codecrafters-远端测试的关系)

## 1. 测试组织

```
codecrafters-shell-rust/
├── src/
│   ├── parser/tests.rs                       — 60+ tokenizer / parser 单元测试
│   ├── builtins/
│   │   ├── jobs.rs       #[cfg(test)] mod tests — Job/JobStatus + reap 三函数 + allocate_job_id 用例
│   │   ├── complete.rs   #[cfg(test)] mod tests — -C/-p/-r 三路分派回归
│   │   ├── history.rs    #[cfg(test)] mod tests — 编号对齐 + 全局位置 + 错误信道
│   │   └── declare.rs    #[cfg(test)] mod tests — 5 路分派 + NAME 校验 + 转义规则
│   └── completion/
│       ├── helpers.rs    #[cfg(test)] mod tests — LCP / split_dir_and_name / classify_path / format_arg_completion
│       └── script.rs     #[cfg(test)] mod tests — extract_completer_context + parse_completer_stdout
└── tests/
    ├── common/mod.rs                         — spawn helper（spawn shell 二进制 + 收 stdout/stderr）
    ├── pipeline_basic.rs                     — N 段管线基础功能
    ├── pipeline_builtin.rs                   — pipeline 中 builtin 段（缓冲喂入）
    ├── jobs_builtin.rs                       — jobs 列表 + 自动 reap
    └── background_stdio.rs                   — 后台进程 stdio 继承（FIFO 验证）
```

测试金字塔：

| 层 | 数量 | 反馈速度 | 覆盖目标 |
|---|---|---|---|
| 单元测试 | ~216 | <100 ms 全跑 | 函数级行为契约 / 格式不变量 |
| 集成测试 | 5 + 5 + 5 + 1 | ~2 s 全跑 | 进程级语义（fd 继承 / pipe EOF / 后台 reap） |

总计 ≈ 230+ 测试用例，全绿是任何代码变更的前置门槛。

## 2. 单元测试

### parser::tests（最大单元测试集合）

60+ 用例覆盖：

- **引号**：单引号字面 / 双引号 + 4 字符转义 / 嵌套拼接 / 未闭合错误
- **转义**：引号外 `\X` 移除特殊含义 / 行尾孤立反斜杠错误 / 双引号内 `\$` / `\"` / `\\` / `` \` ``
- **`$VAR` 展开**：合法 NAME 命中 / 未命中空串 / 单引号内字面保留 / `${NAME}` 大括号形式 / `BadSubstitution` 错误
- **重定向**：`>` / `1>` / `2>` / `>>` / `1>>` / `2>>` 6 类算子识别 / `MissingRedirectTarget` 错误 / 引号内字面保留
- **Pipeline 切分**：`\|` 边界 / `EmptyPipelineSegment` 错误 / 末尾 `&` 后台标志

### builtins 模块内单元测试

跟着源码走，能访问私有函数（避免改成 `pub(crate)` 只为测试）。例如：

- `builtins::jobs::tests` 用 `spawn_running_job(id, command)` 启动真 `sleep 30` 进程构造 Running Job；用 `spawn_exited_job` 启动 `true` 后主动 `wait` 模拟已退出状态；每用例尾用 `kill_job` 兜底回收。
- `builtins::declare::tests` 覆盖 5 路分派全部边界 + NAME 校验失败回显规则 + 双引号上下文 4 字符转义。

### completion 模块单元测试

`helpers.rs` 与 `script.rs` 都带独立测试。LCP 算法 / split_dir_and_name / classify_path / extract_completer_context / parse_completer_stdout 都是纯函数，测试零外部依赖。

## 3. 集成测试

### tests/common/mod.rs 共享 helper

spawn shell 二进制，提供：

- stdin 写入
- stdout / stderr 收集
- 超时控制
- 子进程 reap

### tests/pipeline_basic.rs

N 段外部命令 pipeline：`cat file | wc -l` / `cat file | head -n 5 | tail -n 2` / pipeline 中段重定向 `cmd1 > out | cmd2`。

### tests/pipeline_builtin.rs

Pipeline 中含 builtin 段：`echo hello | wc -c` / `pwd | grep pwd` / `type echo | head -n 1`。验证 builtin 输出缓冲到 `Vec<u8>` 后正确喂入下一段 stdin。

### tests/jobs_builtin.rs

后台作业 + `jobs` 命令组合：

- `sleep 30 &` 后立即 `jobs` 应见 Running 行
- `sleep 0.1 &` 后等待 + 触发 prompt → Done 行自动显示且 retain 移除
- `[1,3]` 间隙复用：`sleep 30 &; sleep 30 &; sleep 30 &; jobs` 看到三行 → kill 第二个 → `jobs` 见 2 个 Running → 再 `sleep 30 &` 新作业 id=2（最小可用，而非 4）

### tests/background_stdio.rs

后台进程 stdio 继承的最直接验证：

```
mkfifo /tmp/fifo
shell_input: cat /tmp/fifo &
[后台启动 echo hello > /tmp/fifo 写入]
shell_stdout 应捕获到 "hello"
```

证明：后台 `cat` 通过 dup2 继承了父 shell 的 stdout fd，FIFO 写入后 cat 写到的 fd1 直达 shell 的 stdout。

## 4. FIFO / 管道验证 stdio 继承

后台作业的 stdio 继承是 codecrafters「Background Job Output」阶段的核心需求。
本项目的实现保证是 **POSIX fd 继承的标准语义** + Rust `Stdio::inherit()`：

1. shell 启动时，进程的 fd1 / fd2 直接挂在控制终端（tty）上
2. `Command::spawn` 默认（或 `Stdio::inherit()`）通过 `dup2(2)` 把父进程的 fd1 / fd2 复制到子进程的同号 fd
3. fd 复制后**独立存活**，与父进程后续是否 `wait`、`Child` 句柄是否 `drop` **完全无关**
4. 后台 `cat` 阻塞在 FIFO 读时，shell 已返回 readline；FIFO 一旦被写入，cat 唤醒后写 fd1 直达终端

FIFO 测试通过 `mkfifo` + 后台 `cat` + 异步写入 FIFO 验证整条 fd 继承链。详见 `tests/background_stdio.rs`。

## 5. 新增测试指引

### 添加 builtin 单元测试

直接在对应 `src/builtins/*.rs` 的 `#[cfg(test)] mod tests` 内加用例。用 `Vec<u8>` 作 sink：

```rust
let mut sink: Vec<u8> = Vec::new();
let mut err: Vec<u8> = Vec::new();
run_xxx(&mut sink, &mut err, args).expect("ok");
let out = String::from_utf8(sink).expect("utf8 stdout");
assert_eq!(out, "expected\n");
```

### 添加 parser 单元测试

在 `src/parser/tests.rs` 加用例。`tokenize` 与 `parse_pipeline` 都直接可见。

### 添加集成测试

在 `tests/` 下新建 `<feature>.rs`，`use crate common;` 拿到 spawn helper。注意：

- 集成测试 spawn 真实 shell 二进制，所以**不能直接调用 crate 内部 API**——通过 stdin 输入命令、断言 stdout / stderr 输出
- 涉及后台 / FIFO 的测试要兜底 `kill` + `wait` 子进程，避免 CI 残留

## 6. 常用命令

```sh
# 全量测试
cargo test

# 仅 parser 单元测试（快）
cargo test --lib parser

# 单个集成测试文件
cargo test --test pipeline_basic
cargo test --test jobs_builtin
cargo test --test background_stdio

# 跑某个 builtin 模块的测试
cargo test --lib builtins::jobs
cargo test --lib builtins::declare

# 关键字过滤
cargo test allocate_job_id     # 仅跑 allocate_job_id_* 用例
cargo test declare_invalid     # 仅跑 declare 非法 NAME 系列

# 文档构建（必须 0 warning）
cargo doc --no-deps --document-private-items

# Release 构建（codecrafters 远端用 release，本地可加速 spawn 测试）
cargo build --release
```

## 7. 与 codecrafters 远端测试的关系

codecrafters 在远端跑 black-box tester（不读你的 Rust 代码，spawn `./your_program.sh` 与 shell 交互断言行为）：

- **本地集成测试 ⊂ codecrafters 远端测试**：远端覆盖面更广（多阶段累积），本地测试是子集 + 关键回归。
- **本地测试 100% 绿 ≠ 远端 100% pass**：远端会测一些边界（如特定字节序列、超时窗口），本地不一定覆盖。
- **本地测试主要价值**：（1）开发期快速反馈（远端要 1 分钟左右）；（2）锁定格式不变量防止重构回归；（3）覆盖纯函数行为（远端的 black-box 覆盖不到 LCP 算法之类的内部细节）。

建议工作流：

1. 本地 `cargo test` 全绿 → 提交
2. `codecrafters submit` 远端验证 → 通过下一 stage

发现远端某 stage 失败但本地全绿时：本地补一条逐字节匹配的集成测试以锁定该回归。
