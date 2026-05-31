# 部署严格定序 — 执行细节

> 主文件 §5（插槽3：部署安全网）的执行细节。核心定序原则在主文件，这里以一个「单机 PostgreSQL 服务 + rsync 部署」的中性示例项目为载体，用户照着改成自己项目的拓扑。

## 一、定序铁律：schema 先于代码，且可反向

新 bin 若依赖新列/新表，**必须先在生产库改完 schema 再部署 bin**，否则 bin 启动即查不到列报错。

实证：
1. 先在生产库 `ALTER TABLE <你的表> RENAME COLUMN <旧列名> TO <新列名>`。
2. 再部署「查新列名」的 bin。
- 顺序反了：旧 schema + 新 bin → bin 查新列名报「列不存在」。

**可反向**：每个 schema 变更都要能写出 down-migration（RENAME 反过来、ADD COLUMN 对应 DROP）。不可逆的变更（DROP COLUMN 带数据）要先备份。

## 二、SQL 上生产的踩坑（ssh 嵌套引号地狱）

**坑**：在 ssh 命令行里嵌 SQL 字面量，引号/`0x27` 会乱码，psql 报「trailing junk after numeric literal」。踩过 3+ 次。

**正解**：SQL 写成文件 → scp 上去 → `psql -f`。不在 ssh 命令行嵌 SQL 字面量。

```bash
# ❌ 别这样
ssh <prod-host> "psql -c \"UPDATE ... WHERE x='value'\""   # 引号地狱

# ✅ 这样
echo "UPDATE ... WHERE x='value';" > /tmp/m.sql
scp /tmp/m.sql <prod-host>:/tmp/m.sql
ssh <prod-host> "psql -f /tmp/m.sql"
```

## 三、只读探针先行（数据不外流）

碰生产第一步是**只读探活**，不是写：
- 查服务 pid / health / 监听端口（`ss -tlnp`）。
- 查 schema 是否就位（`information_schema.columns`）。
- **不打印密码**（探 DB 配置时 `sed` 掉密码字段）。
- **数据不外流**：只取聚合统计（count/avg/分布），绝不把生产记录原文拉到开发机。

## 四、部署安全网（至少复刻这两个）

以下是示例项目部署脚本的参考模式（用户换自己拓扑时，**至少复刻 dry-run + auto-rollback**）：
1. rsync 源码 → <prod-host>。
2. <prod-host> 上执行构建命令（注意项目自己的 feature flag）。
3. stop 服务 → `cp bin bin.bak`（备份）→ cp 新 bin → start。
4. **health check**：失败 → 自动回滚到 bin.bak → `exit 30`。
5. master 分支 guard（非 master 拒绝部署）。
6. 不自动建表（schema 手动 `psql -f`，对齐定序铁律一）。

## 五、端到端真路径实测（不是 health 200）

health 200 只证明进程活着，**不证明新功能生效**。必须跑真实用户路径黑盒验证。

端到端三断言模板（按项目自身领域替换）：
1. 触发写入路径 A → 触发条件升级 → 写入 B 并声明取代 A（带「取代」外键或等效标记）。
2. DB 直查断言：A 的状态字段/外键已正确更新（降级标记✓）+ 计数字段变化正确✓。
3. 接口断言：A 仍可召回（软删语义生效，未被硬删）✓。
4. **测完即删**：清理测试数据，生产库不留测试垃圾。

## 六、部署后留观察窗

上线 ≠ 完结。留观察窗再推下一个：
- 第一个功能上生产先跑 48h 观察，稳定后再推下一个。
- 不在一个功能刚上线就立刻堆下一个——给真实流量暴露问题的时间。
