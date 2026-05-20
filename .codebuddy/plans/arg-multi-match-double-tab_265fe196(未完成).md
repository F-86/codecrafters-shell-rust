---
name: arg-multi-match-double-tab
overview: 在 `complete_filename_arg` 中复刻命令名分支的双 TAB 状态机：首次 TAB BEL；第二次 TAB 换行列出排序候选（目录加 `/`、文件无尾字符），再重画 `$ <line>`；并顺手加上 LCP 扩展。状态 key 用 `(dir_part, name_prefix)` 对，目录判定用 `classify_path` 逐项 stat。
---

