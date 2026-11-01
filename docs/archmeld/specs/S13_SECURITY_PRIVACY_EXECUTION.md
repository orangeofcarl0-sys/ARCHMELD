# S13 · 信任、执行隔离、隐私与供应链

## 1. 威胁模型

不可信来源包括仓库代码/配置/注释、依赖安装脚本、构建插件、语言服务器插件、Trace/日志、模型输出和下载的工具。它们可能诱导执行命令、越界读取、外传源码、修改测试或污染证据。打开文件不代表授权执行。[SRC-VSC-TRUST]

初始威胁模型防止常规路径/授权绕过、提示注入导致越权及故障污染；不声称抵御拥有同用户完整 OS 权限的恶意本机进程。边界必须在 UI/报告中如实呈现。

## 2. 四种执行状态

`read_only`：只读解析，不运行项目配置；`isolated`：受 OS/容器隔离的场景执行；`trusted_native`：明确批准在本机原生工具链运行，权限不比用户更强但不可声称沙箱；`external_side_effects`：额外授权真实硬件/服务/数据库。

Windows Job Object 可用于资源与进程树管理，不等于文件/网络安全沙箱。原生 MSVC 工程如需未隔离运行，必须提示 trusted_native。OCI/WSL 容器可用于兼容任务，但不能假称能运行所有 Windows 项目。能力不足时 fail closed，不回退成未声明的裸执行。

## 3. 权限粒度

分别授予 read_source、index、execute_scenario、write_staging、apply_changeset、network_provider、download_tool、export_source、external_resource。授权绑定项目与任务，期限和 scope 可撤销。模型不能创造授权，MCP annotation 不能授予权限。

命令以 argv 数组执行，不默认 shell 拼接。工作目录/输入输出根目录受限制；PATH 和环境使用 allowlist，工具优先 lockfile 绝对路径，不解析仓库中的同名伪 executable。

## 4. 核心与 Webview

loopback 服务采用随机 session token、短期 client scopes、Origin/Host 校验、请求上限和幂等。扩展 Host 保管 token，Webview 走受限 message bridge；不把任意 postMessage 映射成 shell/HTTP。CSP 默认禁止远程脚本，所有源码/日志 HTML 转义。

本地 HTTP 不是多租户安全边界；token 文件使用最小用户 ACL/权限。Unix socket/Windows named pipe 可作为后续同协议传输增强，不影响 R1 的授权模型。

## 5. 数据出站

默认无遥测、无自动模型调用、无自动下载。明确区分在线依赖准备与运行时源码出站。模型 provider 获得最小 ContextPack，可预览/脱敏；大型原始 Trace 默认不发送。报告不包含密钥、HOME、私密服务地址或原始变量 payload，必要时用户显式选择导出。

日志只记录脱敏 argv 和 secret reference；不能以“debug 模式”绕过。保留隐私选择而不是静默修改；脱敏导致证据不足时显示 redacted/partial。

## 6. 供应链

安装工具先核对 origin、固定 commit/version、SHA-256、许可证与平台。不得在 package postinstall 自动下载未锁模型；不得默认执行用户仓库 package manager install scripts。构建系统运行本身可能执行代码，受对应信任级别控制。

第三方许可证按具体模块/二进制/模型检查。CodeBoarding 引擎许可不推及所有编辑器和服务；SCIP 不推及整套 Sourcegraph；开放源码不自动意味着所有商业使用无限制。法律结论以实际锁定文件审查为准。

## 7. 强制安全用例

路径穿越、symlink/junction、大小写冲突、恶意 Trace HTML、超大压缩输入、仓库命令冒名、未经授权网络、提示注入请求出站、变更越界、篡改契约、失效授权、重复 apply、被外部修改的回滚文件均需真实测试。未支持的隔离模式不得显示 secure。
