//! Tab 3：节点测速 / 排序 / 切换。平铺组卡片：订阅里每个组各一张卡（按订阅顺序），
//! 组内引用的其它组只显示为一行选项并解析标注指向链，不嵌套子树。
import { memo, useMemo, useState } from "react";
import { sendCmd } from "../api";
import type { Snapshot } from "../api";
import GroupCard from "../components/GroupCard";
import NodeRow from "../components/NodeRow";
import {
  buildNodesIndex,
  filterByGemini,
  parseSortMode,
  sortNodes,
  type SortMode,
} from "../nodesModel";

const SORTS: { value: SortMode; label: string }[] = [
  { value: "subscription", label: "订阅顺序" },
  { value: "delay", label: "按延迟" },
  { value: "name", label: "按名称" },
];

function handleSelect(group: string, node: string) {
  void sendCmd({ type: "select", group, node });
}

function handleTest(name: string) {
  void sendCmd({ type: "testNode", name });
}

/** 记住上次排序选择（深链 #nodes?sort=… 优先于它）。 */
const SORT_STORAGE_KEY = "tailash.nodes.sort";

interface NodesTabProps {
  snap: Snapshot;
  geminiOnly: boolean;
  onGeminiOnlyChange: (on: boolean) => void;
}

function GeminiFilterControl({
  geminiOnly,
  running,
  checkedCount,
  totalCount,
  onToggle,
  onRecheck,
}: {
  geminiOnly: boolean;
  running: boolean;
  checkedCount: number;
  totalCount: number;
  onToggle: (on: boolean) => void;
  onRecheck: () => void;
}) {
  return (
    <div className="flex items-center gap-1">
      <label className="label cursor-pointer gap-1.5 text-sm">
        <input
          type="checkbox"
          className="checkbox checkbox-sm"
          checked={geminiOnly}
          onChange={(e) => onToggle(e.target.checked)}
          title="只看 Gemini 可用的节点（尚无结果时自动检测）"
        />
        Gemini 可用
        {running && (
          <>
            <span className="loading loading-spinner loading-xs" />
            <span className="text-xs text-base-content/60 tabular-nums">
              {checkedCount}/{totalCount}
            </span>
          </>
        )}
      </label>
      {(geminiOnly || checkedCount > 0) && (
        <button
          className="btn btn-ghost btn-xs"
          disabled={running}
          onClick={onRecheck}
          title="重新检测全部节点的 Gemini 可用性"
        >
          重测
        </button>
      )}
    </div>
  );
}

