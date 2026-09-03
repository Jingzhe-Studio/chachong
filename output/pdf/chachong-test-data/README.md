# 查重工具测试数据

这是一组可直接导入桌面应用的离线测试数据。文档参考库含 100 份 PDF，代码
参考库含 100 个 Python 文件。外部材料仅选用明确允许再利用的 CC BY 4.0 论文
和 MIT 代码，并固定到具体版本。详细归属见 `metadata/SOURCES.md`。

## 导入顺序

1. 在“参考库”中新建文档库，导入 `reference_library/documents`。
2. 新建代码库，导入 `reference_library/code`。
3. 在批次页面导入 `batch`；其中每个一级目录会成为一份作业。
4. 依次运行三种算法，并参照 `metadata/expected_matches.json` 检查结果。

文档库中的 2 份文件是完整 arXiv 原文，另外 98 份是从这两篇原文生成的不同
正文摘录，用于形成百文件级、含近重复项的召回压力测试语料。代码库中的 100
个文件来自同一 GitHub 提交，并保留仓库内相对路径。

## 作业设计

- `student_01_exact_copy`：论文与 merge sort 都是参考库内容的完整副本。
- `student_02_partial_and_modified`：论文只保留参考论文前 3 页；binary search
  系统性改写了标识符，另含一份跨作业共享代码。
- `student_03_peer_copy`：使用不在参考库中的跨领域论文，同时复制 student_02 的共享代码。
- `student_04_clean_code`：来自同一 MIT 仓库、但不在参考库中的独立代码校准样本。

相似度是算法相关的，不把某个浮点分数写死为测试断言；应检查来源排序和风险区域。
两个校准样本不应产生 15% 以上的强参考库命中；百文件语料下允许出现少量普通词或
代码关键字形成的低分片段命中。
