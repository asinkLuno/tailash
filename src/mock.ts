//! 浏览器 dev 用的假后端（无 Tauri 环境）：镜像 worker 的命令语义，方便纯前端走查 UI。
//! 真实环境（tauri dev/build）不会引入此路径的行为。
import type { Cmd, GroupInfo, NodeInfo, Snapshot, TrafficPoint } from "./api";

export const MOCK_SNAPSHOT_EVENT = "tailash-mock-snapshot";

function node(name: string, kind: string, delay: number | null, udp = true): NodeInfo {
  return { name, kind, delay, udp };
}

function group(name: string, kind: string, now: string | null, members: string[]): GroupInfo {
  return { name, kind, now, members };
}

const HK = ["香港HK-A", "香港HK-HY2", "香港-优化", "香港WAP-优化"];
const JP = ["日本JP-HY2", "日本JP-A", "日本-优化"];
const SG = ["新加坡SG-HY2", "新加坡SG-A", "新加坡-优化-GPT"];
const US = ["美国USLA-A", "美国LA-优化-GPT"];

const nodes: NodeInfo[] = [
  node("香港HK-A", "Anytls", 128),
  node("香港HK-HY2", "Hysteria2", 156),
  node("香港-优化", "Vmess", 212),
  node("香港WAP-优化", "Vmess", 486),
  node("日本JP-HY2", "Hysteria2", 96),
  node("日本JP-A", "Anytls", 142),
  node("日本-优化", "Vmess", null),
  node("新加坡SG-HY2", "Hysteria2", 338),
  node("新加坡SG-A", "Anytls", 0),
  node("新加坡-优化-GPT", "Vmess", 402, false),
  node("美国USLA-A", "Anytls", 100000),
  node("美国LA-优化-GPT", "Vmess", 902),
];

const groups: GroupInfo[] = [
  group("节点选择", "Selector", "自动选择", ["自动选择", ...JP, ...SG, ...HK, ...US]),
  // URLTest 组的 now 故意给一个过期的（非最小延迟），验证前端按实测 ping 推导选中项
  group("自动选择", "URLTest", "香港HK-A", [...JP, ...SG, ...HK]),
  group("ChatGPT", "Selector", "新加坡-优化-GPT", ["节点选择", "新加坡-优化-GPT", "美国LA-优化-GPT"]),
  group("流媒体", "Selector", "香港HK-A", ["节点选择", "自动选择", ...HK, ...JP]),
  group("磁力下载", "Selector", "DIRECT", ["DIRECT", "节点选择"]),
];

const INITIAL_TRAFFIC_HISTORY: TrafficPoint[] = Array.from({ length: 30 }, (_, i) => ({
  down: Math.round((350 + 280 * Math.sin(i / 4) + ((i * 73) % 190)) * 1024),
  up: Math.round((70 + 45 * Math.cos(i / 3) + ((i * 31) % 60)) * 1024),
}));

export function mockDefaultSnapshot(): Snapshot {
  const last = INITIAL_TRAFFIC_HISTORY[INITIAL_TRAFFIC_HISTORY.length - 1];
  return {
    coreRunning: true,
    capMissing: true,
    systemdAvailable: true,
    mode: "rule",
    tunEnabled: true,
    traffic: {
      up: last.up,
      down: last.down,
      upTotal: 218 * 1024 * 1024,
      downTotal: 1420 * 1024 * 1024,
    },
    trafficHistory: [...INITIAL_TRAFFIC_HISTORY],
    view: { nodes, groups },
    subs: [
      {
        id: "mock-1",
        name: "机场 A",
        url: "https://provider-a.example/sub?token=abcdef",
        active: true,
        nodeCount: 12,
        updatedAt: Math.floor(Date.now() / 1000) - 3600,
      },
      {
        id: "mock-2",
        name: "机场 B",
        url: "https://provider-b.example/sub",
        active: false,
        nodeCount: 8,
        updatedAt: Math.floor(Date.now() / 1000) - 86400,
      },
    ],
    testing: [],
    autoUpdateHours: 24,
    testAllRunning: false,
    message: "已连接内核（mock 模式，命令只作用于演示数据）",
    failedDelays: ["新加坡SG-A", "美国USLA-A"],
  };
}

let state: Snapshot = mockDefaultSnapshot();

function emit() {
  window.dispatchEvent(
    new CustomEvent<Snapshot>(MOCK_SNAPSHOT_EVENT, {
      detail: {
        ...state,
        view: {
          nodes: state.view.nodes.map((n) => ({ ...n })),
          groups: state.view.groups.map((g) => ({ ...g, members: [...g.members] })),
        },
      },
    }),
  );
}

// 浏览器 mock 模式下每秒推一帧演示网速，便于调试折线图。
if (typeof window !== "undefined" && !("__TAURI_INTERNALS__" in window)) {
  window.setInterval(() => {
    if (!state.coreRunning) return;
    const down = Math.round((80 + Math.random() * 900) * 1024);
    const up = Math.round((20 + Math.random() * 180) * 1024);
    state.traffic = {
      up,
      down,
      upTotal: state.traffic.upTotal + up,
      downTotal: state.traffic.downTotal + down,
    };
    state.trafficHistory = [...state.trafficHistory.slice(-59), { up, down }];
    emit();
  }, 1000);
}

function findGroup(name: string): GroupInfo | undefined {
  return state.view.groups.find((g) => g.name === name);
}

export function mockSendCmd(cmd: Cmd): Promise<void> {
  switch (cmd.type) {
    case "toggleCore":
      state.coreRunning = !state.coreRunning;
      state.tunEnabled = false;
      if (!state.coreRunning) {
        state.traffic = { up: 0, down: 0, upTotal: 0, downTotal: 0 };
        state.trafficHistory = [];
      }
      state.message = state.coreRunning ? "正在启动内核…" : "正在停止内核…";
      break;
    case "grantCap":
      state.capMissing = false;
      state.message = "已授权；若内核正在运行，停止后重新启动即可生效";
      break;
    case "toggleTun":
      state.tunEnabled = cmd.on;
      state.message = cmd.on ? "TUN 已开启" : "TUN 已关闭";
      break;
    case "setMode":
      state.mode = cmd.mode;
      state.message = `已切换到${{ rule: "策略", direct: "直连", global: "全局" }[cmd.mode]}模式`;
      break;
    case "select": {
      const g = findGroup(cmd.group);
      if (g) {
        g.now = cmd.node;
        state.message = "已切换";
      }
      break;
    }
    case "testAll": {
      state.message = "测延迟中…";
      state.testAllRunning = true;
      state.failedDelays = [];
      // 点击测全部即清空旧延迟，未测节点显示「未测」
      for (const n of state.view.nodes) n.delay = null;
      emit();
      for (const n of state.view.nodes) {
        window.setTimeout(() => {
          n.delay =
            n.name === "新加坡SG-A"
              ? 0
              : n.name === "美国USLA-A"
                ? 100000
                : 60 + Math.floor(Math.random() * 700);
          if (n.delay === 0 || n.delay === 100000) {
            state.failedDelays = [...state.failedDelays.filter((f) => f !== n.name), n.name];
          } else {
            state.failedDelays = state.failedDelays.filter((f) => f !== n.name);
          }
          emit();
        }, 300 + Math.random() * 1200);
      }
      window.setTimeout(() => {
        state.testAllRunning = false;
        state.message = "测速完成：成功 10 个，失败 2 个";
        emit();
      }, 1700);
      break;
    }
    case "testNode": {
      state.testing = [...state.testing, cmd.name];
      state.message = "测延迟中…";
      window.setTimeout(() => {
        const n = state.view.nodes.find((x) => x.name === cmd.name);
        if (n) {
          n.delay = 60 + Math.floor(Math.random() * 700);
          state.failedDelays = state.failedDelays.filter((f) => f !== n.name);
          state.message = `${n.name}: ${n.delay}ms`;
        }
        state.testing = state.testing.filter((t) => t !== cmd.name);
        emit();
      }, 600 + Math.random() * 600);
      break;
    }
    case "addSub":
      state.subs = [
        ...state.subs,
        {
          id: `mock-${Date.now()}`,
          name: cmd.name || "新订阅",
          url: cmd.url,
          active: state.subs.length === 0,
          nodeCount: 6 + Math.floor(Math.random() * 10),
          updatedAt: Math.floor(Date.now() / 1000),
        },
      ];
      state.message = "订阅已添加";
      break;
    case "updateSub":
      state.subs = state.subs.map((s) =>
        s.id === cmd.id
          ? { ...s, nodeCount: 10 + Math.floor(Math.random() * 8), updatedAt: Math.floor(Date.now() / 1000) }
          : s,
      );
      state.message = "订阅已更新，12 个节点已重载";
      break;
    case "removeSub":
      state.subs = state.subs.filter((s) => s.id !== cmd.id);
      state.message = "订阅已删除";
      break;
    case "switchSub":
      state.subs = state.subs.map((s) => ({ ...s, active: s.id === cmd.id }));
      state.message = "已切换订阅";
      break;
    case "setAutoUpdate":
      state.autoUpdateHours = cmd.hours;
      state.message = cmd.hours ? `自动更新订阅：每 ${cmd.hours} 小时` : "已关闭自动更新";
      break;
  }
  emit();
  return Promise.resolve();
}
