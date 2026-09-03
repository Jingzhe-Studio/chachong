import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { open } from "@tauri-apps/plugin-dialog";

type FileCategory = "document" | "code";
type ParseStatus = "pending" | "parsing" | "ready" | "failed";
type DetectionStatus = "pending" | "running" | "ready" | "failed";
type BatchView = "list" | "batch" | "workItem" | "file";

interface AlgorithmInfo {
  id: string;
  displayName: string;
  description: string;
  supportedCategories: FileCategory[];
}

interface AppInfo {
  name: string;
  algorithms: AlgorithmInfo[];
}

interface ManagedFile {
  id: number;
  name: string;
  relativePath: string;
  objectKey: string;
  parsedKey: string | null;
  contentHash: string;
  extension: string;
  sizeBytes: number;
  category: FileCategory;
  parseStatus: ParseStatus;
  parseError: string | null;
  parserId: string | null;
}

interface BatchSummary {
  id: number;
  name: string;
  workItemCount: number;
  fileCount: number;
  readyFileCount: number;
  detectionStatus: DetectionStatus;
  algorithmId: string | null;
  detectionError: string | null;
}

interface WorkItemSummary {
  id: number;
  batchId: number;
  name: string;
  fileCount: number;
  readyFileCount: number;
}

interface WorkItemFileView {
  file: ManagedFile;
  matchCount: number;
  highestSimilarity: number | null;
}

interface BatchImportSummary {
  batch: BatchSummary;
  imported: number;
  ready: number;
  failed: number;
  skipped: number;
}

interface DetectionRunSummary {
  batchId: number;
  algorithmId: string;
  queryFiles: number;
  comparedPairs: number;
  matches: number;
}

interface DetectionProgress {
  batchId: number;
  processedFiles: number;
  totalFiles: number;
  matches: number;
}

interface BatchFileProgress {
  batchId: number;
  workItemId: number;
  file: ManagedFile;
}

interface TextRange {
  start: number;
  end: number;
}

type RiskLocation =
  | { kind: "document"; startPage: number; endPage: number }
  | { kind: "code"; startLine: number; endLine: number };

interface RiskRegion {
  queryRange: TextRange;
  sourceRange: TextRange | null;
  score: number;
  queryLocation: RiskLocation | null;
  sourceLocation: RiskLocation | null;
}

interface FileMatchSummary {
  id: number;
  sourceFileId: number;
  sourceName: string;
  sourcePath: string;
  sourceKind: "batch" | "reference";
  sourceContainer: string;
  sourceWorkItemName: string | null;
  algorithmId: string;
  similarity: number;
  riskCount: number;
}

interface MatchDetail {
  resultId: number;
  queryFile: ManagedFile;
  sourceFile: ManagedFile;
  queryText: string;
  sourceText: string;
  similarity: number;
  algorithmId: string;
  riskRegions: RiskRegion[];
}

interface ReferenceLibrary {
  id: number;
  name: string;
  category: FileCategory;
}

interface ReferenceLibrarySummary {
  library: ReferenceLibrary;
  fileCount: number;
}

interface ReferenceFileProgress {
  libraryId: number;
  file: ManagedFile;
}

interface ReferenceImportSummary {
  imported: number;
  ready: number;
  failed: number;
  skipped: number;
  skippedItems: Array<{ path: string; reason: string }>;
}

let appInfo: AppInfo = { name: "查重工作台", algorithms: [] };
let batches: BatchSummary[] = [];
let batchView: BatchView = "list";
let selectedBatchId: number | null = null;
let workItems: WorkItemSummary[] = [];
let selectedWorkItem: WorkItemSummary | null = null;
let workItemFiles: WorkItemFileView[] = [];
let selectedBatchFile: ManagedFile | null = null;
let matches: FileMatchSummary[] = [];
let selectedMatchId: number | null = null;
let currentRiskRegions: RiskRegion[] = [];
let activeRiskIndex = -1;
let queryRiskMarks = new Map<number, HTMLElement>();
let sourceRiskMarks = new Map<number, HTMLElement>();
let batchImportBusy = false;
let detectionBusy = false;

let libraries: ReferenceLibrarySummary[] = [];
let referenceFiles: ManagedFile[] = [];
let selectedLibraryId: number | null = null;
let libraryDialogMode: "create" | "rename" = "create";
let referenceImportBusy = false;

window.addEventListener("DOMContentLoaded", () => {
  initializeTabs();
  initializeBatchActions();
  initializeReferenceActions();
  void initializeData();
  void listen<ReferenceFileProgress>("reference-file-status", ({ payload }) => {
    if (payload.libraryId !== selectedLibraryId) return;
    replaceReferenceFile(payload.file);
  });
  void listen<BatchFileProgress>("batch-file-status", ({ payload }) => {
    if (batchImportBusy) {
      byId("batch-import-state").textContent =
        `${statusLabel(payload.file.parseStatus)} · ${payload.file.relativePath}`;
    }
    if (payload.workItemId !== selectedWorkItem?.id) return;
    const existing = workItemFiles.find((item) => item.file.id === payload.file.id);
    if (existing) existing.file = payload.file;
    else workItemFiles.unshift({ file: payload.file, matchCount: 0, highestSimilarity: null });
    renderWorkItemFiles();
  });
  void listen<DetectionProgress>("detection-progress", ({ payload }) => {
    if (payload.batchId !== selectedBatchId) return;
    byId("detection-progress").textContent =
      `已处理 ${payload.processedFiles}/${payload.totalFiles} 个文件 · ${payload.matches} 个匹配`;
  });
});

