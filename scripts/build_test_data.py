#!/usr/bin/env python3
"""Build traceable document/code fixtures for the desktop similarity checker."""

from __future__ import annotations

import argparse
import hashlib
import json
import re
import shutil
import tempfile
import urllib.request
from pathlib import Path

try:
    from pypdf import PdfReader, PdfWriter
except ImportError as error:  # pragma: no cover - depends on the caller's Python
    raise SystemExit(
        "Missing pypdf. Run: python -m pip install -r "
        "scripts/requirements-test-data.txt"
    ) from error


ARXIV_SOURCES = {
    "blood_flow": {
        "id": "arXiv:1604.05171v1",
        "title": (
            "Physical description of the blood flow from the internal jugular "
            "vein to the right atrium of the heart: new ultrasound application "
            "perspectives"
        ),
        "authors": ["Francesco Sisini"],
        "url": "https://export.arxiv.org/pdf/1604.05171v1",
        "license": "CC BY 4.0",
        "license_url": "https://creativecommons.org/licenses/by/4.0/",
    },
    "jugular_pulse": {
        "id": "arXiv:1604.05177v1",
        "title": (
            "Quantitative analysis of jugular venous pulse obtained by using a "
            "general-purpose ultrasound scanner"
        ),
        "authors": ["Francesco Sisini"],
        "url": "https://export.arxiv.org/pdf/1604.05177v1",
        "license": "CC BY 4.0",
        "license_url": "https://creativecommons.org/licenses/by/4.0/",
    },
    "array_programs": {
        "id": "arXiv:2002.09857v1",
        "title": "Verifying Array Manipulating Programs with Full-Program Induction",
        "authors": ["Supratik Chakraborty", "Ashutosh Gupta", "Divyesh Unadkat"],
        "url": "https://export.arxiv.org/pdf/2002.09857v1",
        "license": "CC BY 4.0",
        "license_url": "https://creativecommons.org/licenses/by/4.0/",
    },
}

GITHUB_COMMIT = "dc7d5ebfa4f29feac8d7d1cc485a25a90960b3aa"
GITHUB_REPOSITORY = "https://github.com/TheAlgorithms/Python"
GITHUB_RAW_ROOT = (
    "https://raw.githubusercontent.com/TheAlgorithms/Python/" + GITHUB_COMMIT
)
GITHUB_SOURCES = {
    "merge_sort": "sorts/merge_sort.py",
    "binary_search": "searches/binary_search.py",
    "euclidean_distance": "maths/euclidean_distance.py",
    "license": "LICENSE.md",
}

DATASET_README = """# 查重工作台测试数据

这是一组可直接导入桌面应用的离线测试数据。外部材料仅选用明确允许再利用的
CC BY 4.0 论文和 MIT 代码，并固定到具体版本。详细归属见 `metadata/SOURCES.md`。

## 导入顺序

1. 在“参考库”中新建文档库，导入 `reference_library/documents`。
2. 新建代码库，导入 `reference_library/code`。
3. 在批次页面导入 `batch`；其中每个一级目录会成为一份作业。
4. 依次运行三种算法，并参照 `metadata/expected_matches.json` 检查结果。

## 作业设计

- `student_01_exact_copy`：论文与 merge sort 都是参考库内容的完整副本。
- `student_02_partial_and_modified`：论文只保留参考论文前 3 页；binary search
  系统性改写了标识符，另含一份跨作业共享代码。
- `student_03_peer_copy`：使用不在参考库中的跨领域论文，同时复制 student_02 的共享代码。
- `student_04_clean_code`：来自同一 MIT 仓库、但不在参考库中的独立代码校准样本。

相似度是算法相关的，不把某个浮点分数写死为测试断言；应检查来源排序和风险区域。
两个校准样本不应命中参考库，用于验证分块召回和连续词组比较能够过滤论文常用词及
代码关键字造成的误报。
"""

SHARED_CODE = '''"""Synthetic peer-copy fixture; not sourced from the reference library."""

from collections.abc import Iterable


def rolling_average(samples: Iterable[float], window_size: int) -> list[float]:
    """Return the simple moving average for each complete window."""
    if window_size <= 0:
        raise ValueError("window_size must be positive")

    window: list[float] = []
    running_total = 0.0
    averages: list[float] = []
    for sample in samples:
        value = float(sample)
        window.append(value)
        running_total += value
        if len(window) > window_size:
            running_total -= window.pop(0)
        if len(window) == window_size:
            averages.append(running_total / window_size)
    return averages


def threshold_crossings(values: Iterable[float], threshold: float) -> list[int]:
    """Return indexes where a sequence first moves from below to above a limit."""
    crossings: list[int] = []
    previous = None
    for index, value in enumerate(values):
        if previous is not None and previous < threshold <= value:
            crossings.append(index)
        previous = value
    return crossings
'''


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    default_output = Path(__file__).resolve().parents[1] / "output/pdf/chachong-test-data"
    parser.add_argument(
        "--output",
        type=Path,
        default=default_output,
        help=f"dataset directory (default: {default_output})",
    )
    return parser.parse_args()


