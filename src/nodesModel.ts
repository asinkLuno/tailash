//! 节点/组的纯计算模型：排序、url-test 生效项推导、指向链预解析。
//! 从视图组件中抽离，保证同一份 ProxyView 只构建一次速查表与解析链，避免渲染期 O(G·N) 重复扫描。
import type { GroupInfo, NodeInfo, ProxyView } from "./api.ts";
import { classifyDelay, delaySortKey } from "./delay.ts";

export type SortMode = "subscription" | "delay" | "name";

/** 把持久化/深链来的字符串解析成排序模式；非法值一律回退订阅顺序。 */
export function parseSortMode(value: string | null | undefined): SortMode {
  return value === "delay" || value === "name" ? value : "subscription";
}

export interface ResolvedChain {
  chain: string[];
  node: NodeInfo | null;
}

/** 组名/节点名速查表 + 预解析好的每组生效项与指向链，由 NodesTab 按 view 预构建。 */
export interface SharedMaps {
  groups: Map<string, GroupInfo>;
  nodes: Map<string, NodeInfo>;
  effectiveNows: Map<string, string | null>;
  resolved: Map<string, ResolvedChain>;
}

/** 复用中文排序器，避免 sort 比较器内逐次构造 Intl.Collator。 */
const zhCollator = new Intl.Collator("zh");

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
    return [...nodes].sort((a, b) => zhCollator.compare(a.name, b.name));
  }
  return nodes;
}

/** mihomo API 的 url-test 组类型（兼容 url-test / URLTest 两种写法） */
function isUrlTest(kind: string): boolean {
  return kind.replace(/-/g, "").toLowerCase() === "urltest";
}

function computeEffectiveNow(
  nodes: Map<string, NodeInfo>,
  group: GroupInfo,
): string | null {
  if (isUrlTest(group.kind)) {
    let best: string | null = null;
    let bestDelay = Number.POSITIVE_INFINITY;
    for (const member of group.members) {
      const node = nodes.get(member);
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

/**
 * 组的「当前生效选择」：url-test 组自动选成员里实测 ping 最小的节点。
 * 没有任何实测数据时返回 null（不显示 ✓）——内核此时自己也没重算过
 * （fastNode 粘滞在旧值上），回退显示它的旧选中只会误导。
 * 其余组（Selector/Fallback 等）用内核上报的 now。
 */
export function effectiveNow(
  maps: Pick<SharedMaps, "nodes" | "effectiveNows">,
  group: GroupInfo,
): string | null {
  const cached = maps.effectiveNows.get(group.name);
  if (cached !== undefined) return cached;
  return computeEffectiveNow(maps.nodes, group);
}

/** 沿组的生效选择逐层解析指向链（组 → 组 → … → 节点），防环、最多 5 跳。 */
export function resolveChain(
  maps: Pick<SharedMaps, "groups" | "nodes" | "effectiveNows">,
  groupName: string,
): string[] {
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

const EMPTY_RESOLVED: ResolvedChain = { chain: [], node: null };

export function getResolvedChain(maps: SharedMaps, groupName: string): ResolvedChain {
  return maps.resolved.get(groupName) ?? EMPTY_RESOLVED;
}

/** 为整份 ProxyView 一次性构建组/节点索引与全部组的解析链。 */
export function buildNodesIndex(view: ProxyView): SharedMaps {
  const groups = new Map<string, GroupInfo>();
  for (const g of view.groups) {
    groups.set(g.name, g);
  }
  const nodes = new Map<string, NodeInfo>();
  for (const n of view.nodes) {
    nodes.set(n.name, n);
  }
  const effectiveNows = new Map<string, string | null>();
  for (const g of view.groups) {
    effectiveNows.set(g.name, computeEffectiveNow(nodes, g));
  }
  const resolved = new Map<string, ResolvedChain>();
  const partial = { groups, nodes, effectiveNows };
  for (const g of view.groups) {
    const chain = resolveChain(partial, g.name);
    const node =
      chain.length > 0 ? (nodes.get(chain[chain.length - 1]) ?? null) : null;
    resolved.set(g.name, { chain, node });
  }
  return { groups, nodes, effectiveNows, resolved };
}
