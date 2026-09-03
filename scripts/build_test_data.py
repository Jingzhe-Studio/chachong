#!/usr/bin/env python3
"""Build traceable document/code fixtures for the desktop similarity checker."""

from __future__ import annotations

import argparse
import hashlib
import json
import re
import shutil
import tempfile
import textwrap
import urllib.request
import zipfile
from pathlib import Path

try:
    from pypdf import PdfReader, PdfWriter
    from reportlab.lib.pagesizes import A4
    from reportlab.pdfgen import canvas
except ImportError as error:  # pragma: no cover - depends on the caller's Python
    raise SystemExit(
        "Missing PDF dependencies. Run: python -m pip install -r "
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
GITHUB_ARCHIVE_URL = (
    "https://codeload.github.com/TheAlgorithms/Python/zip/" + GITHUB_COMMIT
)
GITHUB_SOURCES = {
    "merge_sort": "sorts/merge_sort.py",
    "binary_search": "searches/binary_search.py",
    "euclidean_distance": "maths/euclidean_distance.py",
    "license": "LICENSE.md",
}
REFERENCE_DOCUMENT_COUNT = 100
REFERENCE_CODE_COUNT = 100
ARXIV_EXCERPTS_PER_SOURCE = 49

DATASET_README = """# 查重工作台测试数据

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


def reset_generated_directory(output: Path, relative_path: str) -> Path:
    target = (output / relative_path).resolve()
    if output != target and output not in target.parents:
        raise RuntimeError(f"refusing to reset path outside dataset: {target}")
    if target.exists():
        shutil.rmtree(target)
    target.mkdir(parents=True)
    return target


def normalized_pdf_words(source: Path) -> list[str]:
    reader = PdfReader(source)
    text = " ".join(page.extract_text() or "" for page in reader.pages)
    text = text.replace("\u2010", "-").replace("\u2011", "-")
    text = text.replace("\u2012", "-").replace("\u2013", "-")
    text = text.replace("\u2014", "-").replace("\u2212", "-")
    text = text.encode("ascii", errors="ignore").decode("ascii")
    return re.findall(r"\S+", text)


def write_excerpt_pdf(
    destination: Path,
    fixture_id: str,
    source: dict[str, object],
    words: list[str],
    start: int,
    window_words: int,
) -> dict[str, object]:
    excerpt = words[start : start + window_words]
    if len(excerpt) < 120:
        raise RuntimeError(f"not enough extractable text in {source['id']}")

    destination.parent.mkdir(parents=True, exist_ok=True)
    pdf = canvas.Canvas(
        str(destination),
        pagesize=A4,
        pageCompression=1,
        invariant=1,
    )
    width, height = A4
    pdf.setTitle(f"Similarity fixture {fixture_id}")
    pdf.setAuthor(", ".join(source["authors"]))
    pdf.setSubject(f"Excerpt derived from {source['id']} under CC BY 4.0")

    margin = 54
    y = height - margin
    pdf.setFont("Helvetica-Bold", 15)
    pdf.drawString(margin, y, f"Reference excerpt {fixture_id}")
    y -= 23
    pdf.setFont("Helvetica", 8.5)
    for line in textwrap.wrap(
        f"Source: {source['id']} | License: CC BY 4.0 | "
        f"Word window: {start + 1}-{start + len(excerpt)}",
        width=105,
    ):
        pdf.drawString(margin, y, line)
        y -= 11

    y -= 12
    pdf.setFont("Helvetica", 10)
    body = " ".join(excerpt)
    for line in textwrap.wrap(body, width=94, break_long_words=False):
        if y < margin:
            pdf.showPage()
            y = height - margin
            pdf.setFont("Helvetica", 10)
        pdf.drawString(margin, y, line)
        y -= 13
    pdf.save()

    return {
        "path": destination.as_posix(),
        "sourceId": source["id"],
        "startWord": start + 1,
        "endWord": start + len(excerpt),
    }


def build_document_reference_library(
    output: Path,
    downloaded_pdfs: dict[str, Path],
) -> list[dict[str, object]]:
    document_root = reset_generated_directory(output, "reference_library/documents")
    canonical = {
        "blood_flow": document_root / "arxiv_1604.05171v1_blood_flow.pdf",
        "jugular_pulse": document_root / "arxiv_1604.05177v1_jugular_pulse.pdf",
    }
    for key, destination in canonical.items():
        copy(downloaded_pdfs[key], destination)

    catalog: list[dict[str, object]] = []
    for source_index, key in enumerate(("blood_flow", "jugular_pulse"), start=1):
        source = ARXIV_SOURCES[key]
        words = normalized_pdf_words(downloaded_pdfs[key])
        window_words = min(260, max(140, len(words) // 4))
        max_start = len(words) - window_words
        if max_start < ARXIV_EXCERPTS_PER_SOURCE - 1:
            raise RuntimeError(f"not enough source words in {source['id']}")
        for excerpt_index in range(ARXIV_EXCERPTS_PER_SOURCE):
            start = round(
                excerpt_index * max_start / (ARXIV_EXCERPTS_PER_SOURCE - 1)
            )
            fixture_id = f"{source_index:02d}-{excerpt_index + 1:03d}"
            destination = (
                document_root
                / f"arxiv_{str(source['id']).removeprefix('arXiv:').replace('.', '_')}"
                / f"excerpt_{excerpt_index + 1:03d}.pdf"
            )
            record = write_excerpt_pdf(
                destination,
                fixture_id,
                source,
                words,
                start,
                window_words,
            )
            record["path"] = destination.relative_to(output).as_posix()
            catalog.append(record)

    document_count = sum(1 for _ in document_root.rglob("*.pdf"))
    if document_count != REFERENCE_DOCUMENT_COUNT:
        raise RuntimeError(
            f"expected {REFERENCE_DOCUMENT_COUNT} reference PDFs, built {document_count}"
        )
    return catalog


def github_archive_files(archive: Path) -> dict[str, bytes]:
    files: dict[str, bytes] = {}
    with zipfile.ZipFile(archive) as repository:
        for member in repository.infolist():
            if member.is_dir() or "/" not in member.filename:
                continue
            relative_path = member.filename.split("/", 1)[1]
            files[relative_path] = repository.read(member)
    return files


def select_reference_code(files: dict[str, bytes]) -> list[str]:
    required = [GITHUB_SOURCES["merge_sort"], GITHUB_SOURCES["binary_search"]]
    excluded = {GITHUB_SOURCES["euclidean_distance"]}
    selected: list[str] = []
    seen_hashes: set[str] = set()

    candidates = required + sorted(
        path
        for path in files
        if path.endswith(".py")
        and path not in required
        and path not in excluded
        and not any(part in {"tests", "project_euler"} for part in Path(path).parts)
    )
    for path in candidates:
        payload = files[path]
        if not 400 <= len(payload) <= 40_000 or b"\x00" in payload:
            continue
        try:
            payload.decode("utf-8")
        except UnicodeDecodeError:
            continue
        content_hash = hashlib.sha256(payload).hexdigest()
        if content_hash in seen_hashes:
            continue
        selected.append(path)
        seen_hashes.add(content_hash)
        if len(selected) == REFERENCE_CODE_COUNT:
            break

    if len(selected) != REFERENCE_CODE_COUNT:
        raise RuntimeError(
            f"expected {REFERENCE_CODE_COUNT} code files, selected {len(selected)}"
        )
    if not all(path in selected for path in required):
        raise RuntimeError("required exact-match code fixtures were not selected")
    return selected


def build_code_reference_library(
    output: Path,
    archive_files: dict[str, bytes],
) -> list[str]:
    code_root = reset_generated_directory(output, "reference_library/code")
    selected = select_reference_code(archive_files)
    for relative_path in selected:
        destination = code_root / relative_path
        destination.parent.mkdir(parents=True, exist_ok=True)
        destination.write_bytes(archive_files[relative_path])
    return selected


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

The document reference library contains the two complete medical-physics papers plus
49 deterministic text windows from each paper. Each generated excerpt names its
source, word range, and CC BY 4.0 license. The 98 derived PDFs are scale fixtures, not
independent papers. Their full mapping is in `metadata/arxiv_excerpt_catalog.json`.

## GitHub code

- Repository: [TheAlgorithms/Python]({GITHUB_REPOSITORY})
- Snapshot commit: `{GITHUB_COMMIT}`
- License: MIT; the snapshot license text is stored as
  `metadata/TheAlgorithms-Python-LICENSE.md`.
- The reference library contains 100 distinct Python files selected deterministically
  from the snapshot. Their paths are in `metadata/reference_code_paths.json`.

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
            "A stored match requires an IDF-qualified continuous Token chain and at least 0.10 weighted query coverage; there is no raw similarity reporting threshold.",
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
                    "cross-domain calibration probe; no reference-library match "
                    "at or above 0.15 is expected"
                ),
            },
            {
                "query": "batch/student_04_clean_code/euclidean_distance.py",
                "scope": "reference",
                "source": None,
                "expectation": (
                    "code calibration probe; no reference-library match at or above 0.15 is expected"
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

        github_archive = staging / "TheAlgorithms-Python.zip"
        download(GITHUB_ARCHIVE_URL, github_archive)
        archive_files = github_archive_files(github_archive)

        excerpt_catalog = build_document_reference_library(output, downloaded_pdfs)
        selected_code = build_code_reference_library(output, archive_files)

        ref_blood = (
            output
            / "reference_library/documents/arxiv_1604.05171v1_blood_flow.pdf"
        )
        ref_merge = output / "reference_library/code/sorts/merge_sort.py"

        copy(ref_blood, output / "batch/student_01_exact_copy/report.pdf")
        copy(ref_merge, output / "batch/student_01_exact_copy/src/merge_sort.py")

        partial_pdf = output / "batch/student_02_partial_and_modified/report.pdf"
        selected_pages = write_partial_pdf(downloaded_pdfs["jugular_pulse"], partial_pdf)
        adapted = rewrite_identifiers(
            archive_files[GITHUB_SOURCES["binary_search"]].decode("utf-8")
        )
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
        write_text(
            output / "batch/student_04_clean_code/euclidean_distance.py",
            archive_files[GITHUB_SOURCES["euclidean_distance"]].decode("utf-8"),
        )

        write_text(
            output / "metadata/TheAlgorithms-Python-LICENSE.md",
            archive_files[GITHUB_SOURCES["license"]].decode("utf-8"),
        )
        write_text(output / "README.md", DATASET_README)
        write_text(output / "metadata/SOURCES.md", source_notes())
        write_text(
            output / "metadata/arxiv_excerpt_catalog.json",
            json.dumps(excerpt_catalog, ensure_ascii=False, indent=2) + "\n",
        )
        write_text(
            output / "metadata/reference_code_paths.json",
            json.dumps(
                {
                    "repository": GITHUB_REPOSITORY,
                    "commit": GITHUB_COMMIT,
                    "count": len(selected_code),
                    "paths": selected_code,
                },
                ensure_ascii=False,
                indent=2,
            )
            + "\n",
        )
        write_text(
            output / "metadata/expected_matches.json",
            json.dumps(expected_matches(), ensure_ascii=False, indent=2) + "\n",
        )

        source_hashes = {
            **{
                str(source["id"]): sha256(downloaded_pdfs[key])
                for key, source in ARXIV_SOURCES.items()
            },
            "TheAlgorithms/Python:archive": sha256(github_archive),
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
            "archiveUrl": GITHUB_ARCHIVE_URL,
        },
        "referenceCounts": {
            "documents": REFERENCE_DOCUMENT_COUNT,
            "code": REFERENCE_CODE_COUNT,
        },
        "derivedDocumentCount": len(excerpt_catalog),
        "selectedCodePaths": selected_code,
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
