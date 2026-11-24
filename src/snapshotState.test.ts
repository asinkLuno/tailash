import { describe, it } from "node:test";
import assert from "node:assert/strict";
import type { GroupInfo, NodeInfo, Snapshot } from "./api.ts";
import {
  createLatestCoalescer,
  createSnapshotStore,
  hasAppStateChanged,
  hasTrafficChanged,
  reconcileSnapshot,
} from "./snapshotState.ts";

function makeSnapshot(overrides: Partial<Snapshot> = {}): Snapshot {
  return {
    coreRunning: true,
    capMissing: false,
    systemdAvailable: true,
    mode: "rule",
    tunEnabled: true,
    traffic: { up: 10, down: 20, upTotal: 100, downTotal: 200 },
    trafficHistory: [{ up: 10, down: 20 }],
    view: {
      nodes: [
        { name: "HK-1", kind: "Vmess", delay: 80, udp: true },
        { name: "JP-1", kind: "Hysteria2", delay: 120, udp: true },
        { name: "US-1", kind: "Anytls", delay: null, udp: false },
      ],
      groups: [
        { name: "节点选择", kind: "Selector", now: "HK-1", members: ["自动选择", "HK-1", "JP-1", "US-1"] },
        { name: "自动选择", kind: "URLTest", now: "HK-1", members: ["HK-1", "JP-1", "US-1"] },
      ],
    },
    subs: [
      { id: "s1", name: "机场A", url: "https://a.example", active: true, nodeCount: 3, updatedAt: 1000 },
    ],
    testing: [],
    testAllRunning: false,
    message: "已连接内核",
    failedDelays: [],
    ...overrides,
  };
}

describe("reconcileSnapshot", () => {
  it("preserves view and non-traffic references when only traffic changes", () => {
    const prev = makeSnapshot();
    const next = makeSnapshot({
      traffic: { up: 55, down: 99, upTotal: 155, downTotal: 299 },
      trafficHistory: [{ up: 10, down: 20 }, { up: 55, down: 99 }],
    });

    const merged = reconcileSnapshot(prev, next);

    assert.equal(hasTrafficChanged(prev, merged), true);
    assert.equal(hasAppStateChanged(prev, merged), false);
    assert.equal(merged.view, prev.view);
    assert.equal(merged.view.nodes, prev.view.nodes);
    assert.equal(merged.view.groups, prev.view.groups);
    assert.equal(merged.subs, prev.subs);
    assert.equal(merged.testing, prev.testing);
    assert.equal(merged.failedDelays, prev.failedDelays);
  });

  it("replaces only changed NodeInfo objects during delay updates", () => {
    const prev = makeSnapshot({ testAllRunning: true, message: "测延迟中…" });
    const next = makeSnapshot({
      testAllRunning: true,
      message: "测延迟中…",
      view: {
        nodes: [
          { name: "HK-1", kind: "Vmess", delay: 80, udp: true },
          { name: "JP-1", kind: "Hysteria2", delay: 95, udp: true },
          { name: "US-1", kind: "Anytls", delay: null, udp: false },
        ],
        groups: [
          { name: "节点选择", kind: "Selector", now: "HK-1", members: ["自动选择", "HK-1", "JP-1", "US-1"] },
          { name: "自动选择", kind: "URLTest", now: "HK-1", members: ["HK-1", "JP-1", "US-1"] },
        ],
      },
    });

    const merged = reconcileSnapshot(prev, next);

    assert.equal(hasAppStateChanged(prev, merged), true);
    assert.equal(hasTrafficChanged(prev, merged), false);
    assert.equal(merged.view.groups, prev.view.groups);
    assert.equal(merged.view.nodes[0], prev.view.nodes[0], "unchanged HK-1 keeps ref");
    assert.notEqual(merged.view.nodes[1], prev.view.nodes[1], "changed JP-1 gets new ref");
    assert.equal(merged.view.nodes[1].delay, 95);
    assert.equal(merged.view.nodes[2], prev.view.nodes[2], "unchanged US-1 keeps ref");
  });

  it("replaces only the group whose selection changed", () => {
    const prev = makeSnapshot();
    const next = makeSnapshot({
      view: {
        nodes: prev.view.nodes.map((n) => ({ ...n })),
        groups: [
          { name: "节点选择", kind: "Selector", now: "JP-1", members: ["自动选择", "HK-1", "JP-1", "US-1"] },
          { name: "自动选择", kind: "URLTest", now: "HK-1", members: ["HK-1", "JP-1", "US-1"] },
        ],
      },
    });

    const merged = reconcileSnapshot(prev, next);

    assert.equal(merged.view.nodes, prev.view.nodes);
    assert.notEqual(merged.view.groups[0], prev.view.groups[0]);
    assert.equal(merged.view.groups[1], prev.view.groups[1]);
  });

  it("handles large node and group updates quickly while keeping unchanged refs", () => {
    const nodes: NodeInfo[] = Array.from({ length: 500 }, (_, i) => ({
      name: `Node-${i}`,
      kind: "Vmess",
      delay: null,
      udp: true,
    }));
    const memberNames = nodes.map((n) => n.name);
    const groups: GroupInfo[] = Array.from({ length: 20 }, (_, g) => ({
      name: `Group-${g}`,
      kind: g === 1 ? "URLTest" : "Selector",
      now: memberNames[0],
      members: [...memberNames],
    }));
    const prev = makeSnapshot({ view: { nodes, groups } });
    const next = makeSnapshot({
      view: {
        nodes: nodes.map((n, i) => ({ ...n, delay: i < 8 ? 100 + i : n.delay })),
        groups: groups.map((g) => ({ ...g, members: [...g.members] })),
      },
    });

    const t0 = performance.now();
    const merged = reconcileSnapshot(prev, next);
    const elapsed = performance.now() - t0;

    assert.equal(merged.view.groups, prev.view.groups);
    let kept = 0;
    for (let i = 0; i < 500; i++) {
      if (merged.view.nodes[i] === prev.view.nodes[i]) kept++;
    }
    assert.equal(kept, 492);
    assert.ok(elapsed < 20, `reconcileSnapshot took ${elapsed.toFixed(2)}ms`);
  });
});

