# cpp-infra-verify

[![CI](https://github.com/qubit-ltd/cpp-infra-verify/actions/workflows/ci.yml/badge.svg)](https://github.com/qubit-ltd/cpp-infra-verify/actions/workflows/ci.yml)
[![Crates.io](https://img.shields.io/crates/v/qubit-cpp-infra-verify.svg)](https://crates.io/crates/qubit-cpp-infra-verify)
[![文档](https://img.shields.io/docsrs/qubit-cpp-infra-verify)](https://docs.rs/qubit-cpp-infra-verify)
[![Rust](https://img.shields.io/badge/rust-1.94%2B-orange)](https://www.rust-lang.org)
[![许可证](https://img.shields.io/badge/license-Apache--2.0-blue)](LICENSE)
[![English](https://img.shields.io/badge/README-English-blue)](README.md)

C++ 库在本机能编译，并不代表公开头文件可以独立包含，也不代表用户能消费安装包。`cpp-infra-verify` 面向项目维护者，统一执行真实 CMake 构建、CTest、逐头编译，以及迁移安装目录后的消费验证。分析和覆盖率工具复用它输出的 File API 构建上下文，避免各自重新解析和配置工程。

当前为 0.1.0 开发代码，尚未发布到 crates.io 或 docs.rs；上面的徽章只是项目入口，不表示已经发布。仓库内的小工程覆盖 Linux GCC/Clang 与 Ninja，其他平台尚未完成验证。

## 安装

使用 Rust 1.94 或更新版本从源码安装：

```sh
cargo install --path . --locked
cpp-infra-verify --version --json
```

要求 CMake 3.25+，首期接受已提交的 Presets schema 6。测试使用 CMake 3.28、Ninja、GCC 和 `/usr/bin/clang++-18`。`sanitize`、`fuzz` 限定 Linux Clang 与 Ninja。只有 profile 配置了 `conan_context` 时才需要 Conan。

## 快速开始

假设应用已有 CMake 工程，并包含 `src`、`include`、`tests` 目录。先提交 `.infra/project.toml`：

```toml
schema = 1
kind = "application"
source_roots = ["src", "include"]
test_roots = ["tests"]

[profiles.debug]
configure = "debug"
build = "debug"
test = "debug"
```

再提交 `.infra/verify/config.toml`：

```toml
schema = 1
required_suites = ["build", "test", "headers"]
jobs = 4

[headers]
roots = ["include"]
exclude = []
```

项目的 `CMakePresets.json` 应声明这些 preset。configure preset 必须明确 generator，并把 `binaryDir` 放在 `${sourceDir}/build/infra/` 下的独立目录；对应 build/test preset 必须引用同一 configure preset。项目自行提供 CMake target 和 CTest 注册，然后执行：

```sh
cpp-infra-verify --project . --report build/verify-report.json run --profile debug
cpp-infra-verify --project . --report build/prepare-report.json prepare --profile debug --output build/build-context.json
python3 -c 'import json; r=json.load(open("build/verify-report.json")); assert r["status"]=="passed"'
python3 -c 'import json; c=json.load(open("build/build-context.json")); print(c["binary_dir"], [t["name"] for t in c["targets"]])'
```

显式 `prepare --output` 只接受项目 build 目录下、CMake binary tree 之外的 JSON 文件，已有目标必须是本项目的构建上下文。源码、配置、未知业务文件和符号链接别名会在构建前拒绝，写入前再次校验。`run` 执行所选检查；`prepare` 也会真实配置和构建，再输出 schema-1 上下文，包含实际编译器、生成器、target、产物、可选编译数据库及源码和配置摘要。测试数为零、全部跳过或实际失败都会返回失败。`plan --profile debug` 只解析本地配置并输出命令依赖图，不启动外部工具。

## 检查套件与配置

| suite | 行为与必要配置 |
| --- | --- |
| `build` | 使用已提交的 preset 配置、构建，通过 CMake File API 发现实际 target。 |
| `test` | 按同一 CTest preset 先发现再执行测试，保留 JUnit，拒绝零测试和全跳过结果。 |
| `headers` | 每个公开头单独生成翻译单元，在私有构建中保留原 preset、环境和有效 cache，使用项目 target 的使用要求编译。可用 `targets` 明确指定项目 target；`exclude` 必须在 `exceptions` 中映射到存在的宿主头，例如 `exceptions = { "include/private.ipp" = "include/public.hpp" }`。 |
| `install` | 先安装到 prefix-A，复制到 prefix-B 后移除本次自建的 prefix-A；使用迁移后的包配置编译、链接并按需运行消费程序，拒绝导出源码、build 或原安装前缀路径。应用通过安装可执行文件和 smoke 参数验证。 |
| `package` | 执行 CPack preset，安全解包 ZIP/TGZ，检查路径越界、重复条目、链接、大小预算和必需文件。 |
| `sanitize` | 核验 preset 声明的编译与链接插桩，运行 address+undefined 或独立 thread sanitizer 测试。 |
| `fuzz` | 核验 Linux Clang libFuzzer target；支持只构建或限时 smoke，复制 corpus 后运行，保留 crash 和复现参数。 |

单独执行 `run --suite install` 也会完成构建前置，同次调用只配置和构建一次。显式请求未配置或不支持的 suite 会报配置错误，缺少可选配置不会默认启用高级检查。前置失败后，其余检查记录为 blocked。

library 和 header-only 项目必须配置安装消费；header-only 仍需可编译 fixture 和测试。库的典型配置如下：

```toml
[install]
package = "Example"
targets = ["Example::Example"]
headers = ["example/example.hpp"]
consumer_source = "tests/infra/consumer.cpp"
run_consumer = true

[package]
preset = "package"
required_files = ["include/example/example.hpp"]

[sanitize]
sanitizers = ["address", "undefined"]

[fuzz]
mode = "smoke"

[[fuzz.targets]]
target = "example_fuzz"
seconds = 10
max_len = 4096
corpus = "tests/corpus"
timeout_seconds = 5
```

安装 `headers` 支持相对标准 `include` 目录的名字，也支持显式前缀相对路径。应用使用 `[install] executables = ["bin/example"]`、`smoke_args = []` 和 `run_consumer = true`。交叉编译后若要执行消费程序，必须声明 `emulator = ["qemu-..."]`；也可明确设 `run_consumer = false`，报告会标记 build-only。configure/build/test 超时默认分别为 300/1800/600 秒，均须为正数。fuzz 的 `seconds`、`max_len` 和 `timeout_seconds` 也必须为正数；进程总超时为 smoke 预算加单输入超时，再加五秒退出余量。

交付的 [CMake 辅助函数](conf/cmake/verify.cmake) 和 [隐藏 preset 基础模板](conf/presets/verify.json) 登记在 [conf/manifest.json](conf/manifest.json)。项目可在专用 profile 中对 target 调用 `cpp_infra_sanitizers(target)` 或 `cpp_infra_fuzzer(target)`。这些辅助函数由项目显式接入；verify 不修改普通构建的编译参数。组织管理的策略文件与项目配置分开维护。

## 报告与集成

公共报告包含工具身份、bundle/run/task 标识、规范项目路径、源码与配置摘要、状态、真实工具版本、命令参数数组、耗时、日志、诊断及带 SHA-256 的产物。上下文和每次调用的工作目录位于 `build/infra-runs/<run-id>/verify/<invocation-id>/`，artifact 路径相对公共 `report_root`。所有命令支持 `--report`、`--offline` 和 `--run-context`。退出码：0 成功、1 构建/测试/门禁失败、2 配置或协议错误、3 I/O 或进程启动故障、130 取消。

C++ 基础设施入口默认追踪 `cpp-infra` main。运行上下文携带本次调用的不可变工具快照，只有显式基础设施更新才改写项目受管理文件。存在 `runtime_snapshot` 和 `runtime_snapshot_digest` 时，会校验快照 bundle、SHA 以及当前工具的可执行文件摘要和身份。无上下文的源码开发调用使用 `development` bundle，仍校验源码与配置指纹。`CPP_INFRA_BIN_DIR` 固定兄弟工具所在目录，其中缺少依赖工具时会失败，不会从 PATH 混入其他 bundle。

profile 声明 `conan_context` 时，先调用 `cpp-infra-dependency prepare --context NAME --output JSON`，再获取 `binary_dir/.cpp-infra.lock`。持锁后重新核验依赖生成文件，并比对实际 CMake 编译器、版本、标准和构建类型与 Conan settings。消费构建产物的工具必须重新获得同一把锁，并核验 `.cpp-infra-build-state.json` 指纹。失败或中断的重建会使旧成功状态失效，工具不会接管未带管理标记的非空目录。

项目根目录的 JSON 报告会通过运行上下文、公共报告和构建上下文中的 `source_excluded_paths` 明确记录，连续执行不会把上一次报告当成源码输入。只接受规范的非输入报告文件；源码、测试、配置路径及已有业务 JSON 不得覆盖或排除。配置摘要始终不受这些排除项影响。

项目/verify TOML、根 CMakeLists 和递归 preset include 必须是位于可跟踪区域的非符号链接普通文件，不能从 build、target、Git 或事务排除区导入配置；freshness 阶段会再次核验实际输入清单。构建上下文、File API 和运行快照仍使用各自的产物协议。

构建上下文记录 CMake 实际配置输入及缓存的 SHA-256。项目工具链与 include 必须属于受跟踪输入；CMake 安装模块和 Conan 生成工具链分别提供归属证明。受管引擎文件绑定实际 CMake 完整版本和既有生成证明，重新 prepare 不能把手改的构建目录配置变成合法输入。每个 suite 以及 prepare/run 结束时，在报告成功前重验这些证明、完整成功构建状态、管理标记和依赖生成文件。消费方通过带 `run-context.build_context` 的 `prepare` 在构建锁外复验旧上下文，再获取同一把锁并在消费期间核验哈希。

基础设施初始化仅在缺失时创建 `.infra/verify/config.toml`。模板默认启用构建和测试；填写项目设置后再启用头文件、安装与打包检查。基础设施更新保留这份项目配置。

所有 TOML 都要求 `schema = 1` 并拒绝未知字段。配置路径不得通过 `..` 或符号链接越出项目，各类源码根不能重叠。preset include/继承环及冲突的用户 preset 定义会被拒绝，无关的本地 preset 不影响已提交的 profile；构建目录宏仅支持可确定的 `sourceDir`、`presetName`。检查过程不修改源码和配置，也不会上传或发布包。

## 测试

```sh
cargo +nightly fmt --all -- --check --config-path .infra/style/rustfmt.toml
cargo test --locked
cargo clippy --all-targets -- -D warnings
```

测试使用自带小型 CMake 工程，覆盖真实 GCC/Clang 构建、库/header-only/应用的迁移安装消费、多配置构建、CTest 跳过和失败、ZIP/TGZ、ASan 越界、libFuzzer crash、超时、取消和旧上下文拒绝。隔离协议模拟覆盖损坏的 CTest/dependency 回复和不支持的 CMake 版本；适用场景中的构建仍调用真实 CMake。另有本地集成执行真实 Conan prepare 与上下文复用。测试不依赖下游仓库。实际 TSan 运行、MSVC、AppleClang、Windows 及真实交叉编译尚需集成验证。

## 许可证

Apache License 2.0，详见 [LICENSE](LICENSE)。Copyright © 2026 Haixing Hu。

## 贡献

保持改动范围聚焦，为行为变更提供有效的回归 fixture，提交前运行上述检查。

## 作者

Haixing Hu <starfish.hu@gmail.com>，Qubit。