async function initializeData(): Promise<void> {
  await Promise.all([loadAppInfo(), loadBatches(), loadLibraries()]);
}

function initializeTabs(): void {
  const tabs = document.querySelectorAll<HTMLButtonElement>("[data-tab]");
  const panels = document.querySelectorAll<HTMLElement>("[data-panel]");
  tabs.forEach((tab) => {
    tab.addEventListener("click", () => {
      const selected = tab.dataset.tab;
      tabs.forEach((item) => item.classList.toggle("is-active", item === tab));
      panels.forEach((panel) => panel.classList.toggle("is-active", panel.dataset.panel === selected));
    });
  });
}

function initializeBatchActions(): void {
  byId("import-batch").addEventListener("click", () => void selectBatchDirectory());
  byId("back-to-batches").addEventListener("click", () => showBatchView("list"));
  byId("back-to-batch").addEventListener("click", () => showBatchView("batch"));
  byId("back-to-work-item").addEventListener("click", () => showBatchView("workItem"));
  byId("rename-batch").addEventListener("click", () => void renameSelectedBatch());
  byId("delete-batch").addEventListener("click", () => void deleteSelectedBatch());
  byId("run-detection").addEventListener("click", () => void runDetection());
  byId("algorithm-select").addEventListener("change", renderAlgorithmDescription);
  byId("risk-region-select").addEventListener("change", (event) => {
    focusRiskRegion(Number((event.currentTarget as HTMLSelectElement).value));
  });
  byId("previous-risk").addEventListener("click", () => focusRiskRegion(activeRiskIndex - 1));
  byId("next-risk").addEventListener("click", () => focusRiskRegion(activeRiskIndex + 1));
}

async function loadAppInfo(): Promise<void> {
  try {
    appInfo = await invoke<AppInfo>("app_info");
    renderAlgorithmOptions();
  } catch (error) {
    showToast(errorMessage(error), true);
  }
}

async function loadBatches(preferredId?: number): Promise<void> {
  try {
    batches = await invoke<BatchSummary[]>("list_batches");
    if (preferredId !== undefined) selectedBatchId = preferredId;
    if (selectedBatchId !== null && !batches.some((batch) => batch.id === selectedBatchId)) {
      selectedBatchId = null;
    }
    renderBatchList();
    if (batchView === "batch") renderBatchDetail();
  } catch (error) {
    showToast(errorMessage(error), true);
  }
}

function renderBatchList(): void {
  const list = byId("batch-list");
  list.replaceChildren();
  byId("batch-list-empty").classList.toggle("is-hidden", batches.length !== 0);
  for (const batch of batches) {
    const button = document.createElement("button");
    button.type = "button";
    button.className = "batch-card";
    const copy = document.createElement("span");
    const name = document.createElement("strong");
    const meta = document.createElement("small");
    name.textContent = batch.name;
    meta.textContent = `${batch.workItemCount} 个作业项 · ${batch.readyFileCount}/${batch.fileCount} 个文件可用`;
    copy.append(name, meta);
    const status = document.createElement("span");
    status.className = `parse-status ${batch.detectionStatus}`;
    status.textContent = detectionStatusLabel(batch.detectionStatus);
    button.append(copy, status);
    button.addEventListener("click", () => void openBatch(batch.id));
    list.append(button);
  }
}

async function selectBatchDirectory(): Promise<void> {
  if (batchImportBusy) return;
  try {
    const selection = await open({ directory: true, multiple: false });
    if (!selection || Array.isArray(selection)) return;
    batchImportBusy = true;
    (byId("import-batch") as HTMLButtonElement).disabled = true;
    byId("batch-import-state").textContent = "正在托管并解析批次文件…";
    const summary = await invoke<BatchImportSummary>("import_batch", { path: selection });
    await loadBatches(summary.batch.id);
    await openBatch(summary.batch.id);
    const message = [`已导入 ${summary.imported} 个文件`];
    if (summary.failed) message.push(`${summary.failed} 个解析失败`);
    if (summary.skipped) message.push(`${summary.skipped} 个跳过`);
    showToast(message.join("，"), summary.failed > 0);
  } catch (error) {
    showToast(errorMessage(error), true);
  } finally {
    batchImportBusy = false;
    (byId("import-batch") as HTMLButtonElement).disabled = false;
    byId("batch-import-state").textContent = "";
  }
}

