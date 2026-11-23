//! 实时网速监控卡片：参考 clash-verge-rev 的上下行速率、累计流量与双线面积折线图。
//! 纯 SVG 绘制（无第三方图表依赖），最近 60 秒窗口，支持悬停查看单点速率与折叠图表。
import { useId, useMemo, useState, type MouseEvent } from "react";
import type { TrafficPoint, TrafficSample } from "../api";
import { formatBytes, formatSpeed, niceChartMax } from "../traffic";

const WINDOW_SIZE = 60;
const SVG_W = 540;
const SVG_H = 128;
const PAD_X = 4;
const PAD_TOP = 10;
const PAD_BOTTOM = 6;

interface Props {
  coreRunning: boolean;
  traffic: TrafficSample;
  history: TrafficPoint[];
}

function buildPaths(values: number[], maxVal: number): { line: string; area: string } {
  const plotW = SVG_W - PAD_X * 2;
  const plotH = SVG_H - PAD_TOP - PAD_BOTTOM;
  const baseY = SVG_H - PAD_BOTTOM;
  const stepX = plotW / (WINDOW_SIZE - 1);

  const pts = values.map((v, i) => {
    const x = PAD_X + i * stepX;
    const clamped = Math.max(0, Math.min(v, maxVal));
    const y = baseY - (clamped / maxVal) * plotH;
    return [x, y] as const;
  });

  const line = pts
    .map(([x, y], i) => `${i === 0 ? "M" : "L"}${x.toFixed(1)},${y.toFixed(1)}`)
    .join(" ");
  const firstX = pts[0][0].toFixed(1);
  const lastX = pts[pts.length - 1][0].toFixed(1);
  const area = `${line} L${lastX},${baseY.toFixed(1)} L${firstX},${baseY.toFixed(1)} Z`;
  return { line, area };
}

