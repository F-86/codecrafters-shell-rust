---
name: filename-completion-arg-position
overview: 在现有 rustyline 基于的 `ShellHelper::complete` 中扩展参数位置的文件名补全：当光标左侧已包含空白（即不在首词）时，提取最后一个 token 作为前缀，扫描当前工作目录寻找以该前缀开头的条目，单匹配则补全为 `<name> `（含尾空格），其他情况（0/≥2）一律 BEL 响铃 no-op。
todos:
  - id: extract-prefix-helper
    content: 在 src/completion.rs 新增 extract_arg_prefix 纯函数，复用 parser::tokenize 提取参数前缀，并添加单元测试覆盖普通/尾空白/tokenize 错误三类用例
    status: completed
  - id: match-files-helper
    content: 在 src/completion.rs 新增 match_files_in_cwd 函数，使用 std::fs::read_dir(".") 按字面前缀过滤候选（含隐藏文件），I/O 错误返回空 Vec
    status: completed
  - id: wire-arg-completion
    content: 在 ShellHelper 上新增 complete_filename_arg 方法，按候选数 0/1/≥2 实现 BEL / 替换+尾空格 / BEL 三态返回，使用 pos-prefix.len() 作为替换起点并校验 line 字面对齐
    status: completed
    dependencies:
      - extract-prefix-helper
      - match_files-helper
  - id: dispatch-in-complete
    content: 修改 Completer::complete 早返分支，将含空白时的 no-op 替换为 self.complete_filename_arg(line, pos) 分发，保留命令名分支零改动
    status: completed
    dependencies:
      - wire-arg-completion
  - id: verify-build-and-stage
    content: 本地 cargo build / cargo test 验证零回归，并按 codecrafters 测试样例（cat re<TAB> → cat readme.txt 含尾空格）手测 single-match 行为
    status: completed
    dependencies:
      - dispatch-in-complete
---

## 用户需求

为基于 rustyline 的 Rust shell 增加「参数位置」按 TAB 补全文件名的能力。

## 核心功能

- **触发位置**：当 `line[..pos]` 已含至少一个空白（即光标已离开命令名区进入参数区）时，按 TAB 触发文件名补全；不影响现有的命令名补全分支。
- **前缀提取**：复用现有 parser 的 `tokenize`，对 `line[..pos]` 词法切分；当且仅当 `line[..pos]` 末尾不是空白时，将最后一个 token 视为待补全的文件名前缀。
- **匹配范围**：仅扫描当前工作目录（`std::fs::read_dir(".")`），按 `file_name().starts_with(prefix)` 过滤；包含隐藏文件（前缀以 `.` 开头时也参与匹配）；不递归子目录。
- **行为**：
- 单匹配：把 `[pos - prefix.len(), pos)` 区间替换为 `<filename> `（含尾空格）。
- 无匹配 / 多匹配：BEL 响铃（`\x07`），line 不变。
- tokenize 报错（未闭合引号等）/ read_dir 失败 / 前缀为空：均 no-op（不响铃，避免噪音）。
- **任意命令名生效**：包括不存在的命令（如 `xyz read<TAB>`）。

## 兼容性

- 现有命令名补全（首词位置、双 TAB 状态机、LCP 扩展）行为零变更。
- 参数位置补全独立于 `last_tab_prefix` 双 TAB 状态机，不读不写该字段。

## 技术栈

沿用现有项目栈：Rust 2024 edition + rustyline 14（自定义 `Helper` 提供 `Completer`）+ `std::fs` + 既有 `crate::parser::tokenize`。无新依赖。

## 实现方案

### 触发分发

将 `complete()` 入口现有的「含空白即 no-op」早返分支改为「含空白即分发到新私有方法 `complete_filename_arg`」。命令名分支整段代码不动，确保零回归。

### 前缀提取（关键正确性点）

1. 调用 `tokenize(&line[..pos])`：

- `Err(_)` → 返回 `(pos, vec![])` no-op（未闭合引号场景留给后续 stage）。
- `Ok(tokens)`：检查 `line[..pos]` 末尾字符是否为空白：
    - 末尾是空白 → 用户刚结束一个 token 就按 TAB，prefix 为空 → 本 stage 直接 no-op（不"列出全部"）。
    - 末尾非空白 → `tokens.last()` 即用户正在键入的 prefix；若 `tokens` 为空（理论不应发生，防御性）也按 no-op。

2. 由于本 stage 测试场景不含引号/转义，prefix 与 `line` 中字面子串等长；用 `pos - prefix.len()` 作为 rustyline `Pair` 替换起点。

