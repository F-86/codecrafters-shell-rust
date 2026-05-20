---
name: comp-line-point-env-vars
overview: 在调用 `complete -C` 注册的补全脚本时，额外向子进程设置 `COMP_LINE`（整行字面）和 `COMP_POINT`（光标字节索引）两个环境变量，仅作用于该子进程，不污染 shell 自身环境。
todos:
  - id: extend-runner-with-env
    content: 扩展 run_completer_script 签名增加 comp_line/comp_point 两参数，在 Command 链上链式 .env("COMP_LINE", ...) 与 .env("COMP_POINT", point.to_string())，并更新函数文档块说明 env 契约与子进程独享语义
    status: completed
  - id: rewire-call-site
    content: 在 Completer::complete 调用点把 line 与 pos 透传给 run_completer_script，验证 cargo build 与 cargo test 全绿
    status: completed
    dependencies:
      - extend-runner-with-env
---

## 用户需求

为已注册的补全脚本调用过程额外注入两个环境变量，使脚本能感知整行原文与光标位置：

- **COMP_LINE**：TAB 按下瞬间的完整命令行字面（无尾随换行）。
- **COMP_POINT**：光标在 COMP_LINE 中的零基字节索引（光标在行尾时等于 COMP_LINE 字节长度）。

两个环境变量与既有 argv[1..3]（cmd / current_word / prev_word）一并传递；仅对补全子进程可见，不污染 shell 自身环境。

## 核心要点

- COMP_LINE 使用整行 `line` 字面（与 bash 一致，鲁棒覆盖光标在行中间的情况）。
- COMP_POINT 使用 rustyline 提供的 `pos`（保证已是 byte index）。
- 作用域隔离：通过 `Command::env(...)` 链式设置，不调用 `std::env::set_var`。
- 题面示例：`git ad<TAB>` → `COMP_LINE="git ad"`，`COMP_POINT=6`。

## 技术选型

沿用既有栈：Rust + `std::process::Command`（无新增依赖）。

## 实现策略

### 高层思路

单一改动点：扩展 `run_completer_script` 函数签名增加 `comp_line: &str, comp_point: usize` 两参数；在 `Command` 链上追加两次 `.env()`；调用点把 `Completer::complete` 入参的 `line` 与 `pos` 透传过去。环境变量天然只对子进程可见，自动满足"不持久化到 shell 自身"约束。

### 关键决策

1. **COMP_LINE 用完整 `line` 而非 `line[..pos]`**：题面"full text of the command line"与 bash 真实语义一致；当前 tester 用例光标在末尾，两种取法值相同，但整行更鲁棒，覆盖未来"光标在中间"的扩展。
2. **不用 `std::env::set_var`**：进程级污染不可接受；`Command::env` 只在子进程 envp 中生效，符合题目 Notes 强约束。
3. **不扩 `CompleterContext` 字段**：`comp_line/comp_point` 是"调用瞬时快照"，与 `cmd/current/prev/literal_len`（tokenize 派生量）属于不同概念层级；混入会模糊语义，且需要把 `line/pos` 同时透传给 `extract_completer_context`，无价值复杂化。
4. **COMP_POINT 字符串化**：环境变量传递必须是字符串，`format!("{}", pos)` 即可（usize 字面无歧义）；脚本侧自行 `int(os.environ['COMP_POINT'])` 解析。
5. **不为 env 透传写运行时集成测试**：真 spawn 子进程 + 临时可执行脚本会引入 tempfile/chmod/平台耦合，超出 stage 必要面；codecrafters tester 自身会校验 env 透传，过测即正确。

### 性能 / 可靠性

- 单次 TAB 触发一次 fork+exec，env 注入是 O(1) 链式调用，无额外开销。
- 子进程异常路径（spawn 失败 / 非零退出 / 非 UTF-8 / 空 stdout）已在既有 `run_completer_script` 内统一收敛为 `None` → 静默 no-op，本次改动不动这块契约。

## 改动清单

```
src/
└── completion.rs  # [MODIFY]
    ├── run_completer_script (第 659-684 行)
    │   ├── 签名新增两参数：comp_line: &str, comp_point: usize
    │   ├── Command 链上追加 .env("COMP_LINE", comp_line) 与
    │   │   .env("COMP_POINT", comp_point.to_string())
    │   └── 函数文档块补充：env 契约（值来源、字节索引、子进程独享语义）
    └── Completer::complete 调用点 (第 296-301 行)
        └── 调用 run_completer_script 时追加传入 line, pos
```

## 实现注记

- **Grounded**：复用 `Command::env` 现有 API（已 `use std::process::Command`），无新引入。
- **回归边界控制**：不动 `extract_completer_context`、registry 查表、双 TAB 状态机、文件名补全 fallback；改动局部、零 blast radius。
- **向后兼容**：env 仅作用于子进程；既有 argv[1..3] 契约保留；脚本若不读 env 行为不变。
- **日志**：补全是高频交互路径，沿用既有"静默 no-op"策略，不新增日志输出，避免污染 TTY。