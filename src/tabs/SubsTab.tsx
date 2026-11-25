//! Tab 2：订阅管理 —— 添加 / 更新 / 切换 / 删除。
import { useState } from "react";
import { sendCmd } from "../api";
import type { Snapshot, SubStatus } from "../api";

function fmtTime(unixSec: number | null): string {
  if (!unixSec) return "从未更新";
  const d = new Date(unixSec * 1000);
  return d.toLocaleString("zh-CN", { hour12: false });
}

export default function SubsTab({ snap }: { snap: Snapshot }) {
  const [name, setName] = useState("");
  const [url, setUrl] = useState("");
  const [removing, setRemoving] = useState<SubStatus | null>(null);

  function submit(e: React.FormEvent) {
    e.preventDefault();
    const trimmed = url.trim();
    if (!trimmed) return;
    sendCmd({ type: "addSub", name: name.trim(), url: trimmed });
    setName("");
    setUrl("");
  }

  return (
    <div className="flex flex-col gap-4">
      <form onSubmit={submit} className="card bg-base-100 shadow-sm">
        <div className="card-body gap-3 px-3 py-3">
          <h2 className="card-title text-base">添加订阅</h2>
          <div className="flex flex-col gap-2 sm:flex-row">
            <input
              className="input input-sm w-full sm:w-44"
              placeholder="名称（留空自动获取）"
              value={name}
              onChange={(e) => setName(e.target.value)}
            />
            <input
              className="input input-sm w-full"
              placeholder="https://… 订阅地址"
              value={url}
              onChange={(e) => setUrl(e.target.value)}
            />
            <button className="btn btn-primary btn-sm" type="submit">
              添加
            </button>
          </div>
          <p className="text-xs text-base-content/50">
            添加时会先下载校验；第一条订阅自动启用。切换订阅立即生效（内核运行中则热重载）。
          </p>
        </div>
      </form>

      {snap.subs.length === 0 && (
        <div className="alert">还没有订阅，先在上方添加一个。</div>
      )}

      {snap.subs.map((sub) => (
        <div
          key={sub.id}
          className={`card border bg-base-100 shadow-sm ${
            sub.active ? "border-primary" : "border-base-300"
          }`}
        >
          <div className="card-body gap-2 px-3 py-3">
            <div className="flex items-center gap-2">
              <span className="font-medium">{sub.name}</span>
              {sub.active && (
                <span className="badge badge-primary badge-sm">使用中</span>
              )}
              <div className="ml-auto flex gap-1">
                <button
                  className="btn btn-ghost btn-sm"
                  disabled={sub.active}
                  onClick={() => sendCmd({ type: "switchSub", id: sub.id })}
                >
                  启用
                </button>
                <button
                  className="btn btn-ghost btn-sm"
                  onClick={() => sendCmd({ type: "updateSub", id: sub.id })}
                >
                  更新
                </button>
                <button
                  className="btn btn-ghost btn-sm text-error"
                  onClick={() => setRemoving(sub)}
                >
                  删除
                </button>
              </div>
            </div>
            <p className="truncate text-xs text-base-content/50" title={sub.url}>
              {sub.url}
            </p>
            <div className="flex items-center gap-2">
              <span className="badge badge-ghost badge-sm">
                {sub.nodeCount != null ? `${sub.nodeCount} 个节点` : "未知节点数"}
              </span>
              <span className="text-xs text-base-content/50">
                更新于 {fmtTime(sub.updatedAt)}
              </span>
            </div>
          </div>
        </div>
      ))}

      {removing && (
        <div className="modal modal-open">
          <div className="modal-box">
            <h3 className="text-lg font-bold">删除订阅</h3>
            <p className="py-2 text-sm">
              确定删除「{removing.name}」吗？已下载的订阅内容会一并删除。
            </p>
            <div className="modal-action">
              <button className="btn btn-sm" onClick={() => setRemoving(null)}>
                取消
              </button>
              <button
                className="btn btn-error btn-sm"
                onClick={() => {
                  sendCmd({ type: "removeSub", id: removing.id });
                  setRemoving(null);
                }}
              >
                删除
              </button>
            </div>
          </div>
        </div>
      )}
    </div>
  );
}
