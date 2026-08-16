# igrok

**igrok** 是 [Grok Build](https://x.ai/cli) 的个人魔改版:一个终端 AI 编码代理(TUI),支持多 provider 多 model(OpenAI / Anthropic / grok / 任意 OpenAI 兼容端点 / 本地模型),内置去遥测、独立数据目录(`~/.igrok`)与自建 GitHub Releases 自更新。

## 安装

### Windows(PowerShell)

```powershell
irm https://raw.githubusercontent.com/Aliothmoon/grok-build/dev/install.ps1 | iex
```

### macOS / Linux

```sh
curl -fsSL https://raw.githubusercontent.com/Aliothmoon/grok-build/dev/install.sh | sh
```

安装脚本**只做三件事**:

1. 从 [GitHub Releases](https://github.com/Aliothmoon/grok-build/releases) 下载最新的静态链接二进制(无任何运行时依赖)
2. 放到 `~/.igrok/bin/igrok(.exe)`
3. 把该目录加进 PATH

不装其他任何东西:没有运行时、没有服务、不写凭证。数据目录 `~/.igrok/` 由程序首次运行时自动创建,与官方 grok 完全隔离。

### 手动安装

从 [Releases](https://github.com/Aliothmoon/grok-build/releases) 下载对应平台资产,重命名为 `igrok(.exe)` 放进 PATH 即可。

## 使用

```sh
# 配置任意 provider 的 key(设置对应环境变量后,模型会自动出现在 /model 列表)
export ANTHROPIC_API_KEY=sk-ant-...
export OPENAI_API_KEY=sk-...
export GROK_API_KEY=...          # 或任意 models.dev 收录的 provider

igrok                            # 启动 TUI(不会弹登录;按 l 或 /login 才登录)
igrok -m anthropic/claude-opus-4-6 -p "hello"   # headless 指定模型
```

TUI 内:

- `/model` —— 切换模型(会话内热切换,不丢上下文;推理模型带 effort 子菜单)
- `Ctrl+M` —— 模型选择器
- `/login` —— 登录 grok(可选)

### 自定义模型

`~/.igrok/config.toml`:

```toml
[model.my-model]
model = "my-model-id"
base_url = "https://api.example.com/v1"
env_key = "MY_API_KEY"
api_backend = "chat_completions"   # 或 "responses" / "messages"(Anthropic)
```

## 更新

```sh
igrok update    # 从本仓库的 GitHub Releases 拉取最新版
```

## 从源码构建

```sh
cargo build -p xai-grok-pager-bin --release   # 产物: target/release/igrok
```
