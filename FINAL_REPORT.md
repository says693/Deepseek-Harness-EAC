# DSHEAC AIO v1 — 最终交付报告

完成时间：2026-09-02  
版型：AIO（All-in-One）  
用户版本：v1  
机器 SemVer：1.0.0  
上游溯源：DSH-Desktop-EAC `v4.5-lite` / `de55ef6d5319eacc24ce60309acc261b9fb78b6c`

## 最终产物

| 文件 | 大小 | SHA-256 |
| --- | ---: | --- |
| `DSHEAC-AIO-v1-Setup-x64.exe` | 350,624,914 bytes | `56b3227fa7de97baccbcdca2fe6047ae1f0d0d782081a919fa5ef22bcac4a43c` |
| `DSHEAC-AIO-v1-Source.zip` | 212,179,779 bytes | `78fe7e753afffe9f9598765bc0ef8edcbd5ec590f99d6e0cf5765fba72578d9f` |

## 测试结果

- JavaScript：275/275 PASS
- Rust：12/12 PASS
- sidecar RPC：PASS
- NSIS 静态契约：PASS
- staging bundle-manifest：437 个包，自检 PASS
- profile seed 机器路径扫描：PASS
- 安装 E2E：PASS

最终 E2E 报告：

`verification/verification-20260902-021235-780-6228-ce31f805.json`

### E2E 指标

- 静默安装：64.556 秒，退出码 0
- 安装路径：包含中文和空格
- 安装 payload：19,317 文件，345,348,432 bytes
- 首启至 HTTP 200：9.466 秒
- HTTP 端口：14308
- 监听 PID 属于本轮应用进程树：是
- profile 必需插件/技能缺失：0
- 私密设置标记：0
- 本机 pnpm 元数据残留：0
- 静默卸载完整清理：51.432 秒，退出码 0
- 安装目录残留：无
- AIO 进程残留：无
- 监听端口残留：无
- 外部隔离用户数据：按默认策略保留

## 安装与首启优化

- staging 从早期 225.3 MiB 降至 140.4 MiB；
- 复制时过滤 17,931 个非运行时文件和 20 个重复/异构目录；
- 未压缩资源减少约 104.2 MiB；
- 过滤 source map、PDB、TypeScript 声明、ARM64 预编译件；
- OpenTelemetry 仅保留 Node 实际使用的 CommonJS `build/src`，去除重复 `build/esm` / `build/esnext`；
- 最长 staging 路径降至 259 字符；
- 安装器拒绝超过 120 字符的安装根，避免 NSIS 静默漏文件；
- 卸载器为深层 node_modules 增加 robocopy 空镜像清理。

## 安全与隐私修复

- 停用无令牌、可读取任意绝对路径的壳层预览服务；
- 仅接受刚启动 DSH 子进程 stdout 的 ready URL 建立受信 origin；
- Tauri IPC 统一校验 main 窗口和运行时受信 origin；
- 外链使用参数化 `explorer.exe`，不拼接 PowerShell/cmd；
- profile seed 删除 `.modules.yaml`、`.pnpm-workspace-state-v1.json`、`.pnpm/lock.yaml`；
- 个人化 status rotator 文案替换为中性公共默认值；
- 构建时全树扫描原工作区、用户目录、`.dsh-v4lite` 和 pnpm store 痕迹。

## 仍未闭合的公开发布风险

技术安装与卸载已在本机验收通过，但当前不建议未经进一步处理就公开发布：

1. 安装包未 Authenticode 签名；
2. 第三方依赖、Node/npm、Rust crates、WebView2、插件和素材尚无完整 SBOM/notice bundle；
3. 部分本地插件或素材的许可证/权利人证据不足；
4. 当前图标与 `com.deepseek.*` identifier 存在品牌关联风险；
5. 插件更新依赖 npm/GitHub HTTPS，尚无应用级签名 manifest 或固定内容摘要；
6. `csp: null` 与全局 Tauri API 仍需兼容性验证后进一步收紧。

因此结论应区分：

- **本机技术可安装性：PASS**
- **无条件公开再分发授权：未证明**