async function openBatch(batchId: number): Promise<void> {
  selectedBatchId = batchId;
  selectedWorkItem = null;
  selectedBatchFile = null;
  try {
    workItems = await invoke<WorkItemSummary[]>("list_work_items", { batchId });
    renderBatchDetail();
    renderWorkItems();
    showBatchView("batch");
  } catch (error) {
    showToast(errorMessage(error), true);
  }
}

function renderBatchDetail(): void {
  const batch = selectedBatch();
  if (!batch) return;
  byId("batch-name").textContent = batch.name;
  byId("batch-meta").textContent =
    `${batch.workItemCount} 个作业项 · ${batch.readyFileCount}/${batch.fileCount} 个文件解析成功`;
  const status = byId("batch-status");
  status.className = `parse-status ${batch.detectionStatus}`;
  status.textContent = detectionStatusLabel(batch.detectionStatus);
  status.title = batch.detectionError ?? "";
  const select = byId("algorithm-select") as HTMLSelectElement;
  if (batch.algorithmId && appInfo.algorithms.some((algorithm) => algorithm.id === batch.algorithmId)) {
    select.value = batch.algorithmId;
  }
  renderAlgorithmDescription();
}

function renderAlgorithmOptions(): void {
  const select = byId("algorithm-select") as HTMLSelectElement;
  const selected = select.value;
  select.replaceChildren();
  for (const algorithm of appInfo.algorithms) {
    const option = document.createElement("option");
    option.value = algorithm.id;
    option.textContent = algorithm.displayName;
    select.append(option);
  }
  if (appInfo.algorithms.some((algorithm) => algorithm.id === selected)) select.value = selected;
  renderAlgorithmDescription();
}

function renderAlgorithmDescription(): void {
  const id = (byId("algorithm-select") as HTMLSelectElement).value;
  byId("algorithm-description").textContent =
    appInfo.algorithms.find((item) => item.id === id)?.description ?? "";
}

function renderWorkItems(): void {
  const list = byId("work-item-list");
  list.replaceChildren();
  for (const item of workItems) {
    const button = document.createElement("button");
    button.type = "button";
    button.className = "work-item-row";
    const identity = document.createElement("span");
    const name = document.createElement("strong");
    const meta = document.createElement("small");
    name.textContent = item.name;
    meta.textContent = `${item.readyFileCount}/${item.fileCount} 个文件可用`;
    identity.append(name, meta);
    button.append(identity);
    button.addEventListener("click", () => void openWorkItem(item.id));
    list.append(button);
  }
}

async function renameSelectedBatch(): Promise<void> {
  if (detectionBusy) return;
  const batch = selectedBatch();
  if (!batch) return;
  const name = window.prompt("新的批次名称", batch.name)?.trim();
  if (!name || name === batch.name) return;
  try {
    await invoke("rename_batch", { batchId: batch.id, name });
    await loadBatches(batch.id);
  } catch (error) {
    showToast(errorMessage(error), true);
  }
}

async function deleteSelectedBatch(): Promise<void> {
  if (detectionBusy) return;
  const batch = selectedBatch();
  if (!batch) return;
  if (!window.confirm(`确定删除批次“${batch.name}”及其全部作业项和查重结果吗？`)) return;
  try {
    await invoke("delete_batch", { batchId: batch.id });
    selectedBatchId = null;
    showBatchView("list");
    await loadBatches();
    showToast("批次已删除");
  } catch (error) {
    showToast(errorMessage(error), true);
  }
}

async function runDetection(): Promise<void> {
  const batch = selectedBatch();
  const algorithmId = (byId("algorithm-select") as HTMLSelectElement).value;
  if (!batch || !algorithmId || detectionBusy) return;
  if (referenceImportBusy) {
    showToast("请等待参考库导入完成后再开始查重", true);
    return;
  }
  if (batch.readyFileCount === 0) {
    showToast("批次中没有解析成功的文件", true);
    return;
  }
  detectionBusy = true;
  setDetectionBusy(true);
  try {
    const summary = await invoke<DetectionRunSummary>("run_batch_detection", {
      batchId: batch.id,
      algorithmId,
    });
    byId("detection-progress").textContent =
      `完成 · ${summary.comparedPairs} 次精确比较 · ${summary.matches} 个匹配`;
    await loadBatches(batch.id);
    workItems = await invoke<WorkItemSummary[]>("list_work_items", { batchId: batch.id });
    renderWorkItems();
    showToast(`查重完成，发现 ${summary.matches} 个文件匹配`);
  } catch (error) {
    showToast(errorMessage(error), true);
    await loadBatches(batch.id);
  } finally {
    detectionBusy = false;
    setDetectionBusy(false);
  }
}

function setDetectionBusy(busy: boolean): void {
  (byId("run-detection") as HTMLButtonElement).disabled = busy;
  (byId("algorithm-select") as HTMLSelectElement).disabled = busy;
  (byId("rename-batch") as HTMLButtonElement).disabled = busy;
  (byId("delete-batch") as HTMLButtonElement).disabled = busy;
  (byId("import-files") as HTMLButtonElement).disabled = busy;
  (byId("import-folder") as HTMLButtonElement).disabled = busy;
  (byId("delete-library") as HTMLButtonElement).disabled = busy;
  if (busy) byId("detection-progress").textContent = "正在构建索引并检索候选…";
}

