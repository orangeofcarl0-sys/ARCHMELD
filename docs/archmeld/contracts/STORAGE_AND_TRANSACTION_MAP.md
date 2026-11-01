# SQLite与对象库实现映射

这是逻辑schema/约束，不是已经部署的SQL。B003生成迁移，使用SQLite外键、WAL、单写队列，运行真实崩溃测试。JSON payload采用版本化Schema；查询高频字段单独列，不为方便把全部图只存一个大JSON。

| 表 | 主键/唯一性 | 关键列、索引与引用 |
|---|---|---|
| projects | project_id | mode,revision,current_snapshot_id,config_digest,root_mapping,state |
| snapshots | snapshot_id | project_id,config/environment/exclusions_digest,manifest_artifact,stable,created_at；索引project+created_at |
| files | snapshot_id,path | blob_digest,size,role,origin,executable；FK snapshot、artifact |
| generations | project_id,snapshot_id,adapter_id,run_id | status staging/published/failed,final_seq,coverage；published完整scope不能被半批覆盖 |
| entities | project_id,snapshot_id,entity_id | config,kind,language,qualified_name,payload；索引name和kind |
| backend_mappings | project,snapshot,adapter,run,backend_id | entity_id；非全局唯一ID |
| entity_mappings | project,before_snapshot,after_snapshot,mapping_id | before/after实体、basis、evidence、候选冲突；跨版本映射不静默强合并 |
| evidence | project,snapshot,evidence_id | kind,producer,artifact_digest,coverage,freshness；原件不可变 |
| relations | project,snapshot,relation_id | source_id,target_id,kind,basis,resolution,config；双向邻接索引 |
| relation_evidence | project,snapshot,relation_id,evidence_id | 复合FK强制同快照；runtime跨记录引用保留其原来源 |
| claims / claim_support | project,claim_id,revision | snapshot,status,text,producer；support/counterevidence单独关联 |
| architecture_models / memberships | project,model_id,revision | layer,status,snapshot,model_artifact；组件/成员可多归属 |
| scenarios / runtime_runs | project,id | scenario版本/输入/runner；run的build/snapshot/binding/clock/raw artifact |
| contracts / plans | project,id,revision | contract_digest/frozen，plan_scope manifest digest/status |
| changesets | project,id | revision,base/result_snapshot,plan/step,contract_digest,state,prepared_digest |
| change_operations | project,change_id,sequence | op,path,destination,pre/post/content；记录每步应用状态 |
| approvals | project,id | target_kind,target_id,target_digest,actor,decision,capabilities,expiry；不原地改审批历史 |
| verification_runs / check_results | project,verification_id[,check_id] | exact snapshot,contract,toolchain,start/end,verdict；check unique |
| artifacts / artifact_refs | digest / owner_type,owner_id,digest | size,type,path,classification；GC按refs和活跃任务根遍历 |
| jobs / tool_runs | project,id | operation,state,revision,runner_digest,limits,exit,logs；stdout/stderr原件 |
| events | project,sequence | event_id唯一,type,payload,object refs；同业务写outbox事务 |
| idempotency | project,operation,key | request_digest,state,result；相同键异内容拒绝 |
| writer_leases | project | owner,fencing_token,expiry；fencing递增防旧持有者写入 |

## 原子边界

导入：批次落staging与临时对象 → 校验全量来源/引用/coverage → 一事务发布generation和事件。对象先落稳定存储再引用；提交失败未引用对象后续GC。文件系统和DB不是同一事务，启动doctor检测孤儿/缺失，绝不能当正常完整。

应用：DB创建prepared与preimages → 审批登记 → 持久journal标apply intent → 每文件hash/link重查与替换、逐步记录 → 实际快照 → required验证 → acceptance。进程终止后根据journal检测中间状态，不能仅依据DB状态声称文件全部更新。

SQLite事务不执行长时间外部编译/模型请求，不持数据库锁等待Agent。source/Trace大对象不塞BLOB反复复制。backup走SQLite一致快照+引用对象manifest，不能在WAL工作中只复制主db文件。

FTS和后端RocksDB是可重建派生缓存，不影响核心证据权威。多版本数据访问受project+snapshot限定；即使entity_id一样也不能跨快照拼接。
