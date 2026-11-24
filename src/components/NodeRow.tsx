//! 节点行：名称 + 类型/UDP 徽标 + 延迟徽标 + 单点测速；可点击（Selector 组内）切换。
//! 用 React.memo + 稳定回调签名（传 node.name / groupName）避免未变动行随批量测速重渲染。
import { memo } from "react";
import type { NodeInfo } from "../api";
import { delayBadgeClass, delayText } from "../delay";

interface Props {
  node: NodeInfo;
  testing: boolean;
  failed: boolean;
  /** 批量测速进行中：测速按钮一并灰掉（不转菊花，该节点未必正在测） */
  busy?: boolean;
  /** 在所属组里被选中 */
  selected?: boolean;
  /** 所属组是 Selector 才可点击切换 */
  clickable: boolean;
  /** 所属组名（clickable 为 true 时传给 onSelect） */
  groupName?: string;
  onSelect?: (group: string, node: string) => void;
  onTest?: (name: string) => void;
}

function NodeRow({
  node,
  testing,
  failed,
  busy = false,
  selected = false,
  clickable,
  groupName,
  onSelect,
  onTest,
}: Props) {
  return (
    <div
      className={`node-row-virtual flex items-center gap-2 rounded-md px-2 py-1 ${
        selected ? "bg-primary/10" : ""
      } ${clickable ? "cursor-pointer hover:bg-base-200" : ""}`}
      onClick={
        clickable && groupName && onSelect
          ? () => onSelect(groupName, node.name)
          : undefined
      }
      title={clickable ? "点击切换到此节点" : undefined}
    >
      <span
        className={`w-4 shrink-0 text-sm text-primary ${selected ? "" : "invisible"}`}
      >
        ✓
      </span>
      <span
        className={`flex-1 truncate text-sm ${failed ? "text-error" : ""}`}
      >
        {node.name}
      </span>
      <span className="badge badge-ghost badge-sm">{node.kind}</span>
      {node.udp && <span className="badge badge-ghost badge-sm">UDP</span>}
      <span
        className={`badge badge-sm w-16 justify-center ${delayBadgeClass(node.delay)}`}
      >
        {delayText(node.delay)}
      </span>
      <button
        className="btn btn-ghost btn-xs"
        disabled={testing || busy}
        onClick={(e) => {
          e.stopPropagation();
          onTest?.(node.name);
        }}
        title="测速此节点"
      >
        {testing ? <span className="loading loading-spinner loading-xs" /> : "测速"}
      </button>
    </div>
  );
}

export default memo(NodeRow);
