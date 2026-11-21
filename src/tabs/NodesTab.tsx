//! Tab 3：节点测速 / 排序 / 切换。平铺组卡片：订阅里每个组各一张卡（按订阅顺序），
//! 组内引用的其它组只显示为一行选项并解析标注指向链，不嵌套子树。
import { useMemo, useState } from "react";
import { sendCmd } from "../api";
import type { Snapshot } from "../api";
import { sortNodes } from "../components/GroupCard";
import type { SharedMaps, SortMode } from "../components/GroupCard";
import GroupCard from "../components/GroupCard";
import NodeRow from "../components/NodeRow";

const SORTS: { value: SortMode; label: string }[] = [
  { value: "subscription", label: "订阅顺序" },
  { value: "delay", label: "按延迟" },
  { value: "name", label: "按名称" },
];

export default function NodesTab({ snap }: { snap: Snapshot }) {
  // 支持深链指定排序：#nodes?sort=delay（默认订阅顺序）
  const [sort, setSort] = useState<SortMode>(() => {
    const match = /sort=(subscription|delay|name)/.exec(window.location.hash);
    return (match?.[1] as SortMode) ?? "subscription";
  });

  const maps: SharedMaps = useMemo(
    () => ({
      groups: new Map(snap.view.groups.map((g) => [g.name, g])),
      nodes: new Map(snap.view.nodes.map((n) => [n.name, n])),
    }),
    [snap.view],
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
          onChange={(e) => setSort(e.target.value as SortMode)}
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
        <span className="ml-auto text-sm text-base-content/50">
          {snap.view.nodes.length} 个节点 · {snap.view.groups.length} 个组
        </span>
      </div>

      {snap.view.groups.map((g) => (
        <GroupCard
          key={g.name}
          snap={snap}
          group={g}
          maps={maps}
          sort={sort}
          onSelect={(group, node) => sendCmd({ type: "select", group, node })}
          onTest={(name) => sendCmd({ type: "testNode", name })}
        />
      ))}

      <div className="card bg-base-100 shadow-sm">
        {/* 与 GroupCard 相同的内边距节奏，保证两张卡的行左缘对齐 */}
        <h2 className="px-3 pt-3 pb-2 text-base font-medium">全部节点</h2>
        <div className="flex flex-col gap-1 px-3 pb-3">
            {sortNodes(snap.view.nodes, sort).map((node) => (
              <NodeRow
                key={node.name}
                node={node}
                testing={snap.testing.includes(node.name)}
                failed={snap.failedDelays.includes(node.name)}
                clickable={false}
                onTest={() => sendCmd({ type: "testNode", name: node.name })}
              />
            ))}
        </div>
      </div>
    </div>
  );
}