> 注：tokenize 对引号会"剥离"，使 prefix 字符串长度可能小于其在 line 中的字面长度，导致替换起点错位。为本 stage 鲁棒性，在 prefix 提取后追加一道校验：`line[pos - prefix.len()..pos] == prefix`，不一致则 no-op，留给后续 stage 处理引号场景。

### 目录扫描

- `std::fs::read_dir(".")` 一次性遍历 cwd。
- 对每个 entry：取 `file_name().to_string_lossy()`；以 `prefix` 字面前缀匹配；不区分 file/dir（本 stage 测试只放普通文件，统一作为候选；目录尾随 `/` 留待后续 stage）。
- 隐藏文件天然纳入：不做 `.` 开头跳过（与用户选择一致）。
- I/O 失败（read_dir / DirEntry）→ no-op，REPL 不中断。
- 复杂度：O(N)，N 为 cwd 条目数；TAB 是低频交互，无需缓存。

### 替换语义

- 单匹配：返回 `Ok((start, vec![Pair { display: name.clone(), replacement: format!("{} ", name) }]))`，`start = pos - prefix.len()`。rustyline 会把 `[start, pos)` 替换为 `replacement`，光标停于尾空格之后。
- 0 / ≥2 匹配：`print!("\x07"); io::stdout().flush();` 然后 `Ok((pos, vec![]))`。

### 状态机隔离

`complete_filename_arg` 不读不写 `self.last_tab_prefix`。命令名分支双 TAB 状态机的"前缀变化即重置"语义对参数补全不适用，且本 stage 不需要"列出全部"。

## 实现注意事项

- **零回归**：仅替换 `complete()` 顶部那个早返分支的 4 行；命令名路径（含 LCP / 双 TAB / 状态机清理）完全不动。
- **rustyline 协议**：`(start, vec![pair])` 中 `start` 必须等于"被替换区间的左端"；start 错位会导致用户已键入的字符被吃掉或重复。本方案用 `pos - prefix.len()` 并以"line 字面 == prefix"校验保证一致。
- **UTF-8 安全**：rustyline 给的 `pos` 是字节偏移，`prefix.len()` 也是字节长度，一致；`line[..pos]` 切片由 rustyline 保证在 char 边界。
- **无日志**：补全是纯交互路径，I/O 错误静默吞掉（no-op）即可，避免在用户输入区写错误干扰显示。
- **隐藏文件**：read_dir 不会自动过滤 `.` 开头条目；不主动跳过即天然支持隐藏文件匹配。
- **测试可分层**：prefix 提取是纯函数，可单测；read_dir 部分用 `tempfile` crate 或直接复用集成测试 / 手测，避免引入新依赖（不加 `tempfile`，改为对 prefix-extract 与候选过滤分别单测，I/O 部分由 codecrafters 测试器覆盖）。

## 目录结构

```
src/
└── completion.rs   # [MODIFY] 见下
```

### `src/completion.rs` 改动清单

- **新增 import**：`use crate::parser::tokenize;`、`use std::fs;`。
- **修改 `Completer::complete`**：将 `if prefix.chars().any(|c| c.is_whitespace()) { return Ok((pos, Vec::new())); }` 改为 `if prefix.chars().any(|c| c.is_whitespace()) { return self.complete_filename_arg(line, pos); }`。其余命令名分支代码原样保留。
- **新增私有方法 `ShellHelper::complete_filename_arg(&self, line: &str, pos: usize) -> Result<(usize, Vec<Pair>)>`**：执行"提取 prefix → 校验 line 字面对齐 → 扫描 cwd → 按候选数三态（0/1/≥2）返回"的完整逻辑；BEL 输出复用现有 `print!("\x07") + io::stdout().flush()` 模式。
- **新增辅助纯函数 `extract_arg_prefix(line_to_pos: &str) -> Option<String>`**：封装"调用 tokenize → 末尾空白检查 → 取末尾 token"，便于单测；返回 `None` 表示该路径下应 no-op。
- **新增辅助纯函数 `match_files_in_cwd(prefix: &str) -> Vec<String>`**：封装 read_dir 遍历与前缀过滤；I/O 失败返回空 Vec。
- **新增单元测试**：
- `extract_arg_prefix` 用例：`"cat re"` → `Some("re")`；`"cat re "` → `None`（末尾空白）；`"cat"` → `None`（无空白，不属于参数分支）；`"cat 'unclosed"` → `None`（tokenize 错误）。
- 不再为 `match_files_in_cwd` 写依赖文件系统的单测，避免引入 tempfile；由 codecrafters 测试器作为集成测试覆盖。