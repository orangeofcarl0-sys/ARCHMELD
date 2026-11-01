# 合成契约样例

所有文件都是**人为构造的协议样例**，不是CodeGraph、CodeBoarding或ARCHMELD实际运行结果。runtime-run 的成功状态仅演示记录形状；它明确unbound，并无源码运行关联。verification故意为inconclusive/not_run。示例approval已绑定合成digest，不能用于真实项目，也不代表用户已授权。

[索引](index.json) 列明预期有效、Schema应拒绝、Schema有效但语义应拒绝三类。用于contract tests、UI初期fixture和边界设计；R1必须替换为实际执行数据并通过acceptance中的场景。

assets/source-before.py.txt 与source-after.py.txt是小型源码字节样本，保留中文和CRLF；snapshot中的虚拟路径src/example.py对应前者。它们不是产品代码。合成trace不是benchmark。hash-vectors用于跨语言协议一致性，校验器实际核查其中摘要。

示例prepared-manifest仅说明摘要输入，尚无暂存验证，故不能应用；change-set保持draft。真实系统需满足全部guard才会进入prepared/accepted。离线报告样例只是静态占位，不能当作产品可视化演示。
