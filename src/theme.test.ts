import { describe, it, beforeEach } from "node:test";
import assert from "node:assert/strict";

// 浏览器全局桩：theme.ts 的函数体内部才访问它们，import 时不受影响。
const store = new Map<string, string>();
const attrs = new Map<string, string>();
globalThis.localStorage = {
  getItem: (k: string) => store.get(k) ?? null,
  setItem: (k: string, v: string) => void store.set(k, v),
} as unknown as Storage;
globalThis.document = {
  documentElement: {
    setAttribute: (k: string, v: string) => void attrs.set(k, v),
    removeAttribute: (k: string) => void attrs.delete(k),
  },
} as unknown as Document;

const { applyMode, readStoredMode, storeMode } = await import("./theme.ts");

describe("theme", () => {
  beforeEach(() => {
    store.clear();
    attrs.clear();
  });

  it("没存过或存了非法值时按跟随系统处理", () => {
    assert.equal(readStoredMode(), "system");
    store.set("tailash-theme", "purple");
    assert.equal(readStoredMode(), "system");
  });

  it("存过的显式模式能读回", () => {
    storeMode("dark");
    assert.equal(readStoredMode(), "dark");
  });

  it("显式模式写 data-theme，跟随系统则清掉它", () => {
    applyMode("dark");
    assert.equal(attrs.get("data-theme"), "dark");
    applyMode("light");
    assert.equal(attrs.get("data-theme"), "light");
    applyMode("system");
    assert.equal(attrs.has("data-theme"), false);
  });
});
