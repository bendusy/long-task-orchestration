# 部署严格定序 — 执行细节

> 主文件 §4 部署定序的执行细节。这里用中性 `example_service`
> 示例说明顺序；真实项目照自己的数据库、服务和部署脚本替换。

## 一、定序铁律：schema 先于代码，且可反向

新 bin 若依赖新列/新表，**必须先在生产库改完 schema 再部署 bin**，否则 bin 启动即查不到列报错。

示例：
1. 先在生产库 `ALTER TABLE example_items RENAME COLUMN old_count TO new_count`。
2. 再部署「查 `new_count` 这个新列」的服务。
- 顺序反了：旧 schema + 新服务 → 服务查 `new_count` 报「列不存在」。

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

## 四、部署脚本的安全网（至少复刻这两个）

中性 `deploy.sh` 模式（换成你自己的拓扑时，**至少复刻 dry-run + auto-rollback**）：
1. rsync 源码 → <prod-host>。
2. <prod-host> 上运行你的 build 命令（如 `npm run build` / `cargo build --release` / `go build`）。
3. stop 服务 → `cp service service.bak`（备份）→ cp 新服务 → start。
4. **health check**：失败 → 自动回滚到 `service.bak` → `exit 30`。
5. release/main 分支 guard（非允许分支拒绝部署）。
6. 不自动建表（schema 手动 `psql -f`，对齐定序铁律一）。

## 五、端到端真路径实测（不是 health 200）

health 200 只证明进程活着，**不证明新功能生效**。必须跑真实用户路径黑盒验证。

示例验收（三断言）：
1. 通过真实 API 创建对象 A，再创建对象 B 触发新逻辑。
2. DB 只读直查断言：A/B 的状态、计数字段、关联字段符合预期。
3. REST/页面黑盒断言：用户路径仍能查到正确结果。
4. **测完即删**：删除 A/B 和残留测试数据，生产库不留测试垃圾。

## 六、部署后留观察窗

上线 ≠ 完结。留观察窗再推下一个：
- 一个高风险变更上生产先跑 24-48h 观察，确认关键指标稳定，再推下一个。
- 不在一个功能刚上线就立刻堆下一个——给真实流量暴露问题的时间。
