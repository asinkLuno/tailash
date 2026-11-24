//! 平铺组卡片：订阅里每个组各一张卡（顺序即订阅顺序）。组内成员按订阅顺序展示；
//! 引用的其它组只显示为一行选项（带「组」徽标 + 当前解析链标注），不嵌套子树。
import { memo, useMemo, useState } from "react";
import type { GroupInfo, NodeInfo } from "../api";
import { delayBadgeClass, delayText } from "../delay";
import {
  effectiveNow,
  getResolvedChain,
  resolveChain,
  sortNodes,
  type SharedMaps,
  type SortMode,
} from "../nodesModel";
import NodeRow from "./NodeRow";

export { effectiveNow, resolveChain, sortNodes };
export type { SharedMaps, SortMode };

interface GroupRefRowProps {
  groupName: string;
  member: string;
  selected: boolean;
  isSelector: boolean;
  chainText: string;
  node: NodeInfo | null;
  testing: boolean;
  busy: boolean;
  onSelect: (group: string, node: string) => void;
  onTest: (name: string) => void;
}

const GroupRefRow = memo(function GroupRefRow({
  groupName,
  member,
  selected,
  isSelector,
  chainText,
  node,
  testing,
  busy,
  onSelect,
  onTest,
}: GroupRefRowProps) {
  return (
    <div
      className={`node-row-virtual flex items-center gap-2 rounded-md px-2 py-1 ${
        selected ? "bg-primary/10" : ""
      } ${isSelector ? "cursor-pointer hover:bg-base-200" : ""}`}
      onClick={isSelector ? () => onSelect(groupName, member) : undefined}
      title={isSelector ? `点击将「${groupName}」切换到组「${member}」` : undefined}
    >
      <span
        className={`w-4 shrink-0 text-sm text-primary ${selected ? "" : "invisible"}`}
      >
        ✓
      </span>
      <span className="shrink-0 truncate text-sm">{member}</span>
      <span className="badge badge-info badge-outline badge-sm">组</span>
      {chainText && (
        <span className="min-w-0 flex-1 truncate text-xs text-base-content/50">
          → {chainText}
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
            disabled={testing || busy}
            onClick={(e) => {
              e.stopPropagation();
              onTest(node.name);
            }}
            title={`测速当前解析到的节点「${node.name}」`}
          >
            {testing ? <span className="loading loading-spinner loading-xs" /> : "测速"}
          </button>
        </>
      ) : (
        <span className="w-11 shrink-0" />
      )}
    </div>
  );
});

interface BuiltinRowProps {
  groupName: string;
  member: string;
  selected: boolean;
  isSelector: boolean;
  onSelect: (group: string, node: string) => void;
}

const BuiltinRow = memo(function BuiltinRow({
  groupName,
  member,
  selected,
  isSelector,
  onSelect,
}: BuiltinRowProps) {
  return (
    <div
      className={`node-row-virtual flex items-center gap-2 rounded-md px-2 py-1 min-h-8 ${
        selected ? "bg-primary/10" : ""
      } ${isSelector ? "cursor-pointer hover:bg-base-200" : ""}`}
      onClick={isSelector ? () => onSelect(groupName, member) : undefined}
      title={isSelector ? `点击将「${groupName}」切换到「${member}」` : undefined}
    >
      <span
        className={`w-4 shrink-0 text-sm text-primary ${selected ? "" : "invisible"}`}
      >
        ✓
      </span>
      <span className="flex-1 truncate text-sm">{member}</span>
      <span className="badge badge-ghost badge-sm">内建</span>
    </div>
  );
});

interface Props {
  testAllRunning: boolean;
  group: GroupInfo;
  maps: SharedMaps;
  /** 测速中/测速失败节点名集合（NodesTab 预构建，避免每行 includes） */
  testing: Set<string>;
  failed: Set<string>;
  sort: SortMode;
  defaultOpen?: boolean;
  onSelect: (group: string, node: string) => void;
  onTest: (name: string) => void;
}

function GroupCard({
  testAllRunning,
  group,
  maps,
  testing,
  failed,
  sort,
  defaultOpen = false,
  onSelect,
  onTest,
}: Props) {
  const [open, setOpen] = useState(defaultOpen);
  const isSelector = group.kind.toLowerCase() === "selector";
  const now = effectiveNow(maps, group);
  // 卡片头：完整解析链 + 末端节点的延迟徽标（由 NodesTab 预解析，O(1) 查表）
  const header = getResolvedChain(maps, group.name);

  // 排序只作用于真节点行；组引用/内建行保持成员原位（折叠时完全跳过）
  const sortedNodes = useMemo(() => {
    if (!open) return [];
    const list: NodeInfo[] = [];
    for (const member of group.members) {
      const node = maps.nodes.get(member);
      if (node) list.push(node);
    }
    return sortNodes(list, sort);
  }, [open, group.members, maps.nodes, sort]);

  let nodeCursor = 0;

  return (
    <div className="card bg-base-100 shadow-sm">
      <div
        className="flex cursor-pointer items-center gap-2 px-3 pt-3 pb-2 select-none"
        role="button"
        tabIndex={0}
        aria-expanded={open}
        onClick={() => setOpen((v) => !v)}
        onKeyDown={(e) => {
          if (e.key === "Enter" || e.key === " ") {
            e.preventDefault();
            setOpen((v) => !v);
          }
        }}
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
            if (maps.groups.has(member)) {
              const { chain, node } = getResolvedChain(maps, member);
              return (
                <GroupRefRow
                  key={member}
                  groupName={group.name}
                  member={member}
                  selected={now === member}
                  isSelector={isSelector}
                  chainText={chain.join(" → ")}
                  node={node}
                  testing={node ? testing.has(node.name) : false}
                  busy={testAllRunning}
                  onSelect={onSelect}
                  onTest={onTest}
                />
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
                  busy={testAllRunning}
                  selected={now === display.name}
                  clickable={isSelector}
                  groupName={group.name}
                  onSelect={onSelect}
                  onTest={onTest}
                />
              );
            }
            return (
              <BuiltinRow
                key={member}
                groupName={group.name}
                member={member}
                selected={now === member}
                isSelector={isSelector}
                onSelect={onSelect}
              />
            );
          })}
        </div>
      )}
    </div>
  );
}

export default memo(GroupCard);
