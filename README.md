# Apeiron CLI

统一入口：`apeiron <模块> <命令>`。首版接入现有 Ontology CLI，保留参数、输出、退出码和认证配置。Node.js 22+；onto 模块还需要 Bun 与已安装依赖的 ontology 仓库。

## 本地安装

```sh
git clone https://github.com/TheApeironLab/apeiron-cli.git
cd apeiron-cli
npm link
apeiron --help
```

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
npm test
apeiron verify
```

当前仅为本地安装入口，尚未发布到 npm registry。
