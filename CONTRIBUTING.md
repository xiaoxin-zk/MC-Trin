# 参与贡献

欢迎为 MC-Trin 提交问题报告和改进。提交代码前请先阅读根目录及相关模块的 `AGENTS.md`，并保留所依赖项目的许可证与版权声明。

## 问题与拉取请求

- 新功能或较大改动请先开 Issue 说明使用场景和方案；修复问题时附上复现步骤。
- 拉取请求应聚焦一个目标，说明行为变化、影响范围和验证结果。
- 提交信息使用 Conventional Commits，例如 `fix: 修复 Java 自动发现`。
- 不要在代码、日志或截图中提交访问令牌、账号凭据或个人数据。

## 开发与验证

- 桌面端依赖 Rust、Node.js `24.15.0` 或更高版本，以及项目指定的 pnpm 版本。
- 克隆仓库时使用 `git clone --recurse-submodules`，或随后运行 `git submodule update --init --recursive`。
- Pull request 会由 GitHub Actions 执行代码检查、测试及适用的构建任务；发布构建通过 GitHub Actions 完成。
- 启动本地桌面应用前，请阅读数据隔离说明。开发版本默认可能使用已安装启动器的数据目录。

## 应用数据目录与数据库迁移

桌面开发和测试不得使用或修改日常使用中的启动器数据库。需要运行应用或迁移时，先为开发实例设置独立的 `THESEUS_CONFIG_DIR`；也可使用项目支持的 `AXOLOTL_DATA_DIR_SUFFIX` 隔离目录。确认隔离路径后再启动应用。

数据库迁移一经应用即视为不可变。不要修改、重命名或删除既有迁移；需要修复时新增更高版本的前向迁移，并同步更新 `scripts/axolotl/downgrade-app-db.mjs` 中的可回退结构映射及相应测试。详细要求见 [app-lib 开发说明](packages/app-lib/AGENTS.md)。
