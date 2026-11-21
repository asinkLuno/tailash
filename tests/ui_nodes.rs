//! headless UI 集成测试：真实渲染 Ui，验证按组展示的节点网格、组切换、排序与点选。
//! headless UI 集成测试：验证组优先节点列表、折叠、排序与点选。
mod common;

use gpui_kit::base::test_support::texts;
use gpui_kit::test::TestWindowExt as _;
use gpui_kit::{
    AnyWindowHandle, AppContext as _, ElementId, Entity, TestAppContext, Window, px, size,
};
use std::rc::Rc;
use std::sync::{Arc, RwLock};
use tailash::bridge::{SharedSnapshot, Snapshot};
use tailash::proxy_view::{GroupInfo, NodeInfo, ProxyView};
use tailash::ui::SortKey;

fn node(name: &str, delay: Option<u32>) -> NodeInfo {
    NodeInfo {
        name: name.to_string(),
        kind: "Vmess".into(),
        delay,
        udp: false,
    }
}
fn group(name: &str, members: &[&str], now: Option<&str>) -> GroupInfo {
    GroupInfo {
        name: name.to_string(),
        kind: "Selector".into(),
        now: now.map(str::to_string),
        members: members.iter().map(|m| m.to_string()).collect(),
    }
}
fn make_snapshot(nodes: Vec<NodeInfo>, groups: Vec<GroupInfo>) -> SharedSnapshot {
    Arc::new(RwLock::new(Snapshot {
        message: "就绪".into(),
        view: ProxyView { nodes, groups },
        ..Default::default()
    }))
}
struct TestRig {
    ui: Entity<tailash::ui::Ui>,
    handle: AnyWindowHandle,
    cmds: std::sync::mpsc::Receiver<tailash::bridge::UiCmd>,
}
fn open(cx: &mut TestAppContext, snapshot: SharedSnapshot) -> TestRig {
    cx.update(gpui_kit::init);
    let (tx, rx) = std::sync::mpsc::channel();
    let (handle, ui) = common::open_window(cx, Some(size(px(560.), px(640.))), |_, cx| {
        cx.new(|_| tailash::ui::Ui::new(snapshot.clone(), Rc::new(tx), String::new()))
    });
    TestRig {
        ui,
        handle: handle.into(),
        cmds: rx,
    }
}
fn painted_texts(window: &mut Window, cx: &mut gpui_kit::App) -> Vec<String> {
    window.render_frame(cx);
    texts(window).iter().map(|t| t.text().to_string()).collect()
}
fn pos(needle: &str, painted: &[String]) -> usize {
    painted
        .iter()
        .position(|p| p == needle)
        .unwrap_or_else(|| panic!("{needle:?} 未绘制: {painted:?}"))
}

#[gpui_kit::test]
fn cards_render_delay_and_selection(cx: &mut TestAppContext) {
    let snap = make_snapshot(
        vec![
            node("香港 01", Some(120)),
            node("日本 02", Some(450)),
            node("美国 03", None),
        ],
        vec![group(
            "节点选择",
            &["香港 01", "日本 02", "美国 03"],
            Some("香港 01"),
        )],
    );
    let rig = open(cx, snap);
    cx.update_window(rig.handle, |_, window, cx| {
        let painted = painted_texts(window, cx);
        for text in ["香港 01", "日本 02", "美国 03", "120 ms", "450 ms", "未测"] {
            pos(text, &painted);
        }
        window.click(ElementId::named_usize("node-card", 0), cx);
    })
    .unwrap();
    assert!(
        matches!(rig.cmds.try_recv().expect("点击节点应发 Select"), tailash::bridge::UiCmd::Select { ref group, ref node } if group == "节点选择" && node == "香港 01")
    );
}

#[gpui_kit::test]
fn sort_cycle_button_reorders_rows(cx: &mut TestAppContext) {
    let rig = open(
        cx,
        make_snapshot(
            vec![
                node("慢节点", Some(800)),
                node("快节点", Some(80)),
                node("未测节点", None),
            ],
            vec![],
        ),
    );
    let ui = rig.ui.clone();
    cx.update_window(rig.handle, |_, window, cx| {
        assert_eq!(ui.read(cx).sort, SortKey::Subscription);
        window.click("sort-cycle", cx);
        assert_eq!(ui.read(cx).sort, SortKey::Delay);
        let painted = painted_texts(window, cx);
        assert!(pos("快节点", &painted) < pos("慢节点", &painted));
        assert!(pos("慢节点", &painted) < pos("未测节点", &painted));
        window.click("sort-cycle", cx);
        assert_eq!(ui.read(cx).sort, SortKey::Name);
        window.click("sort-cycle", cx);
        assert_eq!(ui.read(cx).sort, SortKey::Subscription);
    })
    .unwrap();
}