describe("createLatestCoalescer", () => {
  it("coalesces rapid updates into a single flush with the latest value", () => {
    const scheduled: (() => void)[] = [];
    const delivered: number[] = [];
    const coalescer = createLatestCoalescer<number>(
      (value) => delivered.push(value),
      (fn) => {
        scheduled.push(fn);
        return scheduled.length;
      },
      () => {},
    );

    coalescer.push(1);
    coalescer.push(2);
    coalescer.push(3);

    assert.equal(scheduled.length, 1, "only one frame should be scheduled");
    assert.deepEqual(delivered, []);

    scheduled[0]();
    assert.deepEqual(delivered, [3]);

    coalescer.push(4);
    assert.equal(scheduled.length, 2);
    scheduled[1]();
    assert.deepEqual(delivered, [3, 4]);
  });

  it("cancels pending flush cleanly", () => {
    let canceledId: number | null = null;
    const scheduled: (() => void)[] = [];
    const delivered: number[] = [];
    const coalescer = createLatestCoalescer<number>(
      (value) => delivered.push(value),
      (fn) => {
        scheduled.push(fn);
        return 42;
      },
      (id) => {
        canceledId = id;
      },
    );

    coalescer.push(99);
    coalescer.cancel();
    assert.equal(canceledId, 42);
    // Even if callback fires late, it must not deliver after cancel
    scheduled[0]();
    assert.deepEqual(delivered, []);
  });
});

describe("createSnapshotStore", () => {
  it("notifies only traffic listeners on traffic-only updates and only app listeners on delay updates", () => {
    const initial = makeSnapshot();
    const store = createSnapshotStore(initial);
    let appNotifies = 0;
    let trafficNotifies = 0;
    const unsubApp = store.subscribeApp(() => appNotifies++);
    const unsubTraffic = store.subscribeTraffic(() => trafficNotifies++);

    store.apply(
      makeSnapshot({
        traffic: { up: 88, down: 99, upTotal: 188, downTotal: 299 },
        trafficHistory: [{ up: 88, down: 99 }],
      }),
    );
    assert.equal(trafficNotifies, 1);
    assert.equal(appNotifies, 0, "traffic-only update must not trigger NodesTab/App re-render");
    assert.equal(store.getTraffic().traffic.down, 99);

    const prevNodes = store.getApp().view.nodes;
    store.apply(
      makeSnapshot({
        traffic: { up: 88, down: 99, upTotal: 188, downTotal: 299 },
        trafficHistory: [{ up: 88, down: 99 }],
        view: {
          nodes: [
            { name: "HK-1", kind: "Vmess", delay: 80, udp: true },
            { name: "JP-1", kind: "Hysteria2", delay: 65, udp: true },
            { name: "US-1", kind: "Anytls", delay: null, udp: false },
          ],
          groups: initial.view.groups.map((g) => ({ ...g, members: [...g.members] })),
        },
      }),
    );
    assert.equal(trafficNotifies, 1, "delay-only update must not trigger traffic re-render");
    assert.equal(appNotifies, 1);
    assert.equal(store.getApp().view.nodes[0], prevNodes[0]);
    assert.equal(store.getApp().view.nodes[1].delay, 65);

    unsubApp();
    unsubTraffic();
  });

  it("handles reordered, added, removed nodes and identical snapshots cleanly", () => {
    const prev = makeSnapshot();
    assert.equal(reconcileSnapshot(prev, makeSnapshot()), prev, "identical snapshot returns prev ref");

    const reordered = makeSnapshot({
      view: {
        nodes: [
          { name: "US-1", kind: "Anytls", delay: null, udp: false },
          { name: "HK-1", kind: "Vmess", delay: 80, udp: true },
          { name: "SG-New", kind: "Vmess", delay: 50, udp: true },
        ],
        groups: prev.view.groups.map((g) => ({ ...g, members: [...g.members] })),
      },
      testing: ["HK-1"],
      failedDelays: ["US-1"],
    });

    const merged = reconcileSnapshot(prev, reordered);
    assert.equal(merged.view.nodes[0], prev.view.nodes[2], "reordered unchanged US-1 keeps ref");
    assert.equal(merged.view.nodes[1], prev.view.nodes[0], "reordered unchanged HK-1 keeps ref");
    assert.equal(merged.view.nodes[2].name, "SG-New");
    assert.deepEqual(merged.testing, ["HK-1"]);
    assert.deepEqual(merged.failedDelays, ["US-1"]);
  });

  it("supports re-entrant push inside coalescer onFlush", () => {
    const scheduled: (() => void)[] = [];
    const delivered: number[] = [];
    const coalescer = createLatestCoalescer<number>(
      (value) => {
        delivered.push(value);
        if (value === 1) coalescer.push(2);
      },
      (fn) => {
        scheduled.push(fn);
        return scheduled.length;
      },
      () => {},
    );

    coalescer.push(1);
    scheduled[0]();
    assert.deepEqual(delivered, [1]);
    assert.equal(scheduled.length, 2, "re-entrant push schedules next frame");
    scheduled[1]();
    assert.deepEqual(delivered, [1, 2]);
  });
});
