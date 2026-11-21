//! headless UI 集成测试：验证展开所有组后，滚轮能一直滚动到列表底部。
//! 回归背景：此前内层 group-panel 限高滚动 + 外层 proxy-groups 滚动，
//! gpui 没有 scroll chaining，滚轮在内层滚到底后就够不到下面的组和未归组节点。
mod common;

use gpui_kit::base::test_support::texts;
use gpui_kit::test::TestWindowExt as _;
use gpui_kit::{
    AnyWindowHandle, AppContext as _, InputEvent as _, ScrollDelta, ScrollEvent, TestAppContext,
    Window, point, px, size,
};
use std::rc::Rc;
use std::sync::{Arc, RwLock};
use tailash::bridge::{SharedSnapshot, Snapshot};
use tailash::proxy_view::{GroupInfo, NodeInfo, ProxyView};

fn group(name: &str, members: &[&str]) -> GroupInfo {
    GroupInfo {
        name: name.to_string(),
        kind: "Selector".into(),
        now: None,
        members: members.iter().map(|m| m.to_string()).collect(),
    }
}

fn make_snapshot() -> SharedSnapshot {
    // 两组各 30 个节点：整页内容远超视口高度；末尾再放一个未归组节点，
    // 滚到头时它必须可见（此前 bug 中它永远够不到）。
    let mk = |tag: &str| -> Vec<NodeInfo> {
        (0..30)
            .map(|ix| tailash::proxy_view::NodeInfo {
                name: format!("{tag}-{ix:02}"),
                kind: "Vmess".into(),
                delay: Some(100 + ix),
                udp: false,
            })
            .collect()
    };
    let a = mk("A");
    let b = mk("B");
    let orphan = NodeInfo {
        name: "孤儿-1".into(),
        kind: "Vmess".into(),
        delay: None,
        udp: false,
    };
    Arc::new(RwLock::new(Snapshot {
        message: "就绪".into(),
        view: ProxyView {
            nodes: [a, b, vec![orphan]].concat(),
            groups: vec![
                group("组A", &["A-00", "A-29"]),
                group("组B", &["B-00", "B-29"]),
            ],
        },
        ..Default::default()
    }))
}

fn open(cx: &mut TestAppContext) -> AnyWindowHandle {
    cx.update(gpui_kit::init);
    let (handle, _ui) = common::open_window(cx, Some(size(px(560.), px(640.))), |_, cx| {
        cx.new(|_| {
            tailash::ui::Ui::new(
                make_snapshot(),
                Rc::new(std::sync::mpsc::channel().0),
                String::new(),
            )
        })
    });
    handle.into()
}

/// 在 position 处模拟一次向下滚轮并把指针先移过去。
fn wheel_down(
    window: &mut Window,
    position: gpui_kit::Point<gpui_kit::Pixels>,
    cx: &mut gpui_kit::App,
) {
    window.dispatch_event(
        gpui_kit::MouseMoveEvent {
            position,
            pressed_button: None,
            modifiers: window.modifiers(),
        }
        .to_platform_input(),
        cx,
    );
    window.dispatch_event(
        ScrollEvent {
            position,
            delta: ScrollDelta::Pixels(point(px(0.), px(-120.))),
            ..Default::default()
        }
        .to_platform_input(),
        cx,
    );
    window.render_frame(cx);
}

fn y_of(window: &Window, text: &str) -> Option<f32> {
    texts(window)
        .iter()
        .find(|t| t.text() == text)
        .map(|t| t.bounds().origin.y.into())
}

fn visible(window: &Window, text: &str) -> bool {
    texts(window)
        .iter()
        .any(|t| t.text() == text && t.visible())
}

const KEY_TEXTS: [&str; 5] = ["A-00", "A-29", "B-00", "B-29", "孤儿-1"];

#[gpui_kit::test]
fn wheel_reaches_bottom_after_expanding_groups(cx: &mut TestAppContext) {
    let handle = open(cx);
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        // 指针放在列表中部（第一组节点区域内），一路向下滚
        let pos = point(px(280.), px(320.));
        let mut moved_last_round: Vec<Option<f32>> = Vec::new();
        for _ in 0..40 {
            wheel_down(window, pos, cx);
            let ys: Vec<Option<f32>> = KEY_TEXTS.iter().map(|k| y_of(window, k)).collect();
            if moved_last_round == ys {
                break; // 连续两次位置不变 = 已到底
            }
            moved_last_round = ys;
        }
        // 到底后：最后一个未归组节点可见（此前 bug 中它永远够不到），且组A 已滚出
        assert!(
            visible(window, "孤儿-1"),
            "滚到底后未归组节点仍不可见，无法翻到头"
        );
        let orphan_y = y_of(window, "孤儿-1").unwrap();
        assert!(orphan_y <= 608., "孤儿-1 在视口外: y={orphan_y}");
        let a00_y = y_of(window, "A-00").unwrap();
        assert!(
            a00_y < 271.,
            "组A 首节点未滚出视口，说明根本没滚动: y={a00_y}"
        );
    })
    .unwrap();
}