#[gpui_kit::test]
fn failed_delay_is_visible_on_node_row(cx: &mut TestAppContext) {
    let snap = make_snapshot(vec![node("超时节点", None)], vec![]);
    snap.write().unwrap().failed_delays.push("超时节点".into());
    let rig = open(cx, snap);
    cx.update_window(rig.handle, |_, window, cx| {
        assert!(painted_texts(window, cx).iter().any(|text| text == "失败"))
    })
    .unwrap();
}

#[gpui_kit::test]
fn empty_state_shown_when_no_nodes(cx: &mut TestAppContext) {
    let rig = open(cx, make_snapshot(vec![], vec![]));
    cx.update_window(rig.handle, |_, window, cx| {
        let painted = painted_texts(window, cx);
        assert!(
            painted.iter().any(|t| t.contains("暂无节点")),
            "空状态应显示: {painted:?}"
        );
        assert!(
            !painted.iter().any(|t| t.contains("ms")),
            "无节点时无延迟文本"
        );
    })
    .unwrap();
}

#[gpui_kit::test]
fn groups_include_nested_members_and_ungrouped_nodes_are_read_only(cx: &mut TestAppContext) {
    let snap = make_snapshot(
        vec![
            node("香港 01", Some(80)),
            node("日本 02", Some(120)),
            node("孤立节点", None),
        ],
        vec![
            group("父组", &["子组", "香港 01"], Some("子组")),
            group("子组", &["日本 02"], Some("日本 02")),
        ],
    );
    let rig = open(cx, snap);
    cx.update_window(rig.handle, |_, window, cx| {
        let painted = painted_texts(window, cx);
        for text in [
            "父组",
            "子组",
            "香港 01",
            "日本 02",
            "孤立节点",
            "未归组节点 · 1",
        ] {
            pos(text, &painted);
        }
        window.click(ElementId::named_usize("node-card", 0), cx);
    })
    .unwrap();
    assert!(
        matches!(rig.cmds.try_recv().unwrap(), tailash::bridge::UiCmd::Select { ref group, ref node } if group == "父组" && node == "香港 01")
    );
    cx.update_window(rig.handle, |_, window, cx| {
        window.click(ElementId::named_usize("node-card", 10000), cx);
    })
    .unwrap();
    assert!(
        matches!(rig.cmds.try_recv().unwrap(), tailash::bridge::UiCmd::Select { ref group, ref node } if group == "子组" && node == "日本 02")
    );
}

#[gpui_kit::test]
fn group_folding_updates_open_state(cx: &mut TestAppContext) {
    let rig = open(
        cx,
        make_snapshot(
            vec![node("节点 A", None)],
            vec![group("节点选择", &["节点 A"], None)],
        ),
    );
    cx.update_window(rig.handle, |_, window, cx| {
        assert!(
            painted_texts(window, cx)
                .iter()
                .any(|text| text == "节点 A")
        );
        window.click(ElementId::named_usize("group-title", 0), cx);
        assert!(
            !painted_texts(window, cx)
                .iter()
                .any(|text| text == "节点 A")
        );
        window.click(ElementId::named_usize("group-title", 0), cx);
        assert!(
            painted_texts(window, cx)
                .iter()
                .any(|text| text == "节点 A")
        );
    })
    .unwrap();
}

#[gpui_kit::test]
fn core_button_shows_only_when_systemd_user_instance_is_available(cx: &mut TestAppContext) {
    let snap = make_snapshot(vec![], vec![]);
    let rig = open(cx, snap.clone());
    cx.update_window(rig.handle, |_, window, cx| {
        window.render_frame(cx);
        assert!(window.try_find("start-core").is_none());
    })
    .unwrap();

    snap.write().unwrap().systemd_available = true;
    let rig = open(cx, snap);
    cx.update_window(rig.handle, |_, window, cx| {
        window.click("start-core", cx);
    })
    .unwrap();
    assert!(matches!(
        rig.cmds.try_recv(),
        Ok(tailash::bridge::UiCmd::ToggleCore)
    ));
}

#[gpui_kit::test]
fn group_header_counts_every_row_it_shows(cx: &mut TestAppContext) {
    let snap = make_snapshot(
        vec![node("香港 01", None), node("日本 02", None)],
        vec![
            group("父组", &["子组"], None),
            group("子组", &["孙组", "日本 02"], None),
            group("孙组", &["香港 01"], None),
        ],
    );
    let rig = open(cx, snap);
    cx.update_window(rig.handle, |_, window, cx| {
        pos("Selector · 2", &painted_texts(window, cx));
    })
    .unwrap();
}
