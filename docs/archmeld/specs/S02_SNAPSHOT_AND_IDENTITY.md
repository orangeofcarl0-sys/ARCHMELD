# S02 · 快照、源码位置、身份与失效

## 1. 快照是分析单位

`project_id` 是生成并持久保存的 UUID；`snapshot_id` 为内容 manifest 的 SHA-256。manifest 包括规范化文件清单及内容哈希、被分析构建配置、依赖/工具链身份、排除策略哈希、子模块/生成代码标识。Git commit 可为空，不能独自代表工作区状态。

纳入已跟踪文件和明确允许的未跟踪源码；默认不读密钥、二进制产物、依赖缓存和超限文件。排除项必须计数并列原因。忽略规则不允许静默排除影响目标功能的配置。Git LFS 指针未物化时标成外部缺失，不能把指针文本当模型输入完成分析。

未保存编辑器 buffer 单独保存为 `overlay`，绑定基础 blob 和 editor document version；overlay 可用于临时导航，但不能作为磁盘验证/应用依据。执行重构前需明确保存或丢弃涉及文件的 overlay。

## 2. 一致性捕获

先枚举文件及元数据，读取字节计算 hash，完成后重新检查发生变化的文件和构建输入；并发变化超过重试预算则返回 unstable_workspace。不能将部分旧、部分新内容声称同一个稳定版本。索引器优先读取 materialized snapshot，或通过运行前后文件清单校验保证 live workspace 与 snapshot 一致。

内容身份不包含生成时间和机器本地绝对目录，避免同一内容因路径搬移产生虚假差异；执行环境单独有 `environment_digest`。含秘密的环境变量只记录变量名称和 secret reference，不把秘密值或可逆替代写进 manifest。

## 3. 路径规则

协议内使用仓库相对、POSIX `/` 分隔、大小写保真的路径。拒绝绝对路径、`..`、空字节、URI 混用、Windows ADS 和不在声明根目录内的符号链接/junction。大小写不敏感磁盘上的冲突必须报错；不能把大小写折叠后的不同 Unix 文件静默合并。

Windows、WSL 和容器路径用明确 `path_mapping` 表转换，并记录执行位置。不能直接将 `C:\...` 填进 Linux 的 compile_commands.json；不明确的映射返回 unmapped_source。

## 4. 字节与行列

规范源码范围使用零起始 UTF-8 **字节偏移、左闭右开**；保存原始字节，不改 CRLF、不做 Unicode 规范化。另可缓存显示行列。LSP 位置必须按协商编码转为字节，SCIP 按其文档/索引元数据的编码转换，不假定所有协议都是 UTF-16。变换前验证范围不落在 UTF-8 编码中间。非 UTF-8 文件的行为是显式转码副本及映射，或返回 unsupported_encoding，不猜编码后写坏原文。

## 5. 实体身份

实体身份在 snapshot 内稳定：优先 `semantic provider + symbol + build-config + scope`；仅语法实体用 `file blob + node kind + byte range + structural discriminator`。为外部 API 生成不暴露路径的 entity_id，保存 backend ID 映射。

跨快照使用 `identity_link` 表，种类为 exact、rename_proven、structural_candidate、unresolved。structural_candidate 不参与自动重命名和自动删除判断。同名函数、重载、局部变量、匿名闭包和 generated symbol 分别处理，不能只用 name 查表。

## 6. 失效传播

文件变动使关联语法事实对新快照不再适用；公共接口、配置、依赖和编译参数变动进一步使相关语义 run 失效。模型解释依赖 evidence IDs，因此其支持证据过期时标记 `needs_review`，而非把整库全部删除。

运行 Trace 永久指向其构建/源码快照。用户改代码后默认打开历史只读源码，另提供“映射到当前版本”的候选；不可直接跳当前同一行冒充当时执行代码。

## 7. 验收重点

脏工作区、未跟踪源码、空 Git 仓库、CRLF、中文路径/emoji、同名重载、宏配置切换、文件搬移、符号链接逃逸、未保存 buffer、索引期间编辑均有独立 AT。所有缓存 key 必须包含正确的 project/snapshot/config/provider 维度。
