---
name: shell-tab-completion-multiple-matches
overview: 在 ShellHelper::complete 内手写「双 TAB」状态机：单候选直接补全；多候选第一次 TAB 写 \x07 响铃并记忆前缀，第二次 TAB（前缀未变）按字母序在新行打印候选并触发提示符重绘，恢复原前缀。
todos:
  - id: extend-helper-state
    content: "为 ShellHelper 增加 last_tab_prefix: Cell<Option<String>> 字段并在 new() 初始化为 None"
    status: completed
  - id: rewrite-complete-statemachine
    content: 重写 Completer::complete：候选去重后排序，按 0/1/≥2 候选三态分支处理（≥2 时用 last_tab_prefix 实现首次响铃、二次列出 + 重画提示符）
    status: completed
    dependencies:
      - extend-helper-state
  - id: update-module-doc
    content: 补充 completion.rs 顶部 //! 注释，描述双 TAB 状态机与提示符常量同步约束
    status: completed
    dependencies:
      - rewrite-complete-statemachine
  - id: verify-build-and-e2e
    content: 运行 cargo build 与 read_lints 零警告校验，编写 expect 脚本端到端验证题面正本 + 单候选回归 + 状态重置三类用例
    status: completed
    dependencies:
      - update-module-doc
---

## 用户需求

扩展 shell 的 TAB 自动补全，支持「多个候选共享公共前缀」场景的 bash 风格双 TAB 列出行为。

## 核心功能

- **首次 TAB（多候选）**：响 BEL 铃声（`\x07`），不修改当前行。
- **二次 TAB（同一前缀，多候选）**：换行后按字母序、用两个空格分隔列出全部匹配项；下一行重画 `$ ` 提示符与原前缀，等待用户继续输入。
- **单候选**：直接补全为 `<name> `（末尾追加空格），与上一阶段行为一致。
- **0 候选**：静默无操作。
- **状态收敛**：当用户在两次 TAB 之间键入字符使前缀变化时，TAB 状态机重置（再次响铃而非直接列出）。
- **候选源**：`builtins::BUILTINS` 与 `builtins::list_path_executables()` 合并；同名按 builtin 优先去重；PATH 内部不同目录同名也仅取首个；最终按字母序排序输出。

## 技术方案

### 技术栈

- 沿用现有 Rust 1.95 + rustyline 14 + std-only 实现，无新依赖。
- 仅修改 `src/completion.rs`，`src/main.rs`、`src/builtins.rs` 不动（候选源已就绪）。

### 实现策略

**核心思路**：由于 rustyline 14 的 `ConditionalEventHandler::handle` 仅能返回 `Cmd` 枚举（无副作用通道，不可打印/响铃/重绘），故在 `Completer::complete(&self, ...)` 内部用 `Cell<Option<String>>` 维护「上次 TAB 的前缀」状态机，并直接向 stdout 写 BEL/列表/提示符序列；通过返回 `(pos, vec![])` 让 rustyline 维持 line buffer 不变，达到「响铃后保留输入」「列出后保留输入」的效果。

### 状态机三态

对当前光标左侧子串 `prefix = &line[..pos]`：

- **首词守卫**：`prefix` 含空白 → 立即返回 `(pos, vec![])`，状态机不更新。
- 收集候选：先按现有 builtin → PATH 顺序去重收 `Vec<String>`，最终 `candidates.sort()`（字典序即字母序）。
- **0 候选**：`last_tab_prefix.set(None)`；返回 `(pos, vec![])`。
- **1 候选**：`last_tab_prefix.set(None)`；返回 `(0, vec![Pair{display:name, replacement:format!("{} ", name)}])`，沿用上 stage 单候选直补 + 末空格语义。
- **≥2 候选**：
- `last_tab_prefix.take()` 比对当前 prefix：
    - 不等 / None → **首次 TAB**：写入新 prefix；`print!("\x07"); stdout.flush()`；返回 `(pos, vec![])`。
    - 等 → **二次 TAB**：清状态；按字母序后用 `"  "`（两空格）`join`；输出序列 `"\n{joined}\n$ {prefix}"` 后 flush；返回 `(pos, vec![])`。

### 关键技术点

