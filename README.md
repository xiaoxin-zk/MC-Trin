# MC-Trin

MC-Trin 是一款面向 Minecraft Java Edition 玩家、以简体中文用户为重点的 Windows、macOS 与 Linux 桌面启动器。项目使用 Vue、Rust 和 Tauri 构建，提供实例管理、内容下载、Java 管理、多人联机、AI 助手和本地插件等功能。

## 功能

- **账号与启动**：支持 Microsoft 登录、兼容 Yggdrasil 的认证服务器和离线账号；创建、导入、配置并启动 Minecraft 实例。
- **实例与游戏内容**：管理实例中的模组及其他内容，查看依赖和更新；浏览 Modrinth、CurseForge、MCArchive、Planet Minecraft 等来源。CurseForge 功能需要当前构建提供 API 密钥，或由用户在设置中配置。
- **Java 与下载**：自动发现本机 Java，选择或下载兼容运行时并设置默认版本。下载器支持校验、断点续传、重试，以及 BMCLAPI、阿里云和用户配置的下载源。
- **多人联机**：创建邀请制房间、用邀请码加入，或将房间发布到公开大厅；MCTier 房间提供可选的语音聊天。还包含 Hongshi 公网隧道与 Terracotta 服务端功能。
- **AI 助手**：连接用户配置的模型服务，帮助分析启动日志、查找资源、准备安装计划和管理实例。内置工具有权限控制；修改操作依权限设置执行或请求确认。也可连接外部 MCP 服务，为助手添加工具。
- **本地插件**：通过本地 JSON 清单安装插件，可添加受限页面、主页面板、主题和快捷操作，并在启用前查看权限。当前不提供远程插件市场；详见[插件 API 文档](docs/MC-TRIN-PLUGIN-API.md)。
- **实验室工具**：提供种子地图、建筑预览、配方生成、命令生成、皮肤编辑、模组翻译和渐变文字等工具。
- **备份与管理**：管理世界存档、截图、实例文件、下载任务和实例备份；支持备份恢复及设置项管理。

## 重要说明

AI 模型和 MCP 服务由用户自行配置，使用时会按所选服务发送相应的提示内容、日志或工具请求。公开联机大厅依赖项目配置的共享信令服务和网络连接；房间转发使用 EasyTier 无 TUN 模式。语音聊天为可选功能，需要用户授权麦克风并满足网络连接条件。

不同资源来源、账号类型和联机服务各自有网络、API 密钥或服务端要求；可用功能也可能因平台和发布构建配置而异。MC-Trin 是独立社区项目，与 Mojang、Microsoft、Rinth、CurseForge 或 MCTier 均无从属关系。

## 下载

从 [GitHub Releases](https://github.com/xiaoxin-zk/MC-Trin/releases) 获取发布版本。Windows、macOS 和 Linux 的 CI 构建会在 GitHub Actions 中验证；实际可用安装包以对应版本发布页列出的产物为准。

## 开发

开发环境要求 Node.js 24.15 或更新版本、pnpm 10.33.2，以及仓库 `rust-toolchain.toml` 指定的 Rust 工具链。克隆后初始化 Git 子模块：

```sh
git clone --recurse-submodules https://github.com/xiaoxin-zk/MC-Trin.git
cd MC-Trin
pnpm install
pnpm app:dev
```

构建桌面应用：

```sh
pnpm app:build
```

Windows 开发还需要 Visual Studio C++ Build Tools 和 MinGW UCRT64 GCC。Cubiomes 构建找不到 GCC 时，可设置 `AXOLOTL_CUBIOMES_CC` 指向 `gcc.exe`。更多贡献和本地数据隔离说明见 [CONTRIBUTING.md](CONTRIBUTING.md)。

## 许可

桌面应用基于 GPL-3.0-only 授权的 Modrinth App 源码修改而来。分发或修改时请保留上游版权、许可及第三方告知文件：[桌面许可](apps/app/LICENSE)、[前端许可](apps/app-frontend/LICENSE)、[第三方与来源说明](MC-TRIN-NOTICES.md)。
