//! 平铺组卡片：订阅里每个组各一张卡（顺序即订阅顺序）。组内成员按订阅顺序展示；
//! 引用的其它组只显示为一行选项（带「组」徽标 + 当前解析链标注），不嵌套子树。
import { useState } from "react";
import type { GroupInfo, NodeInfo, Snapshot } from "../api";
import { classifyDelay, delayBadgeClass, delaySortKey, delayText } from "../delay";
import NodeRow from "./NodeRow";

export type SortMode = "subscription" | "delay" | "name";

/** 组名/节点名速查表，由 NodesTab 预构建后逐卡下传 */
export interface SharedMaps {
  groups: Map<string, GroupInfo>;
  nodes: Map<string, NodeInfo>;
}

/** 卡片内的真节点按排序模式重排；组引用行始终保持在成员原位。 */
export function sortNodes(nodes: NodeInfo[], sort: SortMode): NodeInfo[] {
  if (sort === "delay") {
    return [...nodes].sort((a, b) => {
      const [ka, va] = delaySortKey(a.delay);
      const [kb, vb] = delaySortKey(b.delay);
      return ka - kb || va - vb;
    });
  }
  if (sort === "name") {
    return [...nodes].sort((a, b) => a.name.localeCompare(b.name, "zh"));
  }
  return nodes;
}

/** mihomo API 的 url-test 组类型（兼容 url-test / URLTest 两种写法） */
function isUrlTest(kind: string): boolean {
  return kind.replace(/-/g, "").toLowerCase() === "urltest";
}

/**
 * 组的「当前生效选择」：url-test 组自动选成员里实测 ping 最小的节点。
 * 没有任何实测数据时返回 null（不显示 ✓）——内核此时自己也没重算过
 * （fastNode 粘滞在旧值上），回退显示它的旧选中只会误导。
 * 其余组（Selector/Fallback 等）用内核上报的 now。
 */
export function effectiveNow(maps: SharedMaps, group: GroupInfo): string | null {
  if (isUrlTest(group.kind)) {
    let best: string | null = null;
    let bestDelay = Number.POSITIVE_INFINITY;
    for (const member of group.members) {
      const node = maps.nodes.get(member);
      if (!node || classifyDelay(node.delay) !== "measured") continue;
      if ((node.delay as number) < bestDelay) {
        best = member;
        bestDelay = node.delay as number;
      }
    }
    return best;
  }
  return group.now ?? null;
}

/** 沿组的生效选择逐层解析指向链（组 → 组 → … → 节点），防环、最多 5 跳。 */
export function resolveChain(maps: SharedMaps, groupName: string): string[] {
  const chain: string[] = [];
  let current = groupName;
  for (let depth = 0; depth < 5; depth++) {
    const group = maps.groups.get(current);
    const next = group ? effectiveNow(maps, group) : null;
    if (!next || next === current || chain.includes(next)) break;
    chain.push(next);
    current = next;
  }
  return chain;
}

/** 组引用/组卡片的生效链 + 末端节点（末端不是真节点时 node 为 null，如 DIRECT）。 */
function resolveChainInfo(
  maps: SharedMaps,
  groupName: string,
): { chain: string[]; node: NodeInfo | null } {
  const chain = resolveChain(maps, groupName);
  const node = chain.length > 0 ? (maps.nodes.get(chain[chain.length - 1]) ?? null) : null;
  return { chain, node };
}

interface Props {
  snap: Snapshot;
  group: GroupInfo;
  maps: SharedMaps;
  /** 测速中/测速失败节点名集合（NodesTab 预构建，避免每行 includes） */
  testing: Set<string>;
  failed: Set<string>;
  sort: SortMode;
  onSelect: (group: string, node: string) => void;
  onTest: (name: string) => void;
}

