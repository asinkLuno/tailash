//! 网速与流量字节格式化工具（供顶栏徽标与实时网速图表复用）。

const UNITS = ["B", "KB", "MB", "GB", "TB"] as const;

export function formatBytes(bytes: number): string {
  if (!Number.isFinite(bytes) || bytes <= 0) return "0 B";
  let value = bytes;
  let idx = 0;
  while (value >= 1024 && idx < UNITS.length - 1) {
    value /= 1024;
    idx += 1;
  }
  if (idx === 0) return `${Math.round(value)} B`;
  const digits = value >= 100 ? 0 : value >= 10 ? 1 : 2;
  return `${value.toFixed(digits)} ${UNITS[idx]}`;
}

export function formatSpeed(bytesPerSec: number): string {
  return `${formatBytes(bytesPerSec)}/s`;
}

/** 计算图表纵轴上限：向上取整到易读刻度，且设定最小量程避免空闲时放大噪声。 */
export function niceChartMax(peakBytesPerSec: number): number {
  const MIN_SCALE = 16 * 1024; // 16 KB/s
  if (!Number.isFinite(peakBytesPerSec) || peakBytesPerSec <= MIN_SCALE) {
    return MIN_SCALE;
  }
  // 在 1024 进制内按 1 / 2 / 4 / 8 / 16 / 32 / 64 / 128 / 256 / 512 / 1024 取整
  let base = 1024;
  while (peakBytesPerSec > base * 1024) {
    base *= 1024;
  }
  const ratio = peakBytesPerSec / base;
  const steps = [1, 2, 5, 10, 20, 50, 100, 200, 500, 1024];
  const step = steps.find((s) => ratio <= s) ?? 1024;
  return step * base;
}
