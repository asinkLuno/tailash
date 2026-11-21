//! tailash 主界面：顶栏（品牌 + 三 tab 导航 + 状态徽标）+ 内容区 + 底部消息条。
//! 状态唯一来源是 useSnapshot（后端 500ms 增量推送），命令一律经 sendCmd 异步下发。
import { useState } from "react";
import { useSnapshot } from "./api";
import CoreTab from "./tabs/CoreTab";
import SubsTab from "./tabs/SubsTab";
import NodesTab from "./tabs/NodesTab";

type Tab = "core" | "subs" | "nodes";

const TABS: { id: Tab; label: string }[] = [
  { id: "core", label: "内核" },
  { id: "subs", label: "订阅" },
  { id: "nodes", label: "节点" },
];

export default function App() {
  const snap = useSnapshot();
  // 支持 #subs / #nodes 直达对应 tab（可带参数，如 #nodes?sort=delay；默认内核）
  const [tab, setTab] = useState<Tab>(() => {
    const hash = window.location.hash.slice(1).split("?")[0];
    return TABS.some((t) => t.id === hash) ? (hash as Tab) : "core";
  });

  return (
    <div className="flex h-screen flex-col">
      <div className="navbar min-h-12 bg-base-200 px-4">
        <span className="text-lg font-bold">tailash</span>
        {/* 导航并入顶栏：品牌左侧、状态徽标右侧 */}
        <div role="tablist" className="tabs tabs-box tabs-sm ml-3">
          {TABS.map((t) => (
            <a
              key={t.id}
              role="tab"
              aria-selected={tab === t.id}
              className={`tab ${tab === t.id ? "tab-active" : ""}`}
              onClick={() => setTab(t.id)}
            >
              {t.label}
            </a>
          ))}
        </div>
        <div className="ml-auto flex items-center gap-2">
          <span
            className={`badge badge-sm ${
              snap.coreRunning ? "badge-success" : "badge-error badge-outline"
            }`}
          >
            {snap.coreRunning ? "内核运行中" : "内核未运行"}
          </span>
          <span
            className={`badge badge-sm ${
              snap.tunEnabled ? "badge-info" : "badge-ghost"
            }`}
          >
            TUN {snap.tunEnabled ? "开" : "关"}
          </span>
        </div>
      </div>

      <div className="flex-1 overflow-auto px-4 py-3">
        <div className="mx-auto flex max-w-xl flex-col gap-4">
          {tab === "core" && <CoreTab snap={snap} />}
          {tab === "subs" && <SubsTab snap={snap} />}
          {tab === "nodes" && <NodesTab snap={snap} />}
        </div>
      </div>

      <div className="bg-base-200 px-4 py-1.5 text-sm text-base-content/70">
        {snap.message}
      </div>
    </div>
  );
}
