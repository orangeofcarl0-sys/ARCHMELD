# S14 · 持久化、备份、恢复与日常运维

## 1. 存储分层

`project.db` 保存事务化实体和状态；`objects/sha256/` 保存不可变内容；`cache/adapter/version/` 保存可重建索引；`runs/` 保存受管理任务目录；`exports/` 保存用户明确导出。按项目 namespace 隔离，不复用原 CodeGraph 全局数据库。

默认系统数据目录：Windows 应用数据目录、Linux XDG 路径；位置可配置。产品不得把初始 ZIP 中的 synthetic examples 当持久项目。实例持有 lock/owner token 防止两个 core 同时写同一数据库；同一项目多个 UI 客户端共享一个 core。

## 2. Durability 与迁移

对象先写临时文件、计算 hash、flush，再原子 rename 到 hash 路径；数据库事务只引用已可读取对象。崩溃产生的孤儿对象可稍后 GC，数据库不能引用缺失对象。apply journal 写入和文件替换顺序见 S09，不能只靠 SQLite 事务保护磁盘多文件变化。

Schema migration 必须有版本、前置检查、备份和回滚/恢复路径。升级前把所有活动 change 标为暂停，不能迁移一半还允许 apply。备份用一致性数据库快照加可达对象清单，不在 WAL 活跃时简单复制一个 db 文件就称完整备份。

## 3. 保留与 GC

建议默认保留最近 30 个未固定分析快照，批准计划、验收、运行证据和用户 pin 的对象不因数量阈值删除；这些是可调整默认政策，不是数据规模硬上限。GC 是标记—清扫，有 dry-run 清单与 owned-root 检查。正在导出的对象 pin 到导出完成。

Expedition 完成后的自动清理先确认导出完整且用户策略允许。任务中断的 session 保留到恢复/放弃决定；不在系统启动时无条件清空所有缓存目录。

## 4. 恢复与重启

启动先检查 apply journal、未完成 DB migration 与运行任务。发现 applying 状态应进入恢复界面并锁住新的 apply，不默认继续或判成功。外部进程只在 owner/start token 匹配时接管/终止，避免 PID 重用。

离线项目可直接重开历史分析；工具缺失只影响新增分析。语言缓存损坏可重建而不丢人工架构和决策。对象缺失报告 affected IDs 与可恢复来源，不静默删掉历史。

## 5. Manager 演化

每次提交/文件更改产生 drift 摘要：哪些证据过期、哪些约束被破坏、哪些架构 membership 需复核。只把有明确证据的变化放在关键告警；批量格式化不应把所有职责自动判成变化。

用户手工维护的职责与约束标 declared，系统不得覆盖；可提出修订候选。一次性导入外部报告保留 producer/version 与输入身份，默认不授予其执行权限。

## 6. 诊断与支持包

`doctor` 输出版本、平台、工具路径、索引能力、信任/网络状态、可用空间与配置冲突。支持包默认不含源码、Trace payload、HOME 或 token；包含脱敏环境摘要与任务诊断。不能自动上传，导出前可预览内容。
