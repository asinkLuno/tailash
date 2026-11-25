import { describe, it } from "node:test";
import assert from "node:assert/strict";
import type { GroupInfo, NodeInfo, ProxyView } from "./api.ts";
import {
  buildNodesIndex,
  filterByGemini,
  effectiveNow,
  parseSortMode,
  resolveChain,
  sortNodes,
} from "./nodesModel.ts";

describe("sortNodes", () => {
  const nodes: NodeInfo[] = [
    { name: "香港-B", kind: "Vmess", delay: 200, udp: true },
    { name: "日本-A", kind: "Vmess", delay: null, udp: true },
    { name: "新加坡-C", kind: "Vmess", delay: 0, udp: true },
    { name: "美国-D", kind: "Vmess", delay: 90, udp: true },
    { name: "台湾-E", kind: "Vmess", delay: 100_000, udp: true },
  ];

  it("keeps original reference in subscription mode", () => {
    assert.equal(sortNodes(nodes, "subscription"), nodes);
  });

  it("sorts measured < timeout < error < untested in delay mode", () => {
    const names = sortNodes(nodes, "delay").map((n) => n.name);
    assert.deepEqual(names, ["美国-D", "香港-B", "新加坡-C", "台湾-E", "日本-A"]);
  });

  it("sorts by Chinese locale in name mode", () => {
    const sorted = sortNodes(nodes, "name").map((n) => n.name);
    const expected = [...nodes]
      .sort((a, b) => a.name.localeCompare(b.name, "zh"))
      .map((n) => n.name);
    assert.deepEqual(sorted, expected);
  });
});

describe("parseSortMode", () => {
  it("accepts known modes and falls back to subscription", () => {
    assert.equal(parseSortMode("delay"), "delay");
    assert.equal(parseSortMode("name"), "name");
    assert.equal(parseSortMode("subscription"), "subscription");
    assert.equal(parseSortMode(null), "subscription");
    assert.equal(parseSortMode("bogus"), "subscription");
  });
});

describe("buildNodesIndex / effectiveNow / resolveChain", () => {
  it("picks minimum measured delay for url-test groups and null if none measured", () => {
    const view: ProxyView = {
      nodes: [
        { name: "Slow", kind: "Vmess", delay: 400, udp: true },
        { name: "Fast", kind: "Vmess", delay: 80, udp: true },
        { name: "Timeout", kind: "Vmess", delay: 0, udp: true },
        { name: "Untested", kind: "Vmess", delay: null, udp: true },
      ],
      groups: [
        { name: "自动选择", kind: "URLTest", now: "Slow", members: ["Slow", "Fast", "Timeout"] },
        { name: "未测自动", kind: "url-test", now: "Untested", members: ["Untested", "Timeout"] },
        { name: "节点选择", kind: "Selector", now: "自动选择", members: ["自动选择", "Slow", "Fast"] },
      ],
    };

    const index = buildNodesIndex(view);
    assert.equal(effectiveNow(index, view.groups[0]), "Fast");
    assert.equal(effectiveNow(index, view.groups[1]), null);
    assert.equal(effectiveNow(index, view.groups[2]), "自动选择");

    assert.deepEqual(resolveChain(index, "节点选择"), ["自动选择", "Fast"]);
    assert.deepEqual(index.resolved.get("节点选择"), {
      chain: ["自动选择", "Fast"],
      node: view.nodes[1],
    });
    assert.deepEqual(index.resolved.get("未测自动"), {
      chain: [],
      node: null,
    });
  });

  it("breaks cycles safely in group chains", () => {
    const view: ProxyView = {
      nodes: [],
      groups: [
        { name: "G1", kind: "Selector", now: "G2", members: ["G2"] },
        { name: "G2", kind: "Selector", now: "G1", members: ["G1"] },
      ],
    };
    const index = buildNodesIndex(view);
    assert.deepEqual(resolveChain(index, "G1"), ["G2", "G1"]);
  });

  it("builds index and sorts 500 nodes across 20 groups quickly", () => {
    const nodes: NodeInfo[] = Array.from({ length: 500 }, (_, i) => ({
      name: `节点-${String(500 - i).padStart(3, "0")}`,
      kind: "Vmess",
      delay: i % 10 === 0 ? null : 50 + ((i * 37) % 600),
      udp: true,
    }));
    const members = nodes.map((n) => n.name);
    const groups: GroupInfo[] = Array.from({ length: 20 }, (_, g) => ({
      name: `Group-${g}`,
      kind: g === 1 ? "URLTest" : "Selector",
      now: g === 0 ? "Group-1" : members[0],
      members: g === 0 ? ["Group-1", ...members] : members,
    }));

    const t0 = performance.now();
    const index = buildNodesIndex({ nodes, groups });
    const byName = sortNodes(nodes, "name");
    const byDelay = sortNodes(nodes, "delay");
    const elapsed = performance.now() - t0;

    assert.equal(index.resolved.size, 20);
    assert.equal(byName.length, 500);
    assert.equal(byDelay.length, 500);
    assert.ok(elapsed < 30, `buildNodesIndex + sortNodes took ${elapsed.toFixed(2)}ms`);
  });
});

describe("filterByGemini", () => {
  const node = (name: string): NodeInfo => ({ name, kind: "Vmess", delay: 100, udp: true });
  const view: ProxyView = {
    nodes: [node("可用"), node("不可用"), node("未测")],
    groups: [
      { name: "节点选择", kind: "Selector", now: "可用", members: ["可用", "不可用", "DIRECT"] },
      { name: "Gemini", kind: "Selector", now: "节点选择", members: ["节点选择"] },
      { name: "空组", kind: "Selector", now: null, members: ["不可用"] },
    ],
  };
  const filtered = filterByGemini(view, { 可用: true, 不可用: false });

  it("keeps only available nodes, group refs and builtins", () => {
    assert.deepEqual(
      filtered.nodes.map((n) => n.name),
      ["可用"],
    );
    assert.deepEqual(filtered.groups[0].members, ["可用", "DIRECT"]);
    assert.deepEqual(filtered.groups[1].members, ["节点选择"]);
  });

  it("drops groups that become empty", () => {
    assert.deepEqual(
      filtered.groups.map((g) => g.name),
      ["节点选择", "Gemini"],
    );
  });

  it("does not mutate the input view", () => {
    assert.deepEqual(view.nodes.map((n) => n.name), ["可用", "不可用", "未测"]);
    assert.deepEqual(view.groups[0].members, ["可用", "不可用", "DIRECT"]);
  });
});