async function openWorkItem(workItemId: number): Promise<void> {
  try {
    const [item, files] = await Promise.all([
      invoke<WorkItemSummary>("get_work_item", { workItemId }),
      invoke<WorkItemFileView[]>("list_work_item_files", { workItemId }),
    ]);
    selectedWorkItem = item;
    selectedBatchId = item.batchId;
    workItemFiles = files;
    byId("work-item-name").textContent = item.name;
    byId("work-item-meta").textContent = `${item.readyFileCount}/${item.fileCount} 个文件解析成功`;
    renderWorkItemFiles();
    showBatchView("workItem");
  } catch (error) {
    showToast(errorMessage(error), true);
  }
}

function renderWorkItemFiles(): void {
  const list = byId("work-item-files");
  list.replaceChildren();
  for (const view of workItemFiles) {
    const row = document.createElement("div");
    row.className = "result-file-row";
    const openButton = document.createElement("button");
    openButton.type = "button";
    openButton.className = "result-file-main";
    openButton.disabled = view.file.parseStatus !== "ready";
    const copy = document.createElement("span");
    const name = document.createElement("strong");
    const path = document.createElement("small");
    name.textContent = view.file.name;
    path.textContent = `${view.file.relativePath} · ${categoryLabel(view.file.category)}`;
    copy.append(name, path);
    if (view.file.parseError) {
      const error = document.createElement("span");
      error.className = "file-error";
      error.textContent = view.file.parseError;
      copy.append(error);
    }
    openButton.append(copy);
    openButton.addEventListener("click", () => void openFileResult(view.file));
    const result = document.createElement("span");
    result.className = "file-result-meta";
    if (view.file.parseStatus === "ready") {
      const detectionReady = selectedBatch()?.detectionStatus === "ready";
      result.textContent = !detectionReady
        ? "等待查重"
        : view.matchCount
          ? `${view.matchCount} 个来源 · 最高 ${formatPercent(view.highestSimilarity ?? 0)}`
          : "无匹配来源";
    } else {
      const state = document.createElement("span");
      state.className = `parse-status ${view.file.parseStatus}`;
      state.textContent = statusLabel(view.file.parseStatus);
      result.append(state);
      if (view.file.parseStatus === "failed") {
        const retry = actionButton("重试解析");
        retry.addEventListener("click", () => void retryBatchFile(view));
        result.append(retry);
      }
    }
    if (view.file.parseError) result.title = view.file.parseError;
    row.append(openButton, result);
    list.append(row);
  }
}

async function retryBatchFile(view: WorkItemFileView): Promise<void> {
  if (!selectedWorkItem || detectionBusy) return;
  const workItemId = selectedWorkItem.id;
  const batchId = selectedWorkItem.batchId;
  view.file.parseStatus = "parsing";
  view.file.parseError = null;
  renderWorkItemFiles();
  try {
    await invoke<ManagedFile>("retry_batch_parse", { workItemId, fileId: view.file.id });
    await Promise.all([openWorkItem(workItemId), loadBatches(batchId)]);
  } catch (error) {
    showToast(errorMessage(error), true);
    await openWorkItem(workItemId);
  }
}

async function openFileResult(file: ManagedFile): Promise<void> {
  if (selectedBatchId === null) return;
  selectedBatchFile = file;
  selectedMatchId = null;
  byId("result-file-name").textContent = file.name;
  byId("result-file-path").textContent = file.relativePath;
  clearComparison();
  try {
    matches = await invoke<FileMatchSummary[]>("list_file_matches", {
      batchId: selectedBatchId,
      fileId: file.id,
    });
    renderMatches();
    showBatchView("file");
    if (matches[0]) await selectMatch(matches[0]);
  } catch (error) {
    showToast(errorMessage(error), true);
  }
}

function renderMatches(): void {
  const list = byId("match-list");
  list.replaceChildren();
  byId("match-count").textContent = `${matches.length} 个`;
  byId("match-list-empty").classList.toggle("is-hidden", matches.length !== 0);
  for (const match of matches) {
    const button = document.createElement("button");
    button.type = "button";
    button.className = "match-item";
    button.classList.toggle("is-active", match.id === selectedMatchId);
    const heading = document.createElement("span");
    const name = document.createElement("strong");
    const score = document.createElement("b");
    name.textContent = match.sourceName;
    score.textContent = formatPercent(match.similarity);
    heading.append(name, score);
    const source = document.createElement("small");
    source.textContent = match.sourceKind === "reference"
      ? `参考库 · ${match.sourceContainer}`
      : `批次内 · ${match.sourceWorkItemName ?? match.sourceContainer}`;
    const path = document.createElement("small");
    path.textContent = match.sourcePath;
    button.append(heading, source, path);
    button.addEventListener("click", () => void selectMatch(match));
    list.append(button);
  }
}

