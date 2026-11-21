//! Tab 1：mihomo 内核启停、TUN 模式、cap_net_admin 授权、运行模式。
import { sendCmd } from "../api";
import type { ProxyMode, Snapshot } from "../api";

const MODES: { value: ProxyMode; label: string }[] = [
  { value: "rule", label: "策略" },
  { value: "direct", label: "直连" },
  { value: "global", label: "全局" },
];

export default function CoreTab({ snap }: { snap: Snapshot }) {
  return (
    <div className="flex flex-col gap-4">
      <div className="card bg-base-100 shadow-sm">
        <div className="card-body gap-4 px-3 py-3">
          <div className="flex items-center justify-between">
            <div>
              <h2 className="card-title text-base">mihomo 内核</h2>
              <p className="text-sm text-base-content/60">
                经 systemd 用户服务后台运行，退出应用不影响
              </p>
            </div>
            <button
              className={`btn ${snap.coreRunning ? "btn-error" : "btn-primary"}`}
              disabled={!snap.systemdAvailable}
              onClick={() => sendCmd({ type: "toggleCore" })}
            >
              {snap.coreRunning ? "停止内核" : "启动内核"}
            </button>
          </div>
          {!snap.systemdAvailable && (
            <p className="text-sm text-warning">
              未检测到 systemd 用户实例，应用内启停不可用；可在终端手动运行内核。
            </p>
          )}

          <div className="divider my-0" />

          <div className="flex items-center justify-between">
            <div>
              <h3 className="font-medium">TUN 模式</h3>
              <p className="text-sm text-base-content/60">
                虚拟网卡接管全局路由（Tailscale 网段已排除）
              </p>
            </div>
            <input
              type="checkbox"
              className="toggle toggle-primary"
              checked={snap.tunEnabled}
              disabled={!snap.coreRunning}
              onChange={(e) => sendCmd({ type: "toggleTun", on: e.target.checked })}
            />
          </div>

          {snap.capMissing && (
            <div className="alert alert-warning py-2">
              <span className="flex-1 text-sm">
                内核缺少 cap_net_admin，TUN 无法工作。
              </span>
              <button
                className="btn btn-warning btn-sm"
                onClick={() => sendCmd({ type: "grantCap" })}
              >
                授权 TUN
              </button>
            </div>
          )}

          <div className="divider my-0" />

          <div className="flex items-center justify-between">
            <div>
              <h3 className="font-medium">运行模式</h3>
              <p className="text-sm text-base-content/60">
                策略按规则分流；直连不走代理；全局全走代理
              </p>
            </div>
            <div className="join">
              {MODES.map((m) => (
                <button
                  key={m.value}
                  className={`btn join-item btn-sm ${snap.mode === m.value ? "btn-primary" : ""}`}
                  disabled={!snap.coreRunning}
                  onClick={() => sendCmd({ type: "setMode", mode: m.value })}
                >
                  {m.label}
                </button>
              ))}
            </div>
          </div>
        </div>
      </div>
    </div>
  );
}
