//! 后端快照与命令的 TS 映射：字段与 Rust 侧 serde camelCase 一一对应。
//! 浏览器 dev（无 Tauri 环境）时命令走 mock、快照经 window 事件推送。
import { startTransition, useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { mockDefaultSnapshot, mockSendCmd, MOCK_SNAPSHOT_EVENT } from "./mock";
import {
  createLatestCoalescer,
  createSnapshotStore,
  type TrafficState,
} from "./snapshotState";

export type ProxyMode = "rule" | "direct" | "global";

export interface NodeInfo {
  name: string;
  kind: string;
  delay: number | null;
  udp: boolean;
}

export interface GroupInfo {
  name: string;
  kind: string;
  /** 该组当前选中的成员 */
  now: string | null;
  /** mihomo 的 all，顺序即选择顺序 */
  members: string[];
}

export interface ProxyView {
  nodes: NodeInfo[];
  groups: GroupInfo[];
}

export interface SubStatus {
  id: string;
  name: string;
  url: string;
  active: boolean;
  nodeCount: number | null;
  /** unix 秒 */
  updatedAt: number | null;
}

export interface TrafficSample {
  /** 当前上传速率（字节/秒） */
  up: number;
  /** 当前下载速率（字节/秒） */
  down: number;
  /** 累计上传（字节） */
  upTotal: number;
  /** 累计下载（字节） */
  downTotal: number;
}

export interface TrafficPoint {
  up: number;
  down: number;
}

export interface Snapshot {
  coreRunning: boolean;
  capMissing: boolean;
  systemdAvailable: boolean;
  mode: ProxyMode;
  tunEnabled: boolean;
  traffic: TrafficSample;
  trafficHistory: TrafficPoint[];
  view: ProxyView;
  subs: SubStatus[];
  /** 自动更新间隔（小时）；null = 关闭 */
  autoUpdateHours: number | null;
  /** 单点测速进行中的节点名 */
  testing: string[];
  /** 「测全部」进行中 */
  testAllRunning: boolean;
  message: string;
  failedDelays: string[];
}

export type Cmd =
  | { type: "toggleCore" }
  | { type: "grantCap" }
  | { type: "toggleTun"; on: boolean }
  | { type: "setMode"; mode: ProxyMode }
  | { type: "select"; group: string; node: string }
  | { type: "testAll" }
  | { type: "testNode"; name: string }
  | { type: "updateSub"; id: string }
  | { type: "addSub"; name: string; url: string }
  | { type: "removeSub"; id: string }
  | { type: "switchSub"; id: string }
  | { type: "setAutoUpdate"; hours: number | null };

const isTauri = typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;

const emptySnapshot: Snapshot = {
  coreRunning: false,
  capMissing: false,
  systemdAvailable: false,
  mode: "rule",
  tunEnabled: false,
  traffic: { up: 0, down: 0, upTotal: 0, downTotal: 0 },
  trafficHistory: [],
  view: { nodes: [], groups: [] },
  subs: [],
  autoUpdateHours: null,
  testing: [],
  testAllRunning: false,
  message: "正在连接…",
  failedDelays: [],
};

export async function sendCmd(cmd: Cmd): Promise<void> {
  if (!isTauri) return mockSendCmd(cmd);
  return invoke("send_cmd", { cmd });
}

const store = createSnapshotStore(isTauri ? emptySnapshot : mockDefaultSnapshot());
let activeSubscribers = 0;
let stopSource: (() => void) | null = null;

function startSnapshotSource(): () => void {
  const coalescer = createLatestCoalescer<Snapshot>(
    (next) => store.apply(next),
    (cb) => window.requestAnimationFrame(cb),
    (id) => window.cancelAnimationFrame(id),
  );

  if (!isTauri) {
    // mock 模式：mock 后端派发同名语义的 window 事件
    const onMock = (event: Event) =>
      coalescer.push((event as CustomEvent<Snapshot>).detail);
    window.addEventListener(MOCK_SNAPSHOT_EVENT, onMock);
    return () => {
      coalescer.cancel();
      window.removeEventListener(MOCK_SNAPSHOT_EVENT, onMock);
    };
  }

  let unsubscribed = false;
  let unsubscribe: (() => void) | undefined;
  invoke<Snapshot>("get_snapshot")
    // 卸载后到达的快照不再写入
    .then((s) => {
      if (!unsubscribed) store.apply(s);
    })
    .catch(() => {});
  // 卸载早于 promise resolve 时也能正确 unlisten
  listen<Snapshot>("snapshot", (event) => coalescer.push(event.payload)).then(
    (unlisten) => {
      if (unsubscribed) unlisten();
      else unsubscribe = unlisten;
    },
  );
  return () => {
    unsubscribed = true;
    coalescer.cancel();
    unsubscribe?.();
  };
}

function retainSnapshotSource(): () => void {
  activeSubscribers += 1;
  if (activeSubscribers === 1) {
    stopSource = startSnapshotSource();
  }
  return () => {
    activeSubscribers -= 1;
    if (activeSubscribers === 0 && stopSource) {
      stopSource();
      stopSource = null;
    }
  };
}

/** 订阅非流量业务快照（含节点/组/测速状态）；仅业务字段变化时更新，且以 transition 降级避免阻塞交互。 */
export function useSnapshot(): Snapshot {
  const [snap, setSnap] = useState<Snapshot>(() => store.getApp());
  useEffect(() => {
    const release = retainSnapshotSource();
    const unsub = store.subscribeApp((next) => {
      startTransition(() => setSnap(next));
    });
    const current = store.getApp();
    setSnap((prev) => (prev === current ? prev : current));
    return () => {
      unsub();
      release();
    };
  }, []);
  return snap;
}

/** 独立订阅实时网速与历史曲线，避免每秒网速刷新牵连节点页整棵树重渲染。 */
export function useTraffic(): TrafficState {
  const [traffic, setTraffic] = useState<TrafficState>(() => store.getTraffic());
  useEffect(() => {
    const release = retainSnapshotSource();
    const unsub = store.subscribeTraffic((next) => setTraffic(next));
    const current = store.getTraffic();
    setTraffic((prev) => (prev === current ? prev : current));
    return () => {
      unsub();
      release();
    };
  }, []);
  return traffic;
}