async function selectMatch(match: FileMatchSummary): Promise<void> {
  if (!selectedBatchFile) return;
  selectedMatchId = match.id;
  renderMatches();
  try {
    const detail = await invoke<MatchDetail>("get_match_detail", {
      queryFileId: selectedBatchFile.id,
      resultId: match.id,
    });
    if (selectedMatchId !== match.id) return;
    renderComparison(detail, match);
  } catch (error) {
    showToast(errorMessage(error), true);
  }
}

function renderComparison(detail: MatchDetail, match: FileMatchSummary): void {
  byId("comparison-empty").classList.add("is-hidden");
  byId("comparison-content").classList.remove("is-hidden");
  byId("comparison-title").textContent = match.sourceKind === "reference"
    ? `参考库：${match.sourceContainer}`
    : `批次内：${match.sourceWorkItemName ?? match.sourceContainer}`;
  byId("comparison-score").textContent = formatPercent(detail.similarity);
  byId("query-text-title").textContent = detail.queryFile.name;
  byId("source-text-title").textContent = detail.sourceFile.name;
  currentRiskRegions = detail.riskRegions;
  byId("risk-summary").textContent = `${currentRiskRegions.length} 处高风险区域`;
  renderRiskNavigation();
  queryRiskMarks = renderHighlightedText(
    byId("query-text"),
    detail.queryText,
    detail.riskRegions.map((region, index) => ({ range: region.queryRange, index })),
  );
  sourceRiskMarks = renderHighlightedText(
    byId("source-text"),
    detail.sourceText,
    detail.riskRegions.flatMap((region, index) =>
      region.sourceRange ? [{ range: region.sourceRange, index }] : []
    ),
  );
  if (currentRiskRegions.length) focusRiskRegion(0, false);
}

function clearComparison(): void {
  byId("comparison-empty").classList.remove("is-hidden");
  byId("comparison-content").classList.add("is-hidden");
  byId("query-text").replaceChildren();
  byId("source-text").replaceChildren();
  byId("risk-region-select").replaceChildren();
  currentRiskRegions = [];
  activeRiskIndex = -1;
  queryRiskMarks.clear();
  sourceRiskMarks.clear();
}

function renderRiskNavigation(): void {
  const select = byId("risk-region-select") as HTMLSelectElement;
  select.replaceChildren();
  currentRiskRegions.forEach((region, index) => {
    const option = document.createElement("option");
    option.value = String(index);
    option.textContent = `第 ${index + 1} 处：当前${formatRiskLocation(region.queryLocation)}；来源${formatRiskLocation(region.sourceLocation)}`;
    select.append(option);
  });
  const hasRegions = currentRiskRegions.length > 0;
  (byId("previous-risk") as HTMLButtonElement).disabled = !hasRegions;
  (byId("next-risk") as HTMLButtonElement).disabled = currentRiskRegions.length < 2;
  select.disabled = !hasRegions;
}

function formatRiskLocation(location: RiskLocation | null): string {
  if (!location) return "位置未知";
  if (location.kind === "document") {
    return location.startPage === location.endPage
      ? `第 ${location.startPage} 页`
      : `第 ${location.startPage} 至 ${location.endPage} 页`;
  }
  return location.startLine === location.endLine
    ? `第 ${location.startLine} 行`
    : `第 ${location.startLine} 至 ${location.endLine} 行`;
}

function focusRiskRegion(index: number, smooth = true): void {
  if (index < 0 || index >= currentRiskRegions.length) return;
  activeRiskIndex = index;
  const select = byId("risk-region-select") as HTMLSelectElement;
  select.value = String(index);
  (byId("previous-risk") as HTMLButtonElement).disabled = index === 0;
  (byId("next-risk") as HTMLButtonElement).disabled = index === currentRiskRegions.length - 1;

  byId("query-text").querySelectorAll("mark.is-focused")
    .forEach((mark) => mark.classList.remove("is-focused"));
  byId("source-text").querySelectorAll("mark.is-focused")
    .forEach((mark) => mark.classList.remove("is-focused"));

  const queryMark = queryRiskMarks.get(index);
  const sourceMark = sourceRiskMarks.get(index);
  if (queryMark) {
    queryMark.classList.add("is-focused");
    scrollMarkIntoView(byId("query-text"), queryMark, smooth);
  }
  if (sourceMark) {
    sourceMark.classList.add("is-focused");
    scrollMarkIntoView(byId("source-text"), sourceMark, smooth);
  }
}

function scrollMarkIntoView(container: HTMLElement, mark: HTMLElement, smooth: boolean): void {
  const containerRect = container.getBoundingClientRect();
  const markRect = mark.getBoundingClientRect();
  const top = container.scrollTop + markRect.top - containerRect.top - container.clientHeight / 3;
  container.scrollTo({ top: Math.max(0, top), behavior: smooth ? "smooth" : "auto" });
}

interface IndexedTextRange {
  range: TextRange;
  index: number;
}

interface MergedTextRange extends TextRange {
  indexes: number[];
}