- **内部可变性**：`Completer::complete` 签名锁死 `&self`，使用 `Cell<Option<String>>` + `take()/set()` 模式（`String` 非 Copy，禁用 `Cell::get`；用 `take` 取出再判定后按需写回）。避免 `RefCell` 的运行时借用检查开销，且本路径无嵌套借用。
- **去重 + 排序顺序**：`seen: HashSet<&str>` 借用 builtin 的 `&'static str` 与 `self.path_executables` 字段切片，零分配跟踪重名；先去重收 `Vec<String>` 再 `sort()`，单次 O(N log N)，N ≤ 候选总数（PATH 全量典型几千以内，可接受）。
- **`(pos, vec![])` 而非 `(0, vec![Pair{replacement:prefix}])`**：后者会触发 rustyline 的「单候选直补」路径，可能误加尾空格；空 vec 是干净 no-op，line buffer 完全不被 rustyline 触碰。
- **flush 必须显式**：rustyline 在 `complete` 返回后不会立刻 flush stdout；TAB 是高频交互，BEL 与列表必须立即可见/可闻。
- **二次 TAB 后的光标一致性**：物理上输出为 `\n<list>\n$ <prefix>`，光标停在「`$ <prefix>`」末尾；rustyline 内部认为 line buffer 仍是 `prefix`、光标在 prefix 末尾——两者位置一致。下一次按键的增量 refresh 不会错位（codecrafters 用基于 PTY 字符串匹配，不会校验光标行号）。若个别终端表现异常，fallback 是在重画前先 `print!("\x1b[2K\r")` 清行回首列。
- **状态重置策略**：候选数变 0/1 → 清；候选数 ≥2 但 prefix 与上次记忆不同 → 视作新一轮，重新响铃。无需引入时间戳：用户两次 TAB 间敲字符必然改 prefix，自然触发 reset，已覆盖 bash readline 的常见近似。

### 边界与容错

- **空 prefix（光秃 TAB）**：会列出全部 builtin + PATH 可执行；数量大也直接全列（题面无分页要求，简化为先；不会崩溃）。
- **PATH 同名**：执行链由 `find_in_path` 按 PATH 顺序解析首个；补全列表去重后名字唯一，二者一致。
- **响铃落点**：`\x07` 写到 stdout（终端 BEL 通常绑 stdout，与 bash readline 一致）。
- **提示符常量同步**：`"$ "` 在 `main.rs::editor.readline` 与 completion 重画处必须字面一致；本 stage 仍以双处字面量保持，注释中标注「修改需同步两处」。

### 性能与日志

- **热路径**：`complete` 在每次 TAB 调用一次，主要成本是候选过滤 + 排序，O(N log N)；候选源 `path_executables` 是启动期一次性扫描的 `Vec<String>`，TAB 时无 IO。
- **日志**：补全路径不打日志（与现有风格一致）；BEL 与列表是用户感知输出，错误（flush 失败）忽略——交互终端无意义反馈。

## 修改目录结构

```
src/
├── completion.rs   # [MODIFY] ShellHelper 增加 last_tab_prefix: Cell<Option<String>>
│                   # 字段；Completer::complete 改造为三态状态机：
│                   # 0 候选 no-op；1 候选直补 + 空格；≥2 候选首次 TAB 响铃、
│                   # 二次 TAB 字母序双空格列出并重画 $ <prefix>。
│                   # 顶部 //! doc 增补「双 TAB 状态机」段落，与现有「候选源 1/2」
│                   # 描述风格保持一致。
├── builtins.rs     # [UNCHANGED] BUILTINS / list_path_executables / find_in_path
│                   # 现状即满足候选源需求，无修改。
└── main.rs         # [UNCHANGED] Editor 默认 Circular 模式即可（complete 返回空
                    # vec 让 rustyline 不触碰 line），提示符 "$ " 字面量保持。
```

## 关键代码结构

```rust
// src/completion.rs
pub struct ShellHelper {
    path_executables: Vec<String>,
    last_tab_prefix: std::cell::Cell<Option<String>>,
}

impl Completer for ShellHelper {
    type Candidate = Pair;
    fn complete(&self, line: &str, pos: usize, _ctx: &Context<'_>)
        -> Result<(usize, Vec<Pair>)>;
    // 内部分支：首词守卫 → 收集 + 排序候选 → match candidates.len()
    //   0       => 清状态，返回 (pos, vec![])
    //   1       => 清状态，返回 (0, vec![Pair{name, "name "}])
    //   _ (>=2) => 比对 last_tab_prefix；首次响铃，二次列出 + 重画
}
```

## 验证方法

- `cargo build` 0 警告，`read_lints` 0 项。
- expect 端到端（PTY 模拟）：
- 题面正本：`/tmp/cc_multi_bin` 放 `xyz_bar/xyz_baz/xyz_quz` 入 PATH → `xyz_\t` 收 `\x07` → 再 `\t` 见 `xyz_bar  xyz_baz  xyz_quz` 与重绘 `$ xyz_`。
- 单候选回归：`ech\t` → `echo `；`custom\t` → `custom_executable `（含上 stage 临时 PATH）。
- 状态重置：`xyz_\t`（响铃） → `b\t`（前缀变 `xyz_b`，仍 ≥2 候选）应再次响铃而非直接列出。
- `codecrafters test` 提交本 stage；失败时按响铃 / 顺序 / 重绘三类切分排查（响铃→检 flush；顺序→检 sort 与 `"  "` 分隔；重绘→引入 `\x1b[2K\r` 清行）。