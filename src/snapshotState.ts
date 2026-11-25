//! 快照引用稳定与事件合并：
//! 1. reconcileSnapshot 在数据入口对比新旧快照，对未变化的节点/组/数组复用旧引用，
//!    让 React.memo 与 useMemo 仅重渲染真正改变的少数行；
//! 2. 拆分 traffic 与非 traffic 变更判断，使每秒网速刷新不再触发节点页重渲染；
//! 3. createLatestCoalescer 把短时间内的突发事件合并为单次最新值派发，避免主线程积压。
import type {
  GroupInfo,
  NodeInfo,
  ProxyView,
  Snapshot,
  SubStatus,
  TrafficPoint,
  TrafficSample,
} from "./api";

export interface TrafficState {
  traffic: TrafficSample;
  trafficHistory: TrafficPoint[];
}

function stringArraysEqual(a: readonly string[], b: readonly string[]): boolean {
  if (a === b) return true;
  if (a.length !== b.length) return false;
  for (let i = 0; i < a.length; i++) {
    if (a[i] !== b[i]) return false;
  }
  return true;
}

function reconcileStringArray(prev: string[], next: string[]): string[] {
  return stringArraysEqual(prev, next) ? prev : next;
}

function nodeEqual(a: NodeInfo, b: NodeInfo): boolean {
  return (
    a.name === b.name &&
    a.kind === b.kind &&
    a.delay === b.delay &&
    a.udp === b.udp
  );
}

function groupEqual(a: GroupInfo, b: GroupInfo): boolean {
  return (
    a.name === b.name &&
    a.kind === b.kind &&
    a.now === b.now &&
    stringArraysEqual(a.members, b.members)
  );
}

function subEqual(a: SubStatus, b: SubStatus): boolean {
  return (
    a.id === b.id &&
    a.name === b.name &&
    a.url === b.url &&
    a.active === b.active &&
    a.nodeCount === b.nodeCount &&
    a.updatedAt === b.updatedAt
  );
}

function trafficSampleEqual(a: TrafficSample, b: TrafficSample): boolean {
  return (
    a.up === b.up &&
    a.down === b.down &&
    a.upTotal === b.upTotal &&
    a.downTotal === b.downTotal
  );
}

function trafficHistoryEqual(a: TrafficPoint[], b: TrafficPoint[]): boolean {
  if (a === b) return true;
  if (a.length !== b.length) return false;
  for (let i = 0; i < a.length; i++) {
    if (a[i].up !== b[i].up || a[i].down !== b[i].down) return false;
  }
  return true;
}

function reconcileArrayByIndex<T>(
  prev: T[],
  next: T[],
  equal: (a: T, b: T) => boolean,
): T[] {
  if (prev === next) return prev;
  let allSame = prev.length === next.length;
  const out = new Array<T>(next.length);
  for (let i = 0; i < next.length; i++) {
    const prevItem = prev[i];
    const nextItem = next[i];
    if (prevItem !== undefined && equal(prevItem, nextItem)) {
      out[i] = prevItem;
    } else {
      allSame = false;
      out[i] = nextItem;
    }
  }
  return allSame ? prev : out;
}

function reconcileNodes(prev: NodeInfo[], next: NodeInfo[]): NodeInfo[] {
  if (prev === next) return prev;
  // 快路径：多数推送（如测速期间）节点顺序完全不变，按索引直比 O(n) 且零 Map 分配；
  // 顺序不同或增删时再按 name 查旧对象复用引用。
  let prevByName: Map<string, NodeInfo> | null = null;
  let allSame = prev.length === next.length;
  const out = new Array<NodeInfo>(next.length);
  for (let i = 0; i < next.length; i++) {
    const nextNode = next[i];
    let prevNode: NodeInfo | undefined = prev[i];
    if (!prevNode || prevNode.name !== nextNode.name) {
      if (!prevByName) {
        prevByName = new Map(prev.map((n) => [n.name, n]));
      }
      prevNode = prevByName.get(nextNode.name);
      allSame = false;
    }
    if (prevNode && nodeEqual(prevNode, nextNode)) {
      out[i] = prevNode;
    } else {
      allSame = false;
      out[i] = nextNode;
    }
  }
  return allSame ? prev : out;
}

function reconcileView(prev: ProxyView, next: ProxyView): ProxyView {
  if (prev === next) return prev;
  const nodes = reconcileNodes(prev.nodes, next.nodes);
  const groups = reconcileArrayByIndex(prev.groups, next.groups, groupEqual);
  if (nodes === prev.nodes && groups === prev.groups) return prev;
  return { nodes, groups };
}

