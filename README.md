# 查重工作台

本地桌面端作业查重管理工具。Rust 负责业务、解析调度、检索与比较流程，Tauri 承载桌面壳，前端负责交互与结果呈现。

## 当前功能

应用已支持本地持久化的作业导入、文件级查重和参考库维护：

- 批次根目录的第一层目录作为作业项，每个作业项可递归包含多个文档和代码文件；根目录下的单个文件会作为独立作业项。
- 批次、作业项、托管文件、解析状态和文件级匹配结果均持久化到 SQLite。
- 查重时分别检索同一批次的其他作业项和手动维护的参考库，两类来源独立记录。
- 文件详情先展示多个匹配来源，选中来源后并排显示双方原文，按文档页码或代码行号导航连续高风险区域。
- 作业项总体指标保留为空白抽象，目前只展示由单个文件结果直接得到的匹配数和最高相似度。

内置三种静态注册且代码解耦的算法，全部遵循 `preprocess / retrieve / compare` 三阶段接口：

- 长词组覆盖率：使用中英文分词或代码词法单元生成长词组，偏向大段连续复制。
- 短词组覆盖率：使用较短的连续词组，在保持精度的同时允许少量局部修改。
- 稀疏指纹覆盖率：从词法单元中选择稀疏指纹，偏向代码或长文本片段复用。

三种算法均以文本块建立带稀有度权重的倒排索引，先按块召回、再聚合到文件；精确比较使用查询文件的连续匹配覆盖率，而不是会被较长来源稀释的对称相似度。

参考库 Tab 支持：

- 创建、重命名、删除文档库和代码库；库类型创建后保持不变。
- 导入一个或多个文件，或递归导入目录。
- 文档库接受 `.pdf`、`.doc`、`.docx`，使用 LiteParse v2 本地解析；Word 解析依赖 LibreOffice。
- 代码库按 UTF-8 文本读取，递归导入时忽略 `.git`、`node_modules`、`target`、`dist`、`build`、符号链接和二进制文件。
- 展示 `pending / parsing / ready / failed` 状态、解析错误，并支持失败重试与文件删除。
- 原始文件按 SHA-256 托管，解析正文和位置映射保存为 JSON；同库重复内容跳过，跨库复用原始对象与解析产物。

数据保存在 Tauri 为应用分配的本地数据目录中，SQLite 启用外键和 WAL。解析产物与匹配结果会落盘；三种算法的召回索引在每次查重时按当前批次和参考库语料构建，不跨运行持久化。

OCR 明确关闭。扫描 PDF 如果没有可提取文本，会保留为失败文件并提示当前版本未启用 OCR。

## 项目结构

```text
.
├─ crates/chachong-core/src
│  ├─ domain/                  Batch、作业项、文件、参考库、文件级结果
│  ├─ application/             批次/参考库服务、查重调度与算法静态注册
│  ├─ importing/               批次与参考库的目录发现
│  ├─ parser/                  LiteParse、LibreOffice 检测、代码解析与位置映射
│  ├─ detection/               preprocess / retrieve / compare 三阶段接口
│  ├─ algorithms/
│  │  ├─ features.rs           分词、代码词法切分、分块索引与连续匹配链
│  │  ├─ text_duplicate/       长词组覆盖率
│  │  ├─ token_cosine.rs       短词组覆盖率
│  │  └─ winnowing.rs          稀疏指纹覆盖率
│  └─ storage/                 SQLite 仓储、内容寻址对象、解析产物和结果
├─ src-tauri/                  Tauri 桌面入口、命令与导入/查重进度事件
└─ src/                        批次四层导航、原文命中标注与参考库界面
```

作业项的总体指标仍按设计不在当前领域层提前定义。

## 开发命令

```bash
npm install
npm run tauri dev
```

完整验证：

```bash
cargo test --workspace
cargo check -p chachong-desktop
npm run build
```

## 测试数据

仓库提供一套从开放许可 arXiv 论文和固定 GitHub 提交构造的可追溯测试数据，
覆盖参考库完全命中、部分复用、标识符改写、同批次互抄和误报校准样本：

```bash
python -m pip install -r scripts/requirements-test-data.txt
python scripts/build_test_data.py
```

生成后按 `output/pdf/chachong-test-data/README.md` 的顺序，将文档参考库、代码
参考库和批次分别导入应用。来源、许可证、SHA-256 和预期关系会随数据一起生成。

仅测试 Rust 核心：

```bash
cargo test -p chachong-core
```

仅检查前端：

```bash
npm run build
```