function renderHighlightedText(
  container: HTMLElement,
  text: string,
  ranges: IndexedTextRange[],
): Map<number, HTMLElement> {
  container.replaceChildren();
  const marks = new Map<number, HTMLElement>();
  const normalized = mergeIndexedRanges(ranges);
  const boundaries = byteBoundaries(text, normalized.flatMap((range) => [range.start, range.end]));
  let cursor = 0;
  for (const range of normalized) {
    const start = boundaries.get(range.start) ?? cursor;
    const end = boundaries.get(range.end) ?? start;
    if (start > cursor) container.append(document.createTextNode(text.slice(cursor, start)));
    if (end > start) {
      const mark = document.createElement("mark");
      mark.textContent = text.slice(start, end);
      mark.title = range.indexes.map((index) => `高风险区域 ${index + 1}`).join("、");
      container.append(mark);
      range.indexes.forEach((index) => {
        if (!marks.has(index)) marks.set(index, mark);
      });
    }
    cursor = Math.max(cursor, end);
  }
  if (cursor < text.length) container.append(document.createTextNode(text.slice(cursor)));
  return marks;
}

function mergeIndexedRanges(ranges: IndexedTextRange[]): MergedTextRange[] {
  const sorted = ranges
    .filter(({ range }) => range.end > range.start)
    .map(({ range, index }) => ({ ...range, indexes: [index] }))
    .sort((left, right) => left.start - right.start || left.end - right.end);
  const merged: MergedTextRange[] = [];
  for (const range of sorted) {
    const previous = merged[merged.length - 1];
    if (previous && range.start <= previous.end) {
      previous.end = Math.max(previous.end, range.end);
      previous.indexes.push(...range.indexes);
    } else {
      merged.push(range);
    }
  }
  return merged;
}

function byteBoundaries(text: string, requested: number[]): Map<number, number> {
  const targets = [...new Set(requested)].sort((left, right) => left - right);
  const result = new Map<number, number>();
  const encoder = new TextEncoder();
  let targetIndex = 0;
  let byteIndex = 0;
  let stringIndex = 0;
  while (targets[targetIndex] === 0) result.set(targets[targetIndex++], 0);
  for (const character of text) {
    const nextByte = byteIndex + encoder.encode(character).length;
    const nextString = stringIndex + character.length;
    while (targetIndex < targets.length && targets[targetIndex] <= nextByte) {
      const target = targets[targetIndex++];
      result.set(target, target === byteIndex ? stringIndex : nextString);
    }
    byteIndex = nextByte;
    stringIndex = nextString;
  }
  while (targetIndex < targets.length) result.set(targets[targetIndex++], text.length);
  return result;
}

function showBatchView(view: BatchView): void {
  batchView = view;
  const ids: Record<BatchView, string> = {
    list: "batch-list-view",
    batch: "batch-detail-view",
    workItem: "work-item-view",
    file: "file-result-view",
  };
  for (const [candidate, id] of Object.entries(ids)) {
    byId(id).classList.toggle("is-hidden", candidate !== view);
  }
}

function selectedBatch(): BatchSummary | null {
  return batches.find((batch) => batch.id === selectedBatchId) ?? null;
}

function detectionStatusLabel(status: DetectionStatus): string {
  return { pending: "未查重", running: "查重中", ready: "已完成", failed: "查重失败" }[status];
}

function initializeReferenceActions(): void {
  byId("create-library").addEventListener("click", () => openLibraryDialog("create"));
  byId("rename-library").addEventListener("click", () => openLibraryDialog("rename"));
  byId("delete-library").addEventListener("click", () => void deleteSelectedLibrary());
  byId("import-files").addEventListener("click", () => void selectAndImportReferences(false));
  byId("import-folder").addEventListener("click", () => void selectAndImportReferences(true));
  byId("close-library-dialog").addEventListener("click", closeLibraryDialog);
  byId("cancel-library-dialog").addEventListener("click", closeLibraryDialog);
  byId("library-form").addEventListener("submit", (event) => {
    event.preventDefault();
    void saveLibrary();
  });
}

async function loadLibraries(preferredId?: number): Promise<void> {
  try {
    libraries = await invoke<ReferenceLibrarySummary[]>("list_reference_libraries");
    const nextId = preferredId ?? selectedLibraryId;
    selectedLibraryId = libraries.some(({ library }) => library.id === nextId)
      ? nextId
      : (libraries[0]?.library.id ?? null);
    renderLibraries();
    if (selectedLibraryId !== null) await loadReferenceFiles(selectedLibraryId);
    else {
      referenceFiles = [];
      renderLibraryDetail();
    }
  } catch (error) {
    showToast(errorMessage(error), true);
  }
}

async function loadReferenceFiles(libraryId: number): Promise<void> {
  try {
    referenceFiles = await invoke<ManagedFile[]>("list_reference_files", { libraryId });
    renderLibraryDetail();
    renderReferenceFiles();
  } catch (error) {
    showToast(errorMessage(error), true);
  }
}