export default function GroupCard({
  snap,
  group,
  maps,
  testing,
  failed,
  sort,
  onSelect,
  onTest,
}: Props) {
  const [open, setOpen] = useState(true);
  const isSelector = group.kind.toLowerCase() === "selector";
  const now = effectiveNow(maps, group);
  // 卡片头：完整解析链 + 末端节点的延迟徽标（末端是 DIRECT 等内建时无徽标）
  const header = resolveChainInfo(maps, group.name);
  // 排序只作用于真节点行；组引用/内建行保持成员原位（按订阅顺序走一遍成员）
  const sortedNodes = open
    ? sortNodes(
        group.members.filter((m) => maps.nodes.has(m)).map((m) => maps.nodes.get(m)!),
        sort,
      )
    : [];
  let nodeCursor = 0;

  return (
    <div className="card bg-base-100 shadow-sm">
      <div
        className="flex items-center gap-2 px-3 pt-3 pb-2"
        role="button"
        tabIndex={0}
        onClick={() => setOpen(!open)}
        onKeyDown={(e) => e.key === "Enter" && setOpen(!open)}
      >
        <svg
          className={`h-4 w-4 shrink-0 transition-transform ${open ? "rotate-90" : ""}`}
          viewBox="0 0 24 24"
          fill="none"
          stroke="currentColor"
          strokeWidth={2}
        >
          <path d="M9 5l7 7-7 7" strokeLinecap="round" strokeLinejoin="round" />
        </svg>
        <span className="shrink-0 font-medium text-base">{group.name}</span>
        <span className="badge badge-ghost badge-sm">{group.kind}</span>
        {header.chain.length > 0 && (
          <span className="truncate text-xs text-base-content/60">
            → {header.chain.join(" → ")}
          </span>
        )}
        {header.node && (
          <span
            className={`badge badge-sm w-16 shrink-0 justify-center ${delayBadgeClass(header.node.delay)}`}
          >
            {delayText(header.node.delay)}
          </span>
        )}
        <span className="ml-auto shrink-0 text-xs text-base-content/50">
          {group.members.length} 个成员
        </span>
      </div>
      {open && (
        <div className="flex flex-col gap-1 px-3 pb-3">
          {group.members.map((member) => {
            const selected = now === member;
            const check = (
              <span
                className={`w-4 shrink-0 text-sm text-primary ${selected ? "" : "invisible"}`}
              >
                ✓
              </span>
            );
            // 组引用：单行选项 + 解析链 + 末端节点的类型/UDP/延迟/测速
            if (maps.groups.has(member)) {
              const { chain, node } = resolveChainInfo(maps, member);
              return (
                <div
                  key={member}
                  className={`flex items-center gap-2 rounded-md px-2 py-1 ${
                    selected ? "bg-primary/10" : ""
                  } ${isSelector ? "cursor-pointer hover:bg-base-200" : ""}`}
                  onClick={isSelector ? () => onSelect(group.name, member) : undefined}
                  title={
                    isSelector ? `点击将「${group.name}」切换到组「${member}」` : undefined
                  }
                >
                  {check}
                  <span className="shrink-0 truncate text-sm">{member}</span>
                  <span className="badge badge-info badge-outline badge-sm">组</span>
                  {chain.length > 0 && (
                    <span className="min-w-0 flex-1 truncate text-xs text-base-content/50">
                      → {chain.join(" → ")}
                    </span>
                  )}
                  {node ? (
                    <>
                      <span className="badge badge-ghost badge-sm">{node.kind}</span>
                      {node.udp && <span className="badge badge-ghost badge-sm">UDP</span>}
                      <span
                        className={`badge badge-sm w-16 shrink-0 justify-center ${delayBadgeClass(node.delay)}`}
                      >
                        {delayText(node.delay)}
                      </span>
                      <button
                        className="btn btn-ghost btn-xs"
                        disabled={testing.has(node.name) || snap.testAllRunning}
                        onClick={(e) => {
                          e.stopPropagation();
                          onTest(node.name);
                        }}
                        title={`测速当前解析到的节点「${node.name}」`}
                      >
                        {testing.has(node.name) ? (
                          <span className="loading loading-spinner loading-xs" />
                        ) : (
                          "测速"
                        )}
                      </button>
                    </>
                  ) : (
                    <span className="w-11 shrink-0" />
                  )}
                </div>
              );
            }
            // 真节点：排序模式下显示的是重排后的节点，选中/点击必须跟着显示的节点走，
            // 否则 ✓ 会标到成员原位置对应的行上（看起来就是"没选最小/选错节点"）
            const node = maps.nodes.get(member);
            if (node) {
              const display = sortedNodes[nodeCursor++] ?? node;
              return (
                <NodeRow
                  key={display.name}
                  node={display}
                  testing={testing.has(display.name)}
                  failed={failed.has(display.name)}
                  busy={snap.testAllRunning}
                  selected={now === display.name}
                  clickable={isSelector}
                  onSelect={() => onSelect(group.name, display.name)}
                  onTest={() => onTest(display.name)}
                />
              );
            }
            // DIRECT 等内建成员
            return (
              <div
                key={member}
                className={`flex items-center gap-2 rounded-md px-2 py-1 min-h-8 ${
                  selected ? "bg-primary/10" : ""
                } ${isSelector ? "cursor-pointer hover:bg-base-200" : ""}`}
                onClick={isSelector ? () => onSelect(group.name, member) : undefined}
                title={isSelector ? `点击将「${group.name}」切换到「${member}」` : undefined}
              >
                {check}
                <span className="flex-1 truncate text-sm">{member}</span>
                <span className="badge badge-ghost badge-sm">内建</span>
              </div>
            );
          })}
        </div>
      )}
    </div>
  );
}
