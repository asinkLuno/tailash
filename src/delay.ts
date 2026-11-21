//! 延迟分档与展示，与后端 proxy_view::classify_delay 的口径保持一致：
//! 测出的 < 超时 < 错误 < 未测。

export const TIMEOUT_MS = 5000;
/** 明显不可能是真实延迟（mihomo 用 1e5/更大值表示错误） */
const IMPLAUSIBLE_DELAY = 100_000;

export type DelayClass = "measured" | "timeout" | "error" | "untested";

export function classifyDelay(delay: number | null | undefined): DelayClass {
  if (delay == null) return "untested";
  if (delay === 0) return "timeout";
  if (delay >= IMPLAUSIBLE_DELAY) return "error";
  if (delay >= TIMEOUT_MS) return "timeout";
  return "measured";
}

/** 排序键：先按分档，再按数值；返回 [分档序, 数值]。 */
export function delaySortKey(delay: number | null | undefined): [number, number] {
  const rank: Record<DelayClass, number> = {
    measured: 0,
    timeout: 1,
    error: 2,
    untested: 3,
  };
  const cls = classifyDelay(delay);
  return [rank[cls], cls === "measured" ? (delay as number) : 0];
}

export function delayText(delay: number | null | undefined): string {
  switch (classifyDelay(delay)) {
    case "measured":
      return `${delay}ms`;
    case "timeout":
      return "超时";
    case "error":
      return "失败";
    case "untested":
      return "未测";
  }
}

/** daisyUI 徽标配色：数值越低越绿，超时/失败红描边，未测灰。 */
export function delayBadgeClass(delay: number | null | undefined): string {
  switch (classifyDelay(delay)) {
    case "measured": {
      const d = delay as number;
      if (d < 200) return "badge-success";
      if (d < 500) return "badge-warning";
      return "badge-error";
    }
    case "timeout":
    case "error":
      return "badge-error badge-outline";
    case "untested":
      return "badge-ghost";
  }
}