export default function TrafficCard({ coreRunning, traffic, history }: Props) {
  const [showGraph, setShowGraph] = useState(true);
  const [hoverIdx, setHoverIdx] = useState<number | null>(null);
  const idPrefix = useId().replace(/[^a-zA-Z0-9_-]/g, "");
  const downGradId = `${idPrefix}-down`;
  const upGradId = `${idPrefix}-up`;

  // 左补 0 对齐到 60 秒固定窗口，保证波形从右向左平滑推进。
  const padded = useMemo<TrafficPoint[]>(() => {
    const slice = history.slice(-WINDOW_SIZE);
    const padLen = WINDOW_SIZE - slice.length;
    if (padLen <= 0) return slice;
    return [
      ...Array.from({ length: padLen }, () => ({ up: 0, down: 0 })),
      ...slice,
    ];
  }, [history]);

  const maxVal = useMemo(() => {
    let peak = 0;
    for (const p of padded) {
      if (p.down > peak) peak = p.down;
      if (p.up > peak) peak = p.up;
    }
    return niceChartMax(peak);
  }, [padded]);

  const downPaths = useMemo(
    () => buildPaths(padded.map((p) => p.down), maxVal),
    [padded, maxVal],
  );
  const upPaths = useMemo(
    () => buildPaths(padded.map((p) => p.up), maxVal),
    [padded, maxVal],
  );

  const plotW = SVG_W - PAD_X * 2;
  const plotH = SVG_H - PAD_TOP - PAD_BOTTOM;
  const baseY = SVG_H - PAD_BOTTOM;
  const stepX = plotW / (WINDOW_SIZE - 1);

  const hovered = hoverIdx !== null ? padded[hoverIdx] : null;
  const hoverX = hoverIdx !== null ? PAD_X + hoverIdx * stepX : 0;
  const hoverDownY =
    hovered !== null ? baseY - (Math.min(hovered.down, maxVal) / maxVal) * plotH : 0;
  const hoverUpY =
    hovered !== null ? baseY - (Math.min(hovered.up, maxVal) / maxVal) * plotH : 0;

  function handleMouseMove(e: MouseEvent<SVGSVGElement>) {
    if (!coreRunning) return;
    const rect = e.currentTarget.getBoundingClientRect();
    if (rect.width <= 0) return;
    const relX = ((e.clientX - rect.left) / rect.width) * SVG_W;
    const idx = Math.round((relX - PAD_X) / stepX);
    setHoverIdx(Math.max(0, Math.min(WINDOW_SIZE - 1, idx)));
  }

  return (
    <div className="card bg-base-100 shadow-sm">
      <div className="card-body gap-3 px-3 py-3">
        <div className="flex items-center justify-between gap-2">
          <div className="flex items-center gap-2">
            <h2 className="card-title text-base">实时网速</h2>
            <span className="text-xs text-base-content/50">最近 60 秒</span>
          </div>
          <button
            className="btn btn-ghost btn-xs text-base-content/60"
            onClick={() => setShowGraph((v) => !v)}
          >
            {showGraph ? "收起图表" : "展开图表"}
          </button>
        </div>

        {/* 上下行实时速率与本次累计流量 */}
        <div className="grid grid-cols-2 gap-2">
          <div className="rounded-box bg-base-200/70 px-3 py-2">
            <div className="flex items-center justify-between text-xs text-base-content/60">
              <span className="flex items-center gap-1.5">
                <span className="inline-block h-2 w-2 rounded-full bg-primary" />
                下载
              </span>
              <span className="tabular-nums">
                总计 {formatBytes(coreRunning ? traffic.downTotal : 0)}
              </span>
            </div>
            <div className="mt-1 font-mono text-lg font-semibold tabular-nums text-primary">
              ↓ {formatSpeed(coreRunning ? traffic.down : 0)}
            </div>
          </div>

          <div className="rounded-box bg-base-200/70 px-3 py-2">
            <div className="flex items-center justify-between text-xs text-base-content/60">
              <span className="flex items-center gap-1.5">
                <span className="inline-block h-2 w-2 rounded-full bg-success" />
                上传
              </span>
              <span className="tabular-nums">
                总计 {formatBytes(coreRunning ? traffic.upTotal : 0)}
              </span>
            </div>
            <div className="mt-1 font-mono text-lg font-semibold tabular-nums text-success">
              ↑ {formatSpeed(coreRunning ? traffic.up : 0)}
            </div>
          </div>
        </div>

        {/* 双线面积折线图 */}
        {showGraph && (
          <div className="relative rounded-box bg-base-200/50 px-2 pt-2 pb-1">
            <div className="pointer-events-none absolute top-1.5 right-2.5 flex items-center gap-3 text-[11px] text-base-content/55 tabular-nums">
              {hovered ? (
                <span>
                  {WINDOW_SIZE - 1 - (hoverIdx ?? 0)}s 前 · ↓ {formatSpeed(hovered.down)} / ↑{" "}
                  {formatSpeed(hovered.up)}
                </span>
              ) : (
                <span>峰值量程 {formatSpeed(maxVal)}</span>
              )}
            </div>
            <svg
              viewBox={`0 0 ${SVG_W} ${SVG_H}`}
              className="h-32 w-full select-none"
              preserveAspectRatio="none"
              onMouseMove={handleMouseMove}
              onMouseLeave={() => setHoverIdx(null)}
            >
              <defs>
                <linearGradient id={downGradId} x1="0" y1="0" x2="0" y2="1">
                  <stop offset="0%" stopColor="var(--color-primary)" stopOpacity="0.28" />
                  <stop offset="100%" stopColor="var(--color-primary)" stopOpacity="0.02" />
                </linearGradient>
                <linearGradient id={upGradId} x1="0" y1="0" x2="0" y2="1">
                  <stop offset="0%" stopColor="var(--color-success)" stopOpacity="0.24" />
                  <stop offset="100%" stopColor="var(--color-success)" stopOpacity="0.02" />
                </linearGradient>
              </defs>

              {/* 参考水平虚线（顶 / 中 / 底） */}
              {[0, 0.5, 1].map((ratio) => {
                const y = PAD_TOP + ratio * plotH;
                return (
                  <line
                    key={ratio}
                    x1={PAD_X}
                    x2={SVG_W - PAD_X}
                    y1={y}
                    y2={y}
                    stroke="currentColor"
                    strokeOpacity={ratio === 1 ? 0.16 : 0.08}
                    strokeDasharray={ratio === 1 ? undefined : "3 3"}
                    strokeWidth="1"
                  />
                );
              })}

              {/* 下载面积与折线 */}
              <path
                d={downPaths.area}
                fill={`url(#${downGradId})`}
                opacity={coreRunning ? 1 : 0.3}
              />
              <path
                d={downPaths.line}
                fill="none"
                stroke="var(--color-primary)"
                strokeWidth="1.75"
                strokeLinejoin="round"
                strokeLinecap="round"
                opacity={coreRunning ? 0.95 : 0.3}
              />

              {/* 上传面积与折线 */}
              <path
                d={upPaths.area}
                fill={`url(#${upGradId})`}
                opacity={coreRunning ? 1 : 0.3}
              />
              <path
                d={upPaths.line}
                fill="none"
                stroke="var(--color-success)"
                strokeWidth="1.5"
                strokeLinejoin="round"
                strokeLinecap="round"
                opacity={coreRunning ? 0.9 : 0.3}
              />

              {/* 悬停准星 */}
              {coreRunning && hovered && (
                <>
                  <line
                    x1={hoverX}
                    x2={hoverX}
                    y1={PAD_TOP}
                    y2={baseY}
                    stroke="currentColor"
                    strokeOpacity="0.25"
                    strokeDasharray="2 2"
                    strokeWidth="1"
                  />
                  <circle cx={hoverX} cy={hoverDownY} r="3" fill="var(--color-primary)" />
                  <circle cx={hoverX} cy={hoverUpY} r="2.8" fill="var(--color-success)" />
                </>
              )}
            </svg>
          </div>
        )}
      </div>
    </div>
  );
}
