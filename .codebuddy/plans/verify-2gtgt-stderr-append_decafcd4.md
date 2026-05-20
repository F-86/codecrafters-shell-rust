---
name: verify-2gtgt-stderr-append
overview: 本 stage 的 `2>>` stderr 追加功能在上一 stage 已顺手实现完毕、单测全绿；本计划专注「端到端验证 spec 4 条 tester 样例 + 补 1-2 个 split-stream 针对性回归测试 + 最小化文档注释补强」，预期零功能代码改动。
todos:
  - id: add-split-stream-regression-test
    content: 在 src/parser.rs tests 模块尾部追加 parse_stdout_append_with_stderr_inherit_split_stream 单测，锁定 `>>` 仅影响 stdout、stderr 保持 inherit 的解析语义
    status: completed
  - id: enrich-doc-comments-orthogonality
    content: 在 src/parser.rs 头注释段落与 ParsedCommand.stderr_append 字段 doc 补「stdout/stderr 重定向正交」说明，方便后续 stage 溯源
    status: completed
  - id: cargo-test-and-release-build
    content: 运行 cargo test 验证 63/63 全绿且零回归，cargo build --release 零警告
    status: completed
    dependencies:
      - add-split-stream-regression-test
      - enrich-doc-comments-orthogonality
  - id: e2e-replay-spec-six-samples
    content: 脚本回放 spec 6 条样例至 /tmp/foo，分别断言 baz.md 空+终端可见 stderr、qux.md 单行、echo 走 stdout+quz.md 创建、连续 2>> 累计两行；结束清理临时文件
    status: completed
    dependencies:
      - cargo-test-and-release-build
---

## 用户原始需求

codecrafters 阶段任务：实现 `2>>` 操作符，将命令的标准错误**追加**到文件（不存在则创建、存在则保留原有内容并追加）。Tester 会发送 6 条命令验证：

1. `ls nonexistent >> /tmp/foo/baz.md` — stderr 仍要在**终端**可见（`>>` 只重定向 stdout）。
2. `ls nonexistent 2>> /tmp/foo/qux.md` — 错误写入文件，终端无错误。
3. `echo James says Error 2>> /tmp/foo/quz.md` — stdout 终端可见，quz.md 被创建（即使无 stderr 写入）。
4-5. 连续两条 `... 2>> /tmp/foo/quz.md` — 文件累计两行错误（追加而非覆盖）。

## 范围确认（B 方案）

**零功能代码改动**：上一 stage 已顺手实现 `2>>` 全链路（tokenize 合并、`ParsedCommand.stderr_append`、parse 路径、`open_err_sink(.., append)`、外部命令分支 `open_file_for_redirect(.., parsed.stderr_append)`、`stderr_redirect=None → Stdio::inherit()`），且 ≥3 个相关单测已绿。本 stage 仅做验证 + 轻量补强。

## 核心交付物

- **端到端验证**：脚本驱动回放 spec 6 条样例，逐项断言文件内容、文件创建、终端可见性。
- **补 1 个针对性单测**：锁定 split-stream 语义（`>>` 仅作用于 stdout、stderr 保持 inherit）—— spec 第 1 条样例对应的解析结果。
- **2 处文档注释补强**：在 `src/parser.rs` 头注释和 `ParsedCommand.stderr_append` 字段 doc 显式化「stdout / stderr 重定向正交」语义，方便后续 stage 溯源。
- **回归保证**：`cargo test` 63/63 绿、`cargo build --release` 零警告。

## 技术栈

- 语言：Rust（edition 沿用当前项目）。
- 测试：`cargo test`（内嵌 `#[cfg(test)] mod tests`）。
- e2e：bash heredoc 驱动 `./target/release/codecrafters-shell`，分别 capture 进程 stdout / stderr 与目标文件。

## 实施策略

### 1. 现状复盘（已确认，无需再读源码）

