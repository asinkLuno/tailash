//! 后端快照与命令的 TS 映射：字段与 Rust 侧 serde camelCase 一一对应。
//! 浏览器 dev（无 Tauri 环境）时命令走 mock、快照经 window 事件推送。
import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { mockDefaultSnapshot, mockSendCmd, MOCK_SNAPSHOT_EVENT } from "./mock";

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
  | { type: "switchSub"; id: string };

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
  testing: [],
  testAllRunning: false,
  message: "正在连接…",
  failedDelays: [],
};

export async function sendCmd(cmd: Cmd): Promise<void> {
  if (!isTauri) return mockSendCmd(cmd);
  return invoke("send_cmd", { cmd });
}

export function useSnapshot(): Snapshot {
  const [snap, setSnap] = useState<Snapshot>(() =>
    isTauri ? emptySnapshot : mockDefaultSnapshot(),
  );
  useEffect(() => {
    if (!isTauri) {
      // mock 模式：mock 后端派发同名语义的 window 事件
      const onMock = (event: Event) =>
        setSnap((event as CustomEvent<Snapshot>).detail);
      window.addEventListener(MOCK_SNAPSHOT_EVENT, onMock);
      return () => window.removeEventListener(MOCK_SNAPSHOT_EVENT, onMock);
    }
    let unsubscribed = false;
    let unsubscribe: (() => void) | undefined;
    invoke<Snapshot>("get_snapshot")
      // 卸载后到达的快照不再写 setState
      .then((s) => !unsubscribed && setSnap(s))
      .catch(() => {});
    // 卸载早于 promise resolve 时也能正确 unlisten
    listen<Snapshot>("snapshot", (event) => setSnap(event.payload)).then(
      (unlisten) => {
        if (unsubscribed) unlisten();
        else unsubscribe = unlisten;
      },
    );
    return () => {
      unsubscribed = true;
      unsubscribe?.();
    };
  }, []);
  return snap;
}
