---
name: stage-completion-any-arg-position
overview: 本 stage 要求文件名补全在任意参数位置（而非仅第一个参数）生效。经核对现有 src/completion.rs 实现已天然满足该要求——complete_filename_arg 通过 tokenize().last() 取最后 token，split_dir_and_name 对无 '/' 的 token 退化为扫描 CWD，0/1/≥2 候选三态与双 TAB 状态机均已就绪。故采用「零产品代码改动 + 补 4 条针对性单元测试 + PTY 端到端复现 stage 4 行样例」方案。
todos:
  - id: add-unit-tests
    content: 在 src/completion.rs::tests 追加 4 条针对性单元测试覆盖任意参数位置语义
    status: completed
  - id: cargo-test
    content: 运行 cargo test 验证 89 个测试全过
    status: completed
    dependencies:
      - add-unit-tests
  - id: pty-e2e
    content: "PTY 端到端复现 stage 4 行样例（fixture: foo + bar/）并断言关键字节序列"
    status: completed
    dependencies:
      - cargo-test
  - id: build-clippy-check
    content: 运行 cargo build + cargo clippy 确认零新增警告
    status: completed
    dependencies:
      - pty-e2e
  - id: cleanup
    content: 清理 /tmp 下的临时 fixture 与 PTY 验证脚本
    status: completed
    dependencies:
      - build-clippy-check
---

## 用户需求

将 Tab 补全从「仅首参数」扩展到「任意参数位置」。每个参数都独立地在当前工作目录（CWD）匹配——**前导目录参数（如 `bar/`）不切换搜索目录**。

## 核心特性

- 任意参数位置的 TAB 触发文件名补全：取「最后一个空格之后的文本」作为前缀
- 在 CWD 内匹配，沿用现有规则：LCP 扩展、多匹配双 TAB 列出、目录尾随 `/`、文件尾随空格、无匹配响铃
- 关键不变量：`ls bar/ f<TAB>` 中 `f` 在 **CWD** 匹配，**不**进入 `bar/`

## 现状核对结论

经核对 `src/completion.rs` 全文，`Completer::complete` 入口对"光标左侧子串含任意空白"即转入参数分支，与"第几个参数"完全无关；`extract_arg_prefix` 通过 `tokenize().last()` 天然取最后 token；`split_dir_and_name("f") = ("", "f")` 天然扫 CWD。Stage 给出的全部 4 行样例可在零产品代码改动下通过。

## 交付物

- 在 `src/completion.rs::tests` 追加 4 条针对性单元测试，将"任意参数位置 + CWD 不切换"两个核心断言固化进 CI
- PTY 端到端复现 stage 完整 4 行样例
- 全套 `cargo build` / `cargo test` 通过；不引入新 clippy 警告

## 实施策略

**零产品代码改动，仅补单元测试 + PTY 端到端验证**。理由：

1. **入口判定与参数位置无关**：`complete()` 仅看 `line[..pos]` 是否含空白，命中即转参数分支（`src/completion.rs:240-244`）。
2. **最后 token 提取已正确**：`extract_arg_prefix` 用 `tokenize().into_iter().last()` 取最后 token，对 `"ls bar/ foo x"` 返回 `"x"`，对 `"ls bar/ f"` 返回 `"f"`（`src/completion.rs:363-370`）。
3. **CWD 隔离已正确**：`split_dir_and_name("f") = ("", "f")` → 扫描 `Path::new(".")`，**不会**因前面有 `bar/` 切到 `bar/`（`src/completion.rs:386-391`、`140-146`）。
4. **三态分支已就位**：0 候选 BEL、1 候选目录尾 `/` / 文件尾空格、≥2 候选 LCP+双 TAB（`src/completion.rs:152-224`）。
5. **状态机 key=(dir_part, name_prefix)**：跨参数位置复用零隐患。

## stage 4 行样例的执行轨迹（验证）

| 输入 | prefix | (dir, name) | scan_dir | 候选 | 行为 |
| --- | --- | --- | --- | --- | --- |
| `ls <TAB><TAB>` | `""` | `("", "")` | CWD | `[bar, foo]` | 首次 BEL，二次列 `bar/  foo` |
| `ls b<TAB>` | `"b"` | `("", "b")` | CWD | `[bar]` | 补 `bar/`（目录，无尾空格） |
| `ls bar/ f<TAB>` | `"f"` | `("", "f")` | **CWD**（非 `bar/`） | `[foo]` | 补 `foo `（文件，尾空格） |
| `ls bar/ foo x<TAB>` | `"x"` | `("", "x")` | CWD | `[]` | BEL，line 不变 |


## 实施要点

- **测试位置**：追加到 `src/completion.rs::tests` 模块尾部，复用现有 `extract_arg_prefix` / `split_dir_and_name` 导入路径。
- **零产品代码改动原则**：不动 `complete_filename_arg`、不动状态机、不动 `tokenize`。stage 通过靠的是上 stage 设计的前瞻性。
- **PTY 验证**：临时目录建 `foo` 文件 + `bar/` 目录，用 Python `pty` 模块 spawn `./target/debug/codecrafters-shell`，逐次发送 stage 样例的 TAB 序列，断言 stdout 中的关键字节序列。
- **避免污染当前工作目录**：fixture 建在 `/tmp/shell_arg_pos_test/`，验证完整目录 `rm -rf`。
- **clippy 现状**：HEAD 已有 2 个 pre-existing warning（`map_or` / parser 文档缩进），本次不引入新警告即可，不处理 pre-existing。

## 目录结构

```
project-root/
└── src/
    └── completion.rs   # [MODIFY] 仅在 tests 模块尾部追加 4 条单元测试：
                        #   1) extract_prefix_at_third_arg     —— 验证多参数尾 token 提取
                        #   2) extract_prefix_after_dir_arg    —— 验证目录参数后空白返回空 prefix
                        #   3) extract_prefix_subsequent_with_prefix —— 验证前导目录参数不影响最后 token
                        #   4) split_dir_and_name_isolated_arg —— 显式锁定『裸名 → 扫 CWD』语义
                        # 产品代码（mod.rs 顶部 doc、ShellHelper 实现、complete_filename_arg）保持不变。
```