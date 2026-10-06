# Apeiron CLI

统一入口：`apeiron <模块> <命令>`。`apeiron init` 提供内置本地配置网页，`apeiron onto` 接入现有 Ontology CLI。TypeScript strict + Bun 1.3.14+；onto 模块需要已安装依赖的 ontology 仓库。

## 本地安装

```sh
git clone https://github.com/TheApeironLab/apeiron-cli.git
cd apeiron-cli
bun install --frozen-lockfile
bun link
apeiron --help
```

## 浏览器配置向导

```sh
apeiron init
```

命令监听 `127.0.0.1` 的空闲端口，自动打开本地网页，依次收集：

1. **Slug name**：组织标识，小写字母、数字和连字符，最长 63 个字符。
2. **LLM 配置**：Base URL、API Key、Model ID。API Key 可以留空以支持无鉴权的本地模型。
3. **应用**：Apeiron、Limani / Ontology、Corpus、Task、Vasi、Files、Filer、Gateway；至少选择一个。

应用 ID 对应宸途 release；这里保存用户选择，部署依赖仍由宸途 Helmfile 负责。
界面、脚本和样式直接包含在 CLI 内，没有 CDN 或单独前端服务，也不依赖 LLM 完成配置。
保存只写入配置，不调用模型、测试连接、部署或启动应用。

默认路径为 `$XDG_CONFIG_HOME/apeiron/config.json`，未设置时为 `~/.config/apeiron/config.json`。
文件含明文 API Key，在 Unix 上使用 `0600` 权限，新建配置目录为 `0700`。禁止保存到 Git 工作目录。
API Key 不回传到网页、不写入日志或浏览器存储。再次启动会加载已有配置；密钥栏留空可保留已有密钥，也可以显式清除。
保存采用文件锁、版本检查和临时文件替换；其他窗口已保存新版本时，旧窗口需要刷新，避免覆盖。

```sh
apeiron init --no-open                       # 手动打开终端输出的地址
apeiron init --port 3210                     # 指定本地端口
apeiron init --config /external/apeiron.json  # 指定配置文件
apeiron init --help
```

单次会话使用随机访问路径，写入接口校验同源请求。完成保存后点击“完成并关闭向导”或在终端按 Ctrl+C，关闭本地服务。
在远程服务器执行时，通过 SSH 转发指定端口，再在本机浏览器打开相同的随机路径；不开放公网监听。

## 独立可执行文件

```sh
bun run build
./dist/apeiron init
```

构建生成当前系统架构的可执行文件，包含本地配置页面和 Bun 运行时，使用向导无需在目标机器安装 Bun。
`onto` 转发仍需要其源码/依赖或配置好的外部 CLI。构建产物不提交到 Git；暂未发布跨平台下载包。

## Ontology 模块

默认寻找相邻的 `../ontology` 仓库，也可以指定：

```sh
export APEIRON_ONTO_ROOT=/absolute/path/to/ontology
export ONTO_MODE=remote
export ONTO_ENDPOINT=http://127.0.0.1:3101/ontology
apeiron onto status
apeiron onto schema
apeiron onto --help
```

连接与认证沿用 Ontology CLI 的配置，不内置服务地址或凭据。顶层 `apeiron status` / `apeiron verify` 检查本地入口；`apeiron onto status` 查看业务服务。

## 扩展模块

```sh
export APEIRON_WLK_BIN=/absolute/path/to/wlk
apeiron wlk --help
apeiron wlk <command>
```

`APEIRON_<MODULE>_BIN` 指定单个可执行文件路径，参数原样转交，不经过 shell。模块名中的 `-` 对应环境变量中的 `_`。也可用 `APEIRON_ONTO_BIN` 覆盖 onto 入口，或 `APEIRON_BUN_BIN` 指定 Bun 路径。未配置的模块返回退出码 4，不提供虚构的业务命令。

## 验证

```sh
bun run typecheck
bun test
bun run build
bun run test:ui
apeiron verify
```

首次浏览器验证需先运行 `bunx playwright install chromium`。`test:ui` 从临时目录启动编译后的二进制，
验证三步配置、保存和重载、密钥保留、完成退出、桌面/手机布局以及没有外部网络请求。
浏览器测试只用虚构配置，完成后删除配置文件；截图保存在打印出的系统临时目录。

当前尚未发布到 npm registry。顶层 `status` / `verify` 仍沿用早期 Ontology 入口检查含义；
`platform init` 已由顶层 `init` 的网页流程替代，`platform install/deploy` 暂未实现。