| 层 | 位置 | 现状 |
| --- | --- | --- |
| tokenize | `src/parser.rs` `'>' =>` 分支 | `chars.clone().next()` peek 第二个 `>`，按 `current == "2"` 合并为 `"2>>"` token |
| parse | `src/parser.rs` parse 循环 `match tok.as_str()` | `"2>>"` 分支写 `stderr_redirect` 并置 `stderr_append = true` |
| ParsedCommand | `src/parser.rs` ~line 228 | 已含 `stderr_redirect: Option<String>` + `stderr_append: bool` |
| sink 打开 | `src/main.rs` `open_err_sink` | `(Option<&str>, append: bool)`，append=true 走 `OpenOptions::new().create(true).append(true).open(path)` |
| REPL 主循环 | `src/main.rs` line 189 | 调用 `open_err_sink(.., parsed.stderr_append)` |
| 外部命令分支 | `src/main.rs` lines 282-291 | `open_file_for_redirect(path, parsed.stderr_append)`；`stderr_redirect=None → Stdio::inherit()`（即终端可见） |
| 单测 | `src/parser.rs` | 已含 `parse_append_stderr_sets_flag`、`parse_mixed_truncate_and_append_takes_last`（stderr 分支）、`parse_stdout_append_and_stderr_append_coexist`、`redirect_append_with_digit_prefix_merges`（`2>>` tokenize） |


### 2. 实施细节（轻量、零功能改动）

**步骤 A：补 1 个 parse-level split-stream 单测**

`src/parser.rs` tests 模块尾部追加 `parse_stdout_append_with_stderr_inherit_split_stream`：

- 断言 `parse("ls nonexistent >> /tmp/foo/baz.md")` 结果四字段：`stdout_redirect=Some("/tmp/foo/baz.md")`、`stdout_append=true`、`stderr_redirect=None`、`stderr_append=false`。
- 目的：spec 第 1 条样例对应的解析结果——未来若有人误把 `>>` 误归类为「同时影响 stderr」，单测立刻报错。

**步骤 B：2 处文档注释补强**

- `src/parser.rs` 头注释（`parse` 函数说明段落，~lines 18-25）：在「识别 6 类重定向操作符」末段补一句，显式声明 **stdout 与 stderr 重定向正交**（互不影响、各自有独立的 redirect+append 字段）。
- `src/parser.rs` `ParsedCommand.stderr_append` 字段 doc：补一句「与 `stdout_redirect` 完全正交，可单独追加 stderr 同时保留 stdout 终端输出（如 `cmd 2>> err`）」。

**步骤 C：端到端验证 spec 6 条样例**

`cargo build --release` 后用 bash heredoc 一次回放：

```
ls nonexistent >> /tmp/foo/baz.md
ls nonexistent 2>> /tmp/foo/qux.md
echo James says Error 2>> /tmp/foo/quz.md
cat nonexistent 2>> /tmp/foo/quz.md
ls nonexistent 2>> /tmp/foo/quz.md
exit 0
```

分别捕获 shell 进程的 stdout / stderr 到两个文件，断言：

1. `baz.md` 为空，shell stderr 含 `ls: nonexistent:` 行（终端可见）。
2. `qux.md` 单行错误内容，shell stderr 不含该行。
3. shell stdout 含 `James says Error`，`quz.md` 被创建。
4-5. `quz.md` 累计两行错误（追加未覆盖）。

最后清理 `/tmp/foo` 与临时日志文件。

### 3. 性能 / 可靠性 / 风险

- **零功能代码改动**：仅追加测试与注释，blast radius 最小。
- **测试新增 1 个**：62 → 63，无既有用例修改，零回归风险。
- **e2e 脚本副作用**：仅在 `/tmp/foo`、`/tmp/stderr.log` 等临时路径写文件，结束统一 `rm -rf` 清理。
- **`cargo build --release` 增量构建**：源码若仅改测试模块，主二进制不重链；若改 `ParsedCommand` doc 注释，rustdoc 不影响二进制（doc comment 不改变 ABI 与代码生成）。

## 关键修改清单

```
src/parser.rs  # [MODIFY] 仅文档注释 + 新增 1 个 #[test]
  - lines 3-25 头注释段落：补「stdout / stderr 重定向正交」声明
  - ParsedCommand.stderr_append 字段 doc：补正交性说明
  - tests 模块尾部：追加 parse_stdout_append_with_stderr_inherit_split_stream
src/main.rs    # 不修改（现有实现已完全满足 spec）
```