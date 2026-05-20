---
name: completer-multiple-candidates
overview: 让 `complete -C` 注册的脚本支持多候选返回：runner 改为返回全部非空行；命令级脚本分支按候选数走「单候选直接替换 / 多候选双 TAB 状态机（首次响铃，二次按字母序双空格列出 + 重画提示符）」三态；新增独立的脚本分支 last-tab key 与现有命令名/文件名两个分支互斥。
todos:
  - id: upgrade-runner-multi-candidate
    content: 升级 run_completer_script 返回 Option<Vec<String>>，抽出纯函数 parse_completer_stdout 并补 5 例单测覆盖单行/多行 LF/CRLF/空行场景
    status: completed
  - id: add-script-tab-state-field
    content: 在 ShellHelper 新增 last_tab_script_key 字段并在 new() 初始化为 None
    status: completed
  - id: rewire-script-branch-tristate
    content: 改写 Completer::complete 脚本分支为三态分发：单候选保留替换、多候选 sort + 双 TAB 状态机（首次 BEL、二次列表+重画 `$
    status: completed
    dependencies:
      - upgrade-runner-multi-candidate
      - add-script-tab-state-field
---

## 用户需求

补全脚本可通过多行 stdout 返回多个候选；shell 需在 TAB 触发时按 bash 双 TAB 节奏交互：

## 核心特性

- 脚本输出每行一个候选；shell 以**字母序**列出全部候选。
- **第一次 TAB**：响铃（BEL `\x07`），不修改 line buffer。
- **第二次 TAB**（输入未变）：换行后用至少单空格（采用双空格）拼接全部候选打印，再换一行重画 `$ <原输入>`，光标停在原输入末尾。
- **单候选**：保持上 stage 行为 —— 直接替换为 `<text> `（带尾空格）。
- **零候选 / 脚本异常**：静默 no-op，line 不变。
- 本 stage 显式**不**实现 LCP 扩展（题目 Notes 已说明）。

## 技术栈

沿用既有：Rust + rustyline + `std::process::Command`，零新增依赖。

## 实现策略

### 高层思路

`run_completer_script` 返回类型从 `Option<String>`（仅首行）升级为 `Option<Vec<String>>`（全部非空行）。`Completer::complete` 的脚本分支按返回 vec 长度三态分发：单候选走既有替换路径；多候选走与命令名/文件名分支同款的"独立双 TAB 状态机"模板（不做 LCP）；零候选/None 静默 no-op。

### 关键决策

1. **runner 返回类型升级**：`Option<Vec<String>>`。`stdout.split('\n')` → 每行 `trim_end_matches('\r')` → 过滤空行 → collect。零有效行映射为 `None`，与脚本失败统一收敛到调用点的"静默 no-op"契约。
2. **不做 LCP 扩展**：题目 Notes 明确说本 stage 不需要；命令名/文件名分支虽有 LCP，是各自独立设计，脚本分支严格遵循当前 stage 要求，避免过度工程。后续 stage 若需要再补。
3. **新增独立状态机字段** `last_tab_script_key: Cell<Option<(String, String, String)>>`，key = `(cmd, current_word, prev_word)`：与命令名分支 `last_tab_prefix`、文件名分支 `last_tab_arg_key` **三者并列、互斥清空**。三元组比 `line[..pos]` 更精确锁定"脚本输入语义"，跨多空白等字面差异也能正确合并节奏。`literal_len` 不入 key（仅用于 replacement 起点）。
4. **进入任一分支时清空对侧两个字段**：现命令级脚本分支已清 `last_tab_prefix` 与 `last_tab_arg_key`（第 285-286 行）；新增字段后，命令名分支与文件名分支返回路径也要补清 `last_tab_script_key`，保持"任一分支被触发即作废另两套节奏"的纪律。
5. **抽出纯函数** `parse_completer_stdout(&str) -> Option<Vec<String>>` 便于单测；runner 主体只负责 spawn + status 检查 + 调用解析器。
6. **测试策略**：仅对 `parse_completer_stdout` 加单测（单行 / 多行 LF / CRLF / 末尾空行 / 全空行）；状态机集成依赖 codecrafters tester 验证（与现有两个分支一致）。

### 性能与可靠性

- 解析：单次 stdout `split('\n')`，O(N) 字节扫描。脚本输出量级在 KB 内，无瓶颈。
- 排序：`Vec<String>::sort()`，候选数实测 3~10，O(n log n) 可忽略。
- 错误收敛：spawn 失败 / 非零退出 / 非 UTF-8 / 零有效行 → 一律 `None` → 调用点静默 no-op，与既有契约对齐。

## 实现注记

- **Grounded**：完全复用现有命令名/文件名分支的双 TAB 模板（`take()/set()` + 物理 `print!` + `flush`），不引入新 IO 抽象。
- **Blast radius**：单文件改动，签名变化局部（runner + 一个调用点 + 一个新字段 + 三处对侧清空补丁）；env / argv / registry / ctx 提取均不动，向后兼容。
- **日志**：补全是高频交互路径，沿用静默策略，不输出任何错误日志，避免污染 TTY。
- **状态泄漏防护**：脚本分支返回前显式清/设 `last_tab_script_key`；零候选与单候选都要清空（防"脚本时灵时不灵"造成的残留节奏）。

## 改动清单

```
src/
└── completion.rs  # [MODIFY]
    ├── 模块顶部 doc 注释（第 16-27 行）
    │   └── 「多候选状态机」段补一句：脚本分支并列第三套 key、不做 LCP
    ├── ShellHelper 结构体（第 70-88 行）
    │   ├── 新增字段 last_tab_script_key: Cell<Option<(String, String, String)>>
    │   └── ShellHelper::new() 初始化为 Cell::new(None)
    ├── 命令名分支（第 322-323、343-345、365-366、385-388 行附近）
    │   └── 所有返回路径补清 self.last_tab_script_key.set(None)
    ├── 文件名分支 complete_filename_arg（第 130-167、172-179、190-197、203-230 行）
    │   └── 所有返回路径补清 self.last_tab_script_key.set(None)
    ├── 命令级脚本分支（第 274-316 行）
    │   ├── ctx 命中处保留现有清 last_tab_prefix / last_tab_arg_key
    │   └── match 改为三态：
    │       - Some(names) where names.len() == 1 → 单候选替换 `<text> ` + 清 last_tab_script_key
    │       - Some(names) where names.len() >= 2 → sort + 双 TAB 状态机（首次 BEL 记 key；二次 print 列表 + 重画 `$ <line[..pos]>` 后清 key）
    │       - None → 静默 no-op + 清 last_tab_script_key
    ├── parse_completer_stdout(&str) -> Option<Vec<String>>  # [NEW 纯函数]
    │   └── split('\n') → trim_end_matches('\r') → 过滤空行 → 空 vec 返 None
    ├── run_completer_script（第 670-699 行）
    │   ├── 返回类型 Option<String> → Option<Vec<String>>
    │   └── 主体调用 parse_completer_stdout 替代手写 split + first
    └── #[cfg(test)] 模块新增 parse_completer_stdout 单测 5 例
```

## 关键代码结构

```rust
/// 解析补全脚本 stdout 为候选列表。空行（含尾随 CRLF）被过滤；零有效候选返回 None。
fn parse_completer_stdout(stdout: &str) -> Option<Vec<String>>;

/// 执行脚本并返回候选列表（不排序，由调用方按需排序）。
fn run_completer_script(
    path: &str, cmd: &str, current_word: &str, prev_word: &str,
    comp_line: &str, comp_point: usize,
) -> Option<Vec<String>>;
```