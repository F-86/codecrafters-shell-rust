# codecrafters-shell-rust

[![progress-banner](https://backend.codecrafters.io/progress/shell/42db5ea8-face-4eb3-8ec4-a91493a839f2)](https://app.codecrafters.io/users/codecrafters-bot?r=2qF)

一个用 Rust 实现的 POSIX 兼容交互式 shell，源自
[CodeCrafters「Build Your Own Shell」挑战](https://app.codecrafters.io/courses/shell/overview)。

## 特性

- **9 个内建命令**：`echo` / `exit` / `pwd` / `cd` / `type` / `complete` / `jobs` / `history` / `declare`
- **解析能力**：单/双引号、转义、`$VAR` / `${NAME}` 展开、null word removal
- **重定向（6 种算子）**：`>` / `1>` / `>>` / `1>>` / `2>` / `2>>`，stdout / stderr 正交
- **Pipeline**：N 段 `|` 串联，builtin 支持（echo / pwd / type） + SIGPIPE 自然回收
- **后台作业**（`&`）：非阻塞 `try_wait` reap + prompt 前 Done 通知 + 最小可用编号复用
- **TAB 补全**：命令名 / 参数路径 / `complete -C` 脚本，三套独立状态机
- **历史持久化**：`$HISTFILE` 加载/保存 + `history -r/-w/-a` 文件 IO

## 快速开始

### 前置要求

- Rust **1.95+**（edition 2024）
- Linux / macOS（Unix fd 继承 + 信号语义；Windows 不支持）

### 构建与运行

```sh
# 直接跑
./your_program.sh

# 或显式编译
cargo build --release
./target/release/codecrafters-shell
```

### 跑测试

```sh
cargo test                      # 全部（216 单元 + 16 集成）
cargo test --lib parser         # 仅 parser 单元测试
cargo test --test pipeline_basic
```

### 生成 rustdoc

```sh
cargo doc --no-deps --document-private-items --open
```

## 技术选型一句话

唯一运行时依赖 **`rustyline = "14"`**（readline + history + 补全 trait 框架）；
其它一律手写：状态机 tokenizer、`Rc<RefCell<>>` 单线程共享状态、`try_wait` +
prompt 前 reap 的非阻塞作业回收、`PrevOutput` 三态枚举驱动 pipeline。

## 文档导航

| 文档 | 主题 |
|------|------|
| [docs/ARCHITECTURE.md](./docs/ARCHITECTURE.md) | 分层架构 + 模块依赖图 + 4 条关键时序 |
| [docs/DESIGN_DECISIONS.md](./docs/DESIGN_DECISIONS.md) | 8 个技术选型的五段式记录 |
| [docs/MODULES.md](./docs/MODULES.md) | 逐模块职责 / 公开 API / 依赖 / 不变量 |
| [docs/TESTING.md](./docs/TESTING.md) | 测试组织 / 运行方式 / 新增指引 |

## CodeCrafters 提交

```sh
codecrafters submit
```
