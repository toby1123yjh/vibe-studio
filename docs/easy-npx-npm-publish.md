# Vibe Studio npm 发布流程

Vibe Studio 使用 GitHub Actions 构建 Windows x64、Linux x64 和 Web 前端，再以 `@toby1123yjh/vibe-studio` 发布到 npm 官方源。本机不构建发布包；安装后的命令仍为 `vibe-studio`。

## main 自动发布

推送到 `main` 会运行 **Publish Vibe Studio to npm**。workflow 文件名保持为 `.github/workflows/publish-easy-npx.yml`，npm Trusted Publishing 必须绑定这个文件名。

```text
AUTO_VERSION_BASE: 2.0.0-beta.1
AUTO_NPM_TAG: beta
```

workflow 从 `npx-cli/package.json` 读取包名，查询这个包已发布的 `2.0.0-beta.N`，再选择下一个序号。新包从 `2.0.0-beta.1` 开始；旧包 `easy-vibe-kanban` 的版本不会影响它。只有 npm 明确返回 E404 才视为新包；网络或认证错误会中止版本计算。

构建会把解析出的版本临时写入 npm 包，然后打包、上传 `.tgz` 和平台 zip，最后发布到 npm。这样发布失败时仍可下载构建产物。artifact 名使用不含 npm scope 的名称，避免 `/` 导致上传失败。它不会自动创建 GitHub Release。

## 新包首次发布

仓库改名不会把旧 npm 包和它的发布授权转移到新包。`@toby1123yjh/vibe-studio` 尚未发布时，先在 GitHub 仓库的 Actions secrets 中设置 `NPM_TOKEN`。token 需要具有在 `toby1123yjh` scope 下创建并发布这个公共 npm 包的权限；如果 npm 账号要求发布时 2FA，应使用满足该发布策略的 token。workflow 通过 `--access public` 发布公共包。

执行一次 `publish`，在 npm 官方源确认包已经创建，然后到 **@toby1123yjh/vibe-studio 的 package settings** 配置 GitHub Actions trusted publisher：

```text
Repository owner: toby1123yjh
Repository name: vibe-studio
Workflow filename: publish-easy-npx.yml
Environment name: 留空
```

之后的发布可以使用 OIDC，验证成功后可移除用于首次发布的 `NPM_TOKEN`。workflow 没有 GitHub environment；如以后增加 environment，需同步 npm 的对应设置。

新包没有 `NPM_TOKEN` 时，workflow 会在版本解析后立即报出配置错误，避免继续执行注定无法发布的大型构建。`dry-run` 不需要此首次发布凭据。

## 手动发布

进入仓库 **Actions → Publish Vibe Studio to npm → Run workflow**：

```text
version: 留空，自动选择下一个版本
npm_tag: beta
publish_mode: publish
```

也可以填写精确版本（如 `2.0.0-beta.1`）；npm 不允许重复发布相同的 `name@version`。选择 `dry-run` 只构建和校验，不发布。

## 发布后验证

先确认 npm 官方源上的实际版本，再运行该版本：

```powershell
npm view @toby1123yjh/vibe-studio@beta version --registry https://registry.npmjs.org/
npx --yes --registry https://registry.npmjs.org/ @toby1123yjh/vibe-studio@beta
```

显式指定包与命令：

```powershell
npx --yes --registry https://registry.npmjs.org/ --package @toby1123yjh/vibe-studio@beta vibe-studio
```

第三方镜像可能延迟同步。首次发布完成前，新的安装命令还不可用；仓库改名或构建成功本身不代表 npm 发布成功。

## 失败排查

- 首次发布报缺少凭据：配置允许创建此包的 `NPM_TOKEN`，重新运行 workflow。
- 已发布包认证失败：检查 trusted publisher 的 owner、repository、workflow filename 与当前仓库完全一致，确认没有额外的 environment 要求。
- 包元数据校验失败：`npx-cli/package.json` 的 `repository.url` 必须是 `https://github.com/toby1123yjh/vibe-studio`。
- 版本已存在：留空版本以自动选择新版本，或填写未发布的版本。
- registry 查询失败：先解决 npm 网络/认证错误，再重跑；不要把这类错误当成新包。

发布 job 使用 npm 11.12.0，保留 `contents: read` 和 `id-token: write` 权限。归档内部的 `vibe-kanban`、`agent-process-host`、`vibe-kanban-mcp` 名称属于执行协议，不能只随产品展示名称一起替换。