def download(url: str, destination: Path) -> None:
    request = urllib.request.Request(
        url,
        headers={"User-Agent": "chachong-test-data/1.0 (local fixture builder)"},
    )
    with urllib.request.urlopen(request, timeout=60) as response:
        payload = response.read()
    destination.parent.mkdir(parents=True, exist_ok=True)
    destination.write_bytes(payload)


def sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        for block in iter(lambda: stream.read(1024 * 1024), b""):
            digest.update(block)
    return digest.hexdigest()


def copy(source: Path, destination: Path) -> None:
    destination.parent.mkdir(parents=True, exist_ok=True)
    shutil.copyfile(source, destination)


def write_text(destination: Path, content: str) -> None:
    destination.parent.mkdir(parents=True, exist_ok=True)
    destination.write_text(content, encoding="utf-8", newline="\n")


def write_partial_pdf(source: Path, destination: Path, page_count: int = 3) -> int:
    reader = PdfReader(source)
    if len(reader.pages) < page_count:
        raise RuntimeError(f"{source.name} has only {len(reader.pages)} pages")
    writer = PdfWriter()
    for page in reader.pages[:page_count]:
        writer.add_page(page)
    writer.add_metadata(
        {
            "/Title": "Partial reuse fixture - first three source pages",
            "/Subject": "Derived from arXiv:1604.05177v1 under CC BY 4.0",
            "/Creator": "chachong test-data builder",
        }
    )
    destination.parent.mkdir(parents=True, exist_ok=True)
    with destination.open("wb") as stream:
        writer.write(stream)
    return page_count


def rewrite_identifiers(source: str) -> str:
    replacements = {
        "collection": "sorted_values",
        "item": "target_value",
        "left": "lower_bound",
        "right": "upper_bound",
        "midpoint": "middle_index",
    }
    rewritten = source
    for old, new in replacements.items():
        rewritten = re.sub(rf"\b{old}\b", new, rewritten)
    if rewritten == source:
        raise RuntimeError("binary_search.py did not contain the expected identifiers")
    notice = (
        "# Adapted test fixture from TheAlgorithms/Python (MIT).\n"
        "# Change: systematic identifier renaming for similarity testing.\n\n"
    )
    return notice + rewritten


def source_notes() -> str:
    paper_lines = []
    for source in ARXIV_SOURCES.values():
        paper_lines.append(
            f"- **{source['id']}** - {source['title']} - "
            f"{', '.join(source['authors'])}. [{source['license']}]"
            f"({source['license_url']}); [PDF]({source['url']})."
        )
    return f"""# Sources and licenses

Generated test fixtures are not real student submissions.

## arXiv documents

{chr(10).join(paper_lines)}

`student_02_partial_and_modified/report.pdf` contains pages 1-3 of
arXiv:1604.05177v1. The selection and PDF metadata are changes made for this fixture.
The exact-copy and independent-control PDFs are unmodified copies.

## GitHub code

- Repository: [TheAlgorithms/Python]({GITHUB_REPOSITORY})
- Snapshot commit: `{GITHUB_COMMIT}`
- License: MIT; the snapshot license text is stored as
  `metadata/TheAlgorithms-Python-LICENSE.md`.
- Imported paths: `sorts/merge_sort.py`, `searches/binary_search.py`, and
  `maths/euclidean_distance.py`.

The student_02 binary-search fixture only renames identifiers and adds an attribution
notice. The merge-sort fixture is an exact copy. The Euclidean-distance file is a
cross-file calibration probe and is intentionally absent from the reference library.

`peer_shared.py` is synthetic fixture code created for this dataset and is copied
between two work items to exercise within-batch matching.
"""


def expected_matches() -> dict[str, object]:
    return {
        "schemaVersion": 1,
        "notes": [
            "Scores vary by algorithm; relations and ordering are the assertions.",
            "A stored match requires similarity >= 0.15 and at least 16 matched bytes.",
        ],
        "cases": [
            {
                "query": "batch/student_01_exact_copy/report.pdf",
                "scope": "reference",
                "source": "reference_library/documents/arxiv_1604.05171v1_blood_flow.pdf",
                "expectation": "exact-copy; similarity should be 1.0",
            },
            {
                "query": "batch/student_01_exact_copy/src/merge_sort.py",
                "scope": "reference",
                "source": "reference_library/code/sorts/merge_sort.py",
                "expectation": "exact-copy; similarity should be 1.0",
            },
            {
                "query": "batch/student_02_partial_and_modified/report.pdf",
                "scope": "reference",
                "source": "reference_library/documents/arxiv_1604.05177v1_jugular_pulse.pdf",
                "expectation": "pages 1-3 reused; should produce highlighted regions",
            },
            {
                "query": "batch/student_02_partial_and_modified/src/binary_search_adapted.py",
                "scope": "reference",
                "source": "reference_library/code/searches/binary_search.py",
                "expectation": "identifier rewrite; strongest expected reference-code match",
            },
            {
                "query": "batch/student_02_partial_and_modified/src/peer_shared.py",
                "scope": "batch",
                "source": "batch/student_03_peer_copy/src/peer_shared_copy.py",
                "expectation": "peer copy; exact apart from the file name",
            },
            {
                "query": "batch/student_03_peer_copy/report.pdf",
                "scope": "reference",
                "source": None,
                "expectation": (
                    "cross-domain calibration probe; all algorithms should reject "
                    "reference-library matches"
                ),
            },
            {
                "query": "batch/student_04_clean_code/euclidean_distance.py",
                "scope": "reference",
                "source": None,
                "expectation": (
                    "code calibration probe; all algorithms should reject reference-library matches"
                ),
            },
        ],
    }