function renderLibraries(): void {
  const list = byId("library-list");
  list.replaceChildren();
  byId("library-list-empty").classList.toggle("is-hidden", libraries.length !== 0);
  for (const summary of libraries) {
    const button = document.createElement("button");
    button.type = "button";
    button.className = "library-item";
    button.classList.toggle("is-active", summary.library.id === selectedLibraryId);
    const copy = document.createElement("span");
    const name = document.createElement("strong");
    const meta = document.createElement("small");
    name.textContent = summary.library.name;
    meta.textContent = `${categoryLabel(summary.library.category)} · ${summary.fileCount} 个文件`;
    copy.append(name, meta);
    button.append(copy);
    button.addEventListener("click", () => {
      selectedLibraryId = summary.library.id;
      renderLibraries();
      void loadReferenceFiles(summary.library.id);
    });
    list.append(button);
  }
}

function renderLibraryDetail(): void {
  const summary = selectedLibrary();
  byId("library-unselected").classList.toggle("is-hidden", summary !== null);
  byId("library-content").classList.toggle("is-hidden", summary === null);
  if (!summary) return;
  byId("library-name").textContent = summary.library.name;
  byId("library-category").textContent = categoryLabel(summary.library.category);
  byId("library-category").className = `category-badge ${summary.library.category}`;
  byId("library-file-count").textContent = `${referenceFiles.length} 个参考文件`;
}

function renderReferenceFiles(): void {
  const body = byId("reference-files");
  body.replaceChildren();
  byId("reference-files-empty").classList.toggle("is-hidden", referenceFiles.length !== 0);
  renderLibraryDetail();
  for (const file of referenceFiles) {
    const row = document.createElement("tr");
    const identity = document.createElement("td");
    const name = document.createElement("strong");
    const path = document.createElement("small");
    name.textContent = file.name;
    path.textContent = file.relativePath;
    identity.append(name, path);
    if (file.parseError) {
      const error = document.createElement("span");
      error.className = "file-error";
      error.textContent = file.parseError;
      identity.append(error);
    }
    const size = document.createElement("td");
    size.textContent = formatBytes(file.sizeBytes);
    const statusCell = document.createElement("td");
    const status = document.createElement("span");
    status.className = `parse-status ${file.parseStatus}`;
    status.textContent = statusLabel(file.parseStatus);
    statusCell.append(status);
    const actions = document.createElement("td");
    actions.className = "file-actions";
    if (file.parseStatus === "failed") {
      const retry = actionButton("重试");
      retry.addEventListener("click", () => void retryReferenceFile(file));
      actions.append(retry);
    }
    const remove = actionButton("删除", true);
    remove.addEventListener("click", () => void deleteReferenceFile(file));
    actions.append(remove);
    row.append(identity, size, statusCell, actions);
    body.append(row);
  }
}

function openLibraryDialog(mode: "create" | "rename"): void {
  const dialog = byId("library-dialog") as HTMLDialogElement;
  const name = byId("library-name-input") as HTMLInputElement;
  const category = byId("library-category-input") as HTMLSelectElement;
  libraryDialogMode = mode;
  byId("library-form-error").textContent = "";
  byId("library-dialog-title").textContent = mode === "create" ? "新建参考库" : "重命名参考库";
  if (mode === "rename") {
    const summary = selectedLibrary();
    if (!summary) return;
    name.value = summary.library.name;
    category.value = summary.library.category;
    category.disabled = true;
  } else {
    name.value = "";
    category.value = "document";
    category.disabled = false;
  }
  dialog.showModal();
  name.focus();
}

function closeLibraryDialog(): void {
  (byId("library-dialog") as HTMLDialogElement).close();
}

async function saveLibrary(): Promise<void> {
  const name = (byId("library-name-input") as HTMLInputElement).value.trim();
  const category = (byId("library-category-input") as HTMLSelectElement).value as FileCategory;
  const errorElement = byId("library-form-error");
  if (!name) {
    errorElement.textContent = "请输入参考库名称";
    return;
  }
  try {
    let library: ReferenceLibrary;
    if (libraryDialogMode === "create") {
      library = await invoke<ReferenceLibrary>("create_reference_library", { name, category });
    } else {
      if (selectedLibraryId === null) return;
      library = await invoke<ReferenceLibrary>("rename_reference_library", {
        libraryId: selectedLibraryId,
        name,
      });
    }
    closeLibraryDialog();
    await loadLibraries(library.id);
  } catch (error) {
    errorElement.textContent = errorMessage(error);
  }
}

async function deleteSelectedLibrary(): Promise<void> {
  if (detectionBusy) {
    showToast("请等待当前查重完成", true);
    return;
  }
  const summary = selectedLibrary();
  if (!summary) return;
  if (!window.confirm(`确定删除参考库“${summary.library.name}”及其全部文件吗？`)) return;
  try {
    await invoke("delete_reference_library", { libraryId: summary.library.id });
    selectedLibraryId = null;
    await loadLibraries();
    await refreshBatchesAfterReferenceChange();
    showToast("参考库已删除；相关查重来源已同步清理");
  } catch (error) {
    showToast(errorMessage(error), true);
  }
}