/** 合并新快照并尽量沿用未变化字段的旧引用。 */
export function reconcileSnapshot(prev: Snapshot, next: Snapshot): Snapshot {
  if (prev === next) return prev;
  const traffic = trafficSampleEqual(prev.traffic, next.traffic) ? prev.traffic : next.traffic;
  const trafficHistory = trafficHistoryEqual(prev.trafficHistory, next.trafficHistory)
    ? prev.trafficHistory
    : next.trafficHistory;
  const view = reconcileView(prev.view, next.view);
  const subs = reconcileArrayByIndex(prev.subs, next.subs, subEqual);
  const testing = reconcileStringArray(prev.testing, next.testing);
  const failedDelays = reconcileStringArray(prev.failedDelays, next.failedDelays);

  const sameScalars =
    prev.coreRunning === next.coreRunning &&
    prev.capMissing === next.capMissing &&
    prev.systemdAvailable === next.systemdAvailable &&
    prev.mode === next.mode &&
    prev.tunEnabled === next.tunEnabled &&
    prev.testAllRunning === next.testAllRunning &&
    prev.autoUpdateHours === next.autoUpdateHours &&
    prev.message === next.message;

  if (
    sameScalars &&
    traffic === prev.traffic &&
    trafficHistory === prev.trafficHistory &&
    view === prev.view &&
    subs === prev.subs &&
    testing === prev.testing &&
    failedDelays === prev.failedDelays
  ) {
    return prev;
  }

  return {
    coreRunning: next.coreRunning,
    capMissing: next.capMissing,
    systemdAvailable: next.systemdAvailable,
    mode: next.mode,
    tunEnabled: next.tunEnabled,
    traffic,
    trafficHistory,
    view,
    subs,
    autoUpdateHours: next.autoUpdateHours,
    testing,
    testAllRunning: next.testAllRunning,
    message: next.message,
    failedDelays,
  };
}

/** 仅检测流量/历史曲线是否变化。 */
export function hasTrafficChanged(prev: Snapshot, next: Snapshot): boolean {
  return prev.traffic !== next.traffic || prev.trafficHistory !== next.trafficHistory;
}

/** 检测除流量外的业务状态是否变化（用于隔离节点页与高频网速推送）。 */
export function hasAppStateChanged(prev: Snapshot, next: Snapshot): boolean {
  return (
    prev.coreRunning !== next.coreRunning ||
    prev.capMissing !== next.capMissing ||
    prev.systemdAvailable !== next.systemdAvailable ||
    prev.mode !== next.mode ||
    prev.tunEnabled !== next.tunEnabled ||
    prev.view !== next.view ||
    prev.subs !== next.subs ||
    prev.autoUpdateHours !== next.autoUpdateHours ||
    prev.testing !== next.testing ||
    prev.testAllRunning !== next.testAllRunning ||
    prev.message !== next.message ||
    prev.failedDelays !== next.failedDelays
  );
}

export interface LatestCoalescer<T> {
  push: (value: T) => void;
  cancel: () => void;
}

/** 同一调度周期内多次 push 只保留最后一次值，防止高负载下 IPC 事件在主线程积压。 */
export function createLatestCoalescer<T>(
  onFlush: (value: T) => void,
  schedule: (cb: () => void) => number,
  cancelSchedule: (id: number) => void,
): LatestCoalescer<T> {
  let scheduledId: number | null = null;
  let hasLatest = false;
  let latest: T | undefined;

  return {
    push(value: T) {
      latest = value;
      hasLatest = true;
      if (scheduledId !== null) return;
      scheduledId = schedule(() => {
        scheduledId = null;
        if (!hasLatest) return;
        const next = latest as T;
        latest = undefined;
        hasLatest = false;
        onFlush(next);
      });
    },
    cancel() {
      if (scheduledId !== null) {
        cancelSchedule(scheduledId);
        scheduledId = null;
      }
      hasLatest = false;
      latest = undefined;
    },
  };
}

export interface SnapshotStore {
  getApp: () => Snapshot;
  getTraffic: () => TrafficState;
  apply: (raw: Snapshot) => void;
  subscribeApp: (listener: (snap: Snapshot) => void) => () => void;
  subscribeTraffic: (listener: (traffic: TrafficState) => void) => () => void;
}

/** 共享快照仓库：分别派发业务状态与高频流量状态，避免每秒网速波动拖累节点页重渲染。 */
export function createSnapshotStore(initial: Snapshot): SnapshotStore {
  let current = initial;
  let appSnap = initial;
  let trafficState: TrafficState = {
    traffic: initial.traffic,
    trafficHistory: initial.trafficHistory,
  };
  const appListeners = new Set<(snap: Snapshot) => void>();
  const trafficListeners = new Set<(traffic: TrafficState) => void>();

  return {
    getApp: () => appSnap,
    getTraffic: () => trafficState,
    apply(raw: Snapshot) {
      const merged = reconcileSnapshot(current, raw);
      if (merged === current) return;
      const appChanged = hasAppStateChanged(current, merged);
      const trafficChanged = hasTrafficChanged(current, merged);
      current = merged;
      if (trafficChanged) {
        trafficState = {
          traffic: merged.traffic,
          trafficHistory: merged.trafficHistory,
        };
        for (const listener of trafficListeners) {
          listener(trafficState);
        }
      }
      if (appChanged) {
        appSnap = merged;
        for (const listener of appListeners) {
          listener(appSnap);
        }
      }
    },
    subscribeApp(listener) {
      appListeners.add(listener);
      return () => {
        appListeners.delete(listener);
      };
    },
    subscribeTraffic(listener) {
      trafficListeners.add(listener);
      return () => {
        trafficListeners.delete(listener);
      };
    },
  };
}