def build(output: Path) -> None:
    output = output.resolve()
    output.mkdir(parents=True, exist_ok=True)

    with tempfile.TemporaryDirectory(prefix="chachong-", dir=output.parent) as temporary:
        staging = Path(temporary)
        downloaded_pdfs: dict[str, Path] = {}
        for key, source in ARXIV_SOURCES.items():
            destination = staging / f"{key}.pdf"
            download(str(source["url"]), destination)
            downloaded_pdfs[key] = destination

        downloaded_code: dict[str, Path] = {}
        for key, relative_path in GITHUB_SOURCES.items():
            destination = staging / "github" / relative_path
            download(f"{GITHUB_RAW_ROOT}/{relative_path}", destination)
            downloaded_code[key] = destination

        ref_blood = output / "reference_library/documents/arxiv_1604.05171v1_blood_flow.pdf"
        ref_pulse = output / "reference_library/documents/arxiv_1604.05177v1_jugular_pulse.pdf"
        copy(downloaded_pdfs["blood_flow"], ref_blood)
        copy(downloaded_pdfs["jugular_pulse"], ref_pulse)

        ref_merge = output / "reference_library/code/sorts/merge_sort.py"
        ref_binary = output / "reference_library/code/searches/binary_search.py"
        copy(downloaded_code["merge_sort"], ref_merge)
        copy(downloaded_code["binary_search"], ref_binary)

        copy(ref_blood, output / "batch/student_01_exact_copy/report.pdf")
        copy(ref_merge, output / "batch/student_01_exact_copy/src/merge_sort.py")

        partial_pdf = output / "batch/student_02_partial_and_modified/report.pdf"
        selected_pages = write_partial_pdf(downloaded_pdfs["jugular_pulse"], partial_pdf)
        adapted = rewrite_identifiers(downloaded_code["binary_search"].read_text("utf-8"))
        write_text(
            output
            / "batch/student_02_partial_and_modified/src/binary_search_adapted.py",
            adapted,
        )
        write_text(
            output / "batch/student_02_partial_and_modified/src/peer_shared.py",
            SHARED_CODE,
        )

        copy(
            downloaded_pdfs["array_programs"],
            output / "batch/student_03_peer_copy/report.pdf",
        )
        write_text(
            output / "batch/student_03_peer_copy/src/peer_shared_copy.py",
            SHARED_CODE,
        )
        copy(
            downloaded_code["euclidean_distance"],
            output / "batch/student_04_clean_code/euclidean_distance.py",
        )

        copy(
            downloaded_code["license"],
            output / "metadata/TheAlgorithms-Python-LICENSE.md",
        )
        write_text(output / "README.md", DATASET_README)
        write_text(output / "metadata/SOURCES.md", source_notes())
        write_text(
            output / "metadata/expected_matches.json",
            json.dumps(expected_matches(), ensure_ascii=False, indent=2) + "\n",
        )

        source_hashes = {
            **{
                str(source["id"]): sha256(downloaded_pdfs[key])
                for key, source in ARXIV_SOURCES.items()
            },
            **{
                f"TheAlgorithms/Python:{path}": sha256(downloaded_code[key])
                for key, path in GITHUB_SOURCES.items()
            },
        }

    files = []
    for path in sorted(output.rglob("*")):
        if path.is_file() and path.name != "manifest.json":
            files.append(
                {
                    "path": path.relative_to(output).as_posix(),
                    "bytes": path.stat().st_size,
                    "sha256": sha256(path),
                }
            )
    manifest = {
        "schemaVersion": 1,
        "arxivSources": list(ARXIV_SOURCES.values()),
        "githubSource": {
            "repository": GITHUB_REPOSITORY,
            "commit": GITHUB_COMMIT,
            "license": "MIT",
        },
        "partialPdfPages": selected_pages,
        "sourceSha256": source_hashes,
        "files": files,
    }
    write_text(
        output / "manifest.json",
        json.dumps(manifest, ensure_ascii=False, indent=2) + "\n",
    )

    print(f"Built {len(files)} fixture files under {output}")


if __name__ == "__main__":
    build(parse_args().output)