async function selectAndImportReferences(directory: boolean): Promise<void> {
  if (selectedLibraryId === null || referenceImportBusy) return;
  if (detectionBusy) {
    showToast("请等待当前查重完成", true);
    return;
  }
  try {
    const selection = await open({ directory, multiple: true });
    if (!selection) return;
    const paths = Array.isArray(selection) ? selection : [selection];
    await importReferencePaths(paths);
  } catch (error) {
    showToast(errorMessage(error), true);
  }
}

async function importReferencePaths(paths: string[]): Promise<void> {
  if (selectedLibraryId === null) return;
  const libraryId = selectedLibraryId;
  setReferenceImportBusy(true);
  try {
    const summary = await invoke<ReferenceImportSummary>("import_reference_paths", {
      libraryId,
      paths,
    });
    await loadLibraries(libraryId);
    await refreshBatchesAfterReferenceChange();
    const parts = [`导入 ${summary.imported} 个`];
    if (summary.failed) parts.push(`${summary.failed} 个失败`);
    if (summary.skipped) parts.push(`${summary.skipped} 个跳过`);
    showToast(parts.join("，"), summary.failed > 0);
  } catch (error) {
    showToast(errorMessage(error), true);
  } finally {
    setReferenceImportBusy(false);
  }
}

async function retryReferenceFile(file: ManagedFile): Promise<void> {
  if (selectedLibraryId === null || detectionBusy) return;
  file.parseStatus = "parsing";
  file.parseError = null;
  renderReferenceFiles();
  try {
    replaceReferenceFile(await invoke<ManagedFile>("retry_reference_parse", {
      libraryId: selectedLibraryId,
      fileId: file.id,
    }));
    await refreshBatchesAfterReferenceChange();
  } catch (error) {
    showToast(errorMessage(error), true);
    await loadReferenceFiles(selectedLibraryId);
  }
}

async function deleteReferenceFile(file: ManagedFile): Promise<void> {
  if (selectedLibraryId === null || detectionBusy) return;
  if (!window.confirm(`确定从参考库中删除“${file.name}”吗？`)) return;
  try {
    await invoke("delete_reference_file", {
      libraryId: selectedLibraryId,
      fileId: file.id,
    });
    referenceFiles = referenceFiles.filter((item) => item.id !== file.id);
    renderReferenceFiles();
    await loadLibraries(selectedLibraryId);
    await refreshBatchesAfterReferenceChange();
  } catch (error) {
    showToast(errorMessage(error), true);
  }
}

function replaceReferenceFile(file: ManagedFile): void {
  const index = referenceFiles.findIndex((item) => item.id === file.id);
  if (index === -1) referenceFiles.unshift(file);
  else referenceFiles[index] = file;
  renderReferenceFiles();
}

async function refreshBatchesAfterReferenceChange(): Promise<void> {
  await loadBatches();
  if (batchView === "workItem" && selectedWorkItem) {
    await openWorkItem(selectedWorkItem.id);
  } else if (batchView === "file") {
    matches = [];
    selectedMatchId = null;
    renderMatches();
    clearComparison();
  }
}

function setReferenceImportBusy(busy: boolean): void {
  referenceImportBusy = busy;
  for (const id of ["import-files", "import-folder"]) {
    (byId(id) as HTMLButtonElement).disabled = busy;
  }
  byId("import-state").textContent = busy ? "正在托管并解析文件…" : "";
}

function selectedLibrary(): ReferenceLibrarySummary | null {
  return libraries.find(({ library }) => library.id === selectedLibraryId) ?? null;
}

function actionButton(label: string, danger = false): HTMLButtonElement {
  const button = document.createElement("button");
  button.type = "button";
  button.className = danger ? "text-action danger" : "text-action";
  button.textContent = label;
  return button;
}

function categoryLabel(category: FileCategory): string {
  return category === "document" ? "文档库" : "代码库";
}

function statusLabel(status: ParseStatus): string {
  return { pending: "等待解析", parsing: "解析中", ready: "可用", failed: "解析失败" }[status];
}

function formatBytes(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
  return `${(bytes / 1024 / 1024).toFixed(1)} MB`;
}

function formatPercent(value: number): string {
  return `${Math.round(value * 100)}%`;
}

function errorMessage(error: unknown): string {
  if (typeof error === "string") return error;
  if (error instanceof Error) return error.message;
  return "操作失败，请稍后重试";
}

let toastTimer: number | undefined;
function showToast(message: string, danger = false): void {
  const toast = byId("toast");
  toast.textContent = message;
  toast.className = `toast is-visible${danger ? " is-danger" : ""}`;
  window.clearTimeout(toastTimer);
  toastTimer = window.setTimeout(() => toast.classList.remove("is-visible"), 4200);
}

function byId(id: string): HTMLElement {
  const element = document.getElementById(id);
  if (!element) throw new Error(`Missing element #${id}`);
  return element;
}