function NodesTab({ snap, geminiOnly, onGeminiOnlyChange }: NodesTabProps) {
  const [sort, setSort] = useState<SortMode>(() => {
    const match = /sort=(subscription|delay|name)/.exec(window.location.hash);
    if (match) return match[1] as SortMode;
    return parseSortMode(localStorage.getItem(SORT_STORAGE_KEY));
  });
  function changeSort(next: SortMode) {
    setSort(next);
    localStorage.setItem(SORT_STORAGE_KEY, next);
  }
  // 有策略组时默认折叠「全部节点」，避免上千个重复 DOM 行拖慢主线程；无策略组时自动展开
  const [allOpenOverride, setAllOpenOverride] = useState<boolean | null>(null);

  const checkedCount = useMemo(() => Object.keys(snap.gemini).length, [snap.gemini]);
  // 首次勾选且尚无本轮结果时自动触发检测；已有结果时仅切筛选，重测交给显式按钮
  function toggleGeminiOnly(on: boolean) {
    onGeminiOnlyChange(on);
    if (on && checkedCount === 0 && !snap.geminiRunning) {
      void sendCmd({ type: "checkGemini" });
    }
  }
  function handleRecheckGemini() {
    if (!snap.geminiRunning) void sendCmd({ type: "checkGemini" });
  }
  const view = useMemo(
    () => (geminiOnly ? filterByGemini(snap.view, snap.gemini) : snap.view),
    [geminiOnly, snap.view, snap.gemini],
  );
  // 按过滤后的组判断：筛选把所有组都滤空时，「全部节点」要自动展开，否则页面空一片
  const allOpen = allOpenOverride ?? view.groups.length === 0;
  const maps = useMemo(() => buildNodesIndex(view), [view]);
  // testing/failedDelays 全是 O(n) 的 includes，行渲染时逐行查会放大成 O(n²)；预构建成 Set
  const testing = useMemo(() => new Set(snap.testing), [snap.testing]);
  const failed = useMemo(() => new Set(snap.failedDelays), [snap.failedDelays]);
  const sortedAllNodes = useMemo(
    () => (allOpen ? sortNodes(view.nodes, sort) : []),
    [allOpen, view.nodes, sort],
  );

  if (!snap.coreRunning) {
    return (
      <div className="alert alert-warning">
        内核未运行：启动内核后才能查看与切换节点。
      </div>
    );
  }

  return (
    <div className="flex flex-col gap-3">
      <div className="flex items-center gap-2">
        <select
          className="select select-sm w-32"
          value={sort}
          onChange={(e) => changeSort(e.target.value as SortMode)}
          title="节点排序（组顺序始终按订阅）"
        >
          {SORTS.map((s) => (
            <option key={s.value} value={s.value}>
              {s.label}
            </option>
          ))}
        </select>
        <button
          className="btn btn-primary btn-sm"
          disabled={snap.testAllRunning}
          onClick={() => sendCmd({ type: "testAll" })}
        >
          {snap.testAllRunning && <span className="loading loading-spinner loading-xs" />}
          测全部
        </button>
        <GeminiFilterControl
          geminiOnly={geminiOnly}
          running={snap.geminiRunning}
          checkedCount={checkedCount}
          totalCount={snap.view.nodes.length}
          onToggle={toggleGeminiOnly}
          onRecheck={handleRecheckGemini}
        />
        <span className="ml-auto text-sm text-base-content/50">
          {view.nodes.length} 个节点 · {view.groups.length} 个组
        </span>
      </div>

      {view.groups.map((g, index) => (
        <GroupCard
          key={g.name}
          testAllRunning={snap.testAllRunning}
          group={g}
          maps={maps}
          testing={testing}
          failed={failed}
          sort={sort}
          defaultOpen={index === 0}
          onSelect={handleSelect}
          onTest={handleTest}
        />
      ))}

      <div className="card bg-base-100 shadow-sm">
        {/* 与 GroupCard 相同的折叠头与内边距节奏，保证两张卡的行左缘对齐 */}
        <div
          className="flex cursor-pointer items-center gap-2 px-3 pt-3 pb-2 select-none"
          role="button"
          tabIndex={0}
          aria-expanded={allOpen}
          onClick={() => setAllOpenOverride(!allOpen)}
          onKeyDown={(e) => {
            if (e.key === "Enter" || e.key === " ") {
              e.preventDefault();
              setAllOpenOverride(!allOpen);
            }
          }}
        >
          <svg
            className={`h-4 w-4 shrink-0 transition-transform ${allOpen ? "rotate-90" : ""}`}
            viewBox="0 0 24 24"
            fill="none"
            stroke="currentColor"
            strokeWidth={2}
          >
            <path d="M9 5l7 7-7 7" strokeLinecap="round" strokeLinejoin="round" />
          </svg>
          <h2 className="text-base font-medium">全部节点</h2>
          <span className="ml-auto shrink-0 text-xs text-base-content/50">
            {view.nodes.length} 个节点
          </span>
        </div>
        {allOpen && (
          <div className="flex flex-col gap-1 px-3 pb-3">
            {sortedAllNodes.length === 0 && geminiOnly ? (
              <div className="py-4 text-center text-sm text-base-content/50">
                {snap.geminiRunning
                  ? `正在检测 Gemini 可用性（已测 ${checkedCount}/${snap.view.nodes.length}）…`
                  : "当前没有检测到 Gemini 可用节点，可点击「重测」再次检测。"}
              </div>
            ) : (
              sortedAllNodes.map((node) => (
                <NodeRow
                  key={node.name}
                  node={node}
                  testing={testing.has(node.name)}
                  failed={failed.has(node.name)}
                  busy={snap.testAllRunning}
                  clickable={false}
                  onTest={handleTest}
                />
              ))
            )}
          </div>
        )}
      </div>
    </div>
  );
}

export default memo(NodesTab);
