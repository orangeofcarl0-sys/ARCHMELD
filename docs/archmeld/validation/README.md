# 本初始化包的验证边界

此目录记录的是**规格包本身**的检查，不是ARCHMELD软件构建或产品验收。

## 已执行的检查类型

JSON语法/重复键；Draft2020-12 Schema元校验；有效合成对象；应被拒绝的Schema负例；少量明确的跨字段语义负例；规范化哈希与原始字节向量；样例实体/证据/源码范围关联；任务依赖无环与需求—测试—任务双向映射；来源ID；本地Markdown链接；JSON引用；OpenAPI/MCP的结构和责任映射；状态机一致性；打包manifest摘要。

OpenAPI检查为本包实现的结构/引用/操作映射校验，**未运行完整OpenAPI第三方一致性套件、真实HTTP请求或MCP客户端**。语义负例检查仅覆盖样例列明的规则，不能替代S19的生产权限/事务/类型实现。

## 没有完成或声称完成的检查

没有本地克隆/编译CodeGraph或CodeBoarding；没有构建ARCHMELD；没有Windows/Linux安装测试；没有真实七语言runtime采集；没有执行80项产品验收；没有外部模型调用与性能实测；没有声称包中样例代表任何真实项目。

任务状态保持not_started，产品用例保持not_run。依赖清单中 `needs_local_resolution` 表示待本地解析锁定，不是一个可直接安装的完整工具链lock。上游SHA固定只证明选定源码版本，不证明依赖组合兼容。

## 复验

在Python 3.10+环境中安装 `tooling/requirements-validation.txt` 后，在包根运行：

```text
python tooling/check_pack.py
```

不传--output时只读，不改包、不联网、不执行被分析项目。需要新记录时输出到包外，以免修改包内容导致原manifest校验失败：

```text
python tooling/check_pack.py --output ../ARCHMELD-recheck.json
```

[RESULTS.json](RESULTS.json) 为交付前检查记录。PACK_MANIFEST.json覆盖有效载荷；SHA256SUMS另外覆盖manifest本身。校验和用于损坏检测，不提供发行人身份签名或对恶意整体替换的真实性保证。
