//! gpui 最小控制台：内核运行状态（不负责启停）、TUN 开关、按组展示的节点列表（选择/测延迟）。
use gpui_kit::base::Selectable;
use gpui_kit::base::TestSupportExt as _;
use gpui_kit::component::button::Button;
use gpui_kit::component::input::Input;
use gpui_kit::component::input::InputState;
use gpui_kit::component::label::Label;
use gpui_kit::component::list::ListItem;
use gpui_kit::component::switch::Switch;
use gpui_kit::component::tag::Tag;
use gpui_kit::component::theme::ActiveTheme as _;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use crate::proxy_view::{DEFAULT_TIMEOUT_MS, DelayClass, classify_delay, delay_sort_key};
use crate::type_scale::TypeRole;

pub struct Ui {
    pub snapshot: crate::bridge::SharedSnapshot,
    /// worker 快照的本地副本；`snap.message` 也承载 UI 本地的提示（占位「就绪」、乐观的「…中」），
    /// 下一次 pull_snapshot 发现与共享快照不一致时会被覆盖。
    pub snap: crate::bridge::Snapshot,
    pub sort: SortKey,
    pub sub_input: Option<Entity<InputState>>,
    pub sub_preload: String,
    pub cmds: std::rc::Rc<std::sync::mpsc::Sender<crate::bridge::UiCmd>>,
    pub open_groups: std::collections::HashSet<String>,
    pub open_ungrouped: bool,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum SortKey {
    #[default]
    Subscription,
    Delay,
    Name,
}

impl SortKey {
    fn next(self) -> Self {
        match self {
            Self::Subscription => Self::Delay,
            Self::Delay => Self::Name,
            Self::Name => Self::Subscription,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Subscription => "默认排序",
            Self::Delay => "按延迟",
            Self::Name => "按名称",
        }
    }
}

/// 按排版档位构造 Label：字号、字重、行高同出一源。
fn label(text: impl Into<SharedString>, role: TypeRole) -> Label {
    Label::new(text)
        .text_size(role.size())
        .font_weight(role.weight())
        .line_height(role.line_height())
}

/// 节点延迟的展示文本（分组行与未归组行共用）。
fn delay_text(node: &crate::proxy_view::NodeInfo, failed: bool) -> String {
    match classify_delay(node.delay, DEFAULT_TIMEOUT_MS) {
        DelayClass::Measured => format!("{} ms", node.delay.unwrap_or(0)),
        DelayClass::Timeout => "超时".into(),
        DelayClass::Error => "错误".into(),
        DelayClass::Untested if failed => "失败".into(),
        DelayClass::Untested => "未测".into(),
    }
}

/// 自动类组（URLTest/Fallback/LoadBalance/Smart）的生效延迟：优先沿 `now`
/// 链解析到实际节点，拿不到再退回成员（含嵌套组）的最小实测延迟。
fn auto_group_delay(
    view: &crate::proxy_view::ProxyView,
    group: &crate::proxy_view::GroupInfo,
) -> Option<(u32, DelayTint)> {
    let delay = auto_now_ms(view, group).or_else(|| best_member_ms(view, &group.members, 0))?;
    Some((delay, delay_tint(Some(delay), false)))
}

/// `now` 链向下（最多 4 层）找到实际节点；该节点超时/未测则放弃。
fn auto_now_ms(
    view: &crate::proxy_view::ProxyView,
    group: &crate::proxy_view::GroupInfo,
) -> Option<u32> {
    let mut name = group.now.clone()?;
    for _ in 0..4 {
        if let Some(node) = view.node(&name) {
            return match classify_delay(node.delay, DEFAULT_TIMEOUT_MS) {
                DelayClass::Measured => node.delay,
                _ => None,
            };
        }
        name = view.group(&name)?.now.clone()?;
    }
    None
}

/// 成员（嵌套组向下递归，最多 4 层）里的最小实测延迟；超时/未测不算。
fn best_member_ms(
    view: &crate::proxy_view::ProxyView,
    members: &[String],
    depth: usize,
) -> Option<u32> {
    if depth > 4 {
        return None;
    }
    let mut best: Option<u32> = None;
    for member in members {
        let candidate = if let Some(node) = view.node(member) {
            match classify_delay(node.delay, DEFAULT_TIMEOUT_MS) {
                DelayClass::Measured => node.delay,
                _ => None,
            }
        } else if let Some(sub) = view.group(member) {
            best_member_ms(view, &sub.members, depth + 1)
        } else {
            None
        };
        if let Some(d) = candidate
            && best.is_none_or(|b| d < b)
        {
            best = Some(d);
        }
    }
    best
}

/// URLTest/Fallback/LoadBalance/Smart 由内核自动挑节点，其余类型由用户手选。
fn is_auto_kind(kind: &str) -> bool {
    matches!(
        kind.to_ascii_lowercase().as_str(),
        "urltest" | "fallback" | "loadbalance" | "smart"
    )
}
/// 延迟染色档：测出值按阈值分绿/黄/红，超时/错误/测速失败红，未测保持 muted。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DelayTint {
    Good,
    Mid,
    Bad,
    Neutral,
}

/// 阈值：≤200ms 顺滑，200–1000ms 可用偏慢，≥1000ms 卡顿。超时即 5s 红线。
const MID_DELAY_MS: u32 = 200;
const BAD_DELAY_MS: u32 = 1000;

/// 节点延迟的染色档（分组行与未归组行共用）。
fn delay_tint(delay: Option<u32>, failed: bool) -> DelayTint {
    match classify_delay(delay, DEFAULT_TIMEOUT_MS) {
        DelayClass::Measured => match delay {
            Some(d) if d >= BAD_DELAY_MS => DelayTint::Bad,
            Some(d) if d >= MID_DELAY_MS => DelayTint::Mid,
            _ => DelayTint::Good,
        },
        DelayClass::Timeout | DelayClass::Error => DelayTint::Bad,
        DelayClass::Untested if failed => DelayTint::Bad,
        DelayClass::Untested => DelayTint::Neutral,
    }
}

impl DelayTint {
    /// 语义色直接取 Tailwind 600 阶（Shadcn 文字用色），不用主题的 `*_foreground`：
    /// 那组色是彩色章底上的白字（Light 主题里是 neutral-50），写在卡片底色上会不可见。
    fn color(self, theme: &gpui_kit::component::theme::Theme) -> gpui::Hsla {
        match self {
            // Shadcn/Tailwind 文字用 600 阶：浅底可读，深底也能对出对比。
            DelayTint::Good => gpui::rgb(0x16a34a).into(),
            DelayTint::Mid => gpui::rgb(0xca8a04).into(),
            DelayTint::Bad => gpui::rgb(0xdc2626).into(),
            DelayTint::Neutral => theme.muted_foreground,
        }
    }
}

fn sort_nodes(
    nodes: &[crate::proxy_view::NodeInfo],
    key: SortKey,
) -> Vec<crate::proxy_view::NodeInfo> {
    let mut out = nodes.to_vec();
    match key {
        SortKey::Subscription => {}
        SortKey::Delay => out.sort_by_key(|r| delay_sort_key(r.delay, DEFAULT_TIMEOUT_MS)),
        SortKey::Name => out.sort_by(|a, b| a.name.cmp(&b.name)),
    }
    out
}
impl Ui {
    pub fn new(
        snapshot: crate::bridge::SharedSnapshot,
        cmds: std::rc::Rc<std::sync::mpsc::Sender<crate::bridge::UiCmd>>,
        preload_url: String,
    ) -> Self {
        let mut snap = snapshot.read().unwrap().clone();
        if snap.message.is_empty() {
            snap.message = "就绪".into();
        }
        let open_groups = snap.view.groups.iter().map(|g| g.name.clone()).collect();
        Self {
            snapshot,
            snap,
            sort: SortKey::Subscription,
            sub_input: None,
            sub_preload: preload_url,
            cmds,
            open_groups,
            open_ungrouped: true,
        }
    }

    fn render_sub_input(&mut self, w: &mut Window, cx: &mut Context<Self>) -> Entity<InputState> {
        self.sub_input
            .get_or_insert_with(|| {
                cx.new(|cx| {
                    InputState::new(w, cx)
                        .default_value(self.sub_preload.clone())
                        .placeholder("https://…/api/v1/client/subscribe?token=…")
                })
            })
            .clone()
    }

    fn pull_snapshot(&mut self) -> bool {
        let Ok(snap) = self.snapshot.read() else {
            return false;
        };
        let mut changed = false;
        if *snap != self.snap {
            self.snap = snap.clone();
            self.open_groups
                .retain(|name| self.snap.view.group(name).is_some());
            changed = true;
        }
        changed
    }

    fn render_group_bar(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let failed = self.snap.failed_delays.clone();
        let cmds = self.cmds.clone();
        let mut seen = std::collections::HashSet::new();
        let group_data: Vec<_> = self
            .snap
            .view
            .groups
            .iter()
            .enumerate()
            .map(|(index, group)| {
                let group_is_child = self
                    .snap
                    .view
                    .groups
                    .iter()
                    .any(|parent| parent.members.contains(&group.name));
                let nodes = group
                    .members
                    .iter()
                    .filter(|member| !self.snap.view.is_group(member) || group_is_child)
                    .filter_map(|member| {
                        self.snap.view.node(member).cloned().or_else(|| {
                            self.snap
                                .view
                                .group(member)
                                .map(|_| crate::proxy_view::NodeInfo {
                                    name: member.clone(),
                                    kind: "组".into(),
                                    delay: None,
                                    udp: false,
                                })
                        })
                    })
                    .collect::<Vec<_>>();
                let nodes = sort_nodes(&nodes, self.sort);
                seen.extend(nodes.iter().map(|node| node.name.clone()));
                (
                    index,
                    group.name.clone(),
                    group.kind.clone(),
                    group.now.clone(),
                    nodes,
                )
            })
            .collect();
        let group_sections = group_data
            .into_iter()
            .map(|(index, name, kind, selected, nodes)| {
                let count = nodes.len();
                // 自动类组标题显示内核实际生效节点的延迟（最小 ping）
                let auto_delay = if is_auto_kind(&kind) {
                    self.snap
                        .view
                        .group(&name)
                        .and_then(|g| auto_group_delay(&self.snap.view, g))
                } else {
                    None
                };
                let failed = failed.clone();
                let cmds = cmds.clone();
                let theme = theme.clone();
                let children = nodes
                    .into_iter()
                    .enumerate()
                    .map(|(ix, node)| {
                        let failed = failed.iter().any(|f| f == &node.name);
                        // 嵌套的自动类组行：显示该组内最小 ping（与标题同源）
                        let (delay, tint) = match self
                            .snap
                            .view
                            .group(&node.name)
                            .filter(|g| is_auto_kind(&g.kind))
                            .and_then(|g| auto_group_delay(&self.snap.view, g))
                        {
                            Some((ms, tint)) => (format!("{ms} ms"), tint),
                            None => (delay_text(&node, failed), delay_tint(node.delay, failed)),
                        };
                        let tint = tint.color(&theme);
                        let node_name = node.name.clone();
                        let selectable = !self.snap.view.is_group(&node.name);
                        let target_group = if selectable {
                            name.clone()
                        } else {
                            node.name.clone()
                        };
                        let node_cmds = cmds.clone();
                        ListItem::new(ElementId::named_usize("node-card", index * 10000 + ix))
                            .selected(selectable && selected.as_deref() == Some(&node.name))
                            .disabled(!selectable)
                            .accessibility_label(format!(
                                "{}，{}，{}，UDP {}",
                                node.name,
                                delay,
                                node.kind,
                                if node.udp { "支持" } else { "不支持" }
                            ))
                            .on_click(move |_, _, _| {
                                let _ = node_cmds.send(crate::bridge::UiCmd::Select {
                                    group: target_group.clone(),
                                    node: node_name.clone(),
                                });
                            })
                            .child(
                                div()
                                    .w_full()
                                    .flex()
                                    .justify_between()
                                    .child(
                                        div()
                                            .flex()
                                            .items_baseline()
                                            .gap_2()
                                            .child(label(node.name, TypeRole::Body))
                                            .child(
                                                label(node.kind, TypeRole::Meta)
                                                    .text_color(theme.muted_foreground),
                                            ),
                                    )
                                    .child(
                                        div()
                                            .flex()
                                            .gap_2()
                                            .child(
                                                label(
                                                    if node.udp { "UDP" } else { "" },
                                                    TypeRole::Meta,
                                                )
                                                .text_color(theme.muted_foreground),
                                            )
                                            .child(label(delay, TypeRole::Meta).text_color(tint)),
                                    ),
                            )
                            .into_any_element()
                    })
                    .collect::<Vec<_>>();
                let is_open = self.open_groups.contains(&name);
                div()
                    .id(ElementId::named_usize("proxy-group", index))
                    .test_support()
                    .w_full()
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .id(ElementId::named_usize("group-title", index))
                            .test_support()
                            .w_full()
                            .flex()
                            .justify_between()
                            .child(label(name.clone(), TypeRole::Section))
                            .child(
                                div()
                                    .flex()
                                    .gap_2()
                                    .children(auto_delay.map(|(delay, tint)| {
                                        label(format!("最优 {delay} ms"), TypeRole::Meta)
                                            .text_color(tint.color(&theme))
                                    }))
                                    .child(
                                        label(format!("{} · {}", kind, count), TypeRole::Meta)
                                            .text_color(theme.muted_foreground),
                                    )
                                    .child(label(if is_open { "▾" } else { "▸" }, TypeRole::Meta)),
                            )
                            .on_click(cx.listener({
                                let name = name.clone();
                                move |this, _, _, cx| {
                                    if !this.open_groups.remove(&name) {
                                        this.open_groups.insert(name.clone());
                                    }
                                    cx.notify();
                                }
                            })),
                    )
                    .when(is_open, |section| {
                        section.child(
                            // 内层面板不限高、不滚动：整页统一交给外层滚动容器。
                            // gpui 没有 scroll chaining，内层滚到底会吞掉滚轮，
                            // 导致下面的组和节点永远翻不到（见 probe_nested_scroll）。
                            div().w_full().children(children),
                        )
                    })
                    .into_any_element()
            })
            .collect::<Vec<_>>();
        let ungrouped: Vec<_> = self
            .snap
            .view
            .nodes
            .iter()
            .filter(|node| !seen.contains(&node.name))
            .cloned()
            .collect();
        let ungrouped = sort_nodes(&ungrouped, self.sort);
        let ungrouped_rows = ungrouped.iter().enumerate().map(|(ix, node)| {
            let failed = self.snap.failed_delays.iter().any(|f| f == &node.name);
            let delay = delay_text(node, failed);
            let tint = delay_tint(node.delay, failed).color(&theme);
            ListItem::new(ElementId::named_usize("ungrouped-node", ix))
                .disabled(true)
                .accessibility_label(format!("{}，未归组，{}", node.name, delay))
                .child(
                    div()
                        .flex()
                        .justify_between()
                        .child(label(node.name.clone(), TypeRole::Body))
                        .child(label(delay, TypeRole::Meta).text_color(tint)),
                )
                .into_any_element()
        });
        div()
            .id("proxy-groups")
            .test_support()
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .children(group_sections)
            .when(!ungrouped.is_empty(), |el| {
                el.child(
                    div()
                        .id("ungrouped-section")
                        .test_support()
                        .w_full()
                        .flex()
                        .flex_col()
                        .child(
                            div()
                                .id("ungrouped-title")
                                .test_support()
                                .w_full()
                                .flex()
                                .justify_between()
                                .child(label(
                                    format!("未归组节点 · {}", ungrouped.len()),
                                    TypeRole::Section,
                                ))
                                .child(label(
                                    if self.open_ungrouped { "▾" } else { "▸" },
                                    TypeRole::Meta,
                                ))
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.open_ungrouped = !this.open_ungrouped;
                                    cx.notify();
                                })),
                        )
                        .when(self.open_ungrouped, |section| {
                            section.child(
                                div()
                                    .id("ungrouped-panel")
                                    .test_support()
                                    .children(ungrouped_rows),
                            )
                        }),
                )
            })
    }
}

pub fn start_polling(entity: &Entity<Ui>, cx: &mut App) {
    cx.spawn({
        let weak = entity.downgrade();
        async move |cx| {
            loop {
                cx.background_executor()
                    .timer(std::time::Duration::from_millis(500))
                    .await;
                weak.update(cx, |ui: &mut Ui, cx| {
                    if ui.pull_snapshot() {
                        cx.notify();
                    }
                })
                .ok();
            }
            #[allow(unreachable_code)]
            let _cx: &mut AsyncApp = cx;
        }
    })
    .detach();
}

impl Render for Ui {
    fn render(&mut self, w: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let (core_on, tun_on, cap_missing) = (
            self.snap.core_running,
            self.snap.tun_enabled,
            self.snap.cap_missing,
        );
        let fg = cx.theme().foreground;
        let muted = cx.theme().muted_foreground;
        let empty = self.snap.view.nodes.is_empty();
        div()
            .size_full()
            .flex()
            .flex_col()
            .p(gpui::rems(2.))
            .gap_3()
            .text_color(fg)
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .justify_between()
                    .child(label("tailash", TypeRole::Title))
                    .child(label(self.snap.message.clone(), TypeRole::Meta).text_color(muted)),
            )
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_6()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(label("内核", TypeRole::Body))
                            .child(if core_on {
                                Tag::success().rounded_full().child("运行中")
                            } else {
                                Tag::secondary().rounded_full().child("未运行")
                            })
                            .when(self.snap.systemd_available, |row| {
                                row.child(
                                    Button::new(if core_on { "stop-core" } else { "start-core" })
                                        .label(if core_on {
                                            "停止内核"
                                        } else {
                                            "启动内核"
                                        })
                                        .on_click({
                                            let cmds = self.cmds.clone();
                                            move |_: &ClickEvent, _, _| {
                                                let _ = cmds.send(crate::bridge::UiCmd::ToggleCore);
                                            }
                                        }),
                                )
                            })
                            .when(cap_missing && !core_on, |row| {
                                row.child(Button::new("grant-cap").label("授权 TUN").on_click({
                                    let cmds = self.cmds.clone();
                                    move |_: &ClickEvent, _, _| {
                                        let _ = cmds.send(crate::bridge::UiCmd::GrantCap);
                                    }
                                }))
                            }),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(label("TUN 模式", TypeRole::Body))
                            .child(Switch::new("tun").checked(tun_on).on_click({
                                let cmds = self.cmds.clone();
                                move |on: &bool, _: &mut Window, _: &mut App| {
                                    let _ = cmds.send(crate::bridge::UiCmd::ToggleTun(*on));
                                }
                            })),
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .child(label("模式", TypeRole::Body))
                    .child(
                        Button::new("mode-rule")
                            .label("策略")
                            .selected(self.snap.mode == crate::mihomo_api::ProxyMode::Rule)
                            .on_click({
                                let cmds = self.cmds.clone();
                                cx.listener(move |_, _: &ClickEvent, _, _| {
                                    let _ = cmds.send(crate::bridge::UiCmd::SetMode(
                                        crate::mihomo_api::ProxyMode::Rule,
                                    ));
                                })
                            }),
                    )
                    .child(
                        Button::new("mode-direct")
                            .label("直连")
                            .selected(self.snap.mode == crate::mihomo_api::ProxyMode::Direct)
                            .on_click({
                                let cmds = self.cmds.clone();
                                cx.listener(move |_, _: &ClickEvent, _, _| {
                                    let _ = cmds.send(crate::bridge::UiCmd::SetMode(
                                        crate::mihomo_api::ProxyMode::Direct,
                                    ));
                                })
                            }),
                    )
                    .child(
                        Button::new("mode-global")
                            .label("全局")
                            .selected(self.snap.mode == crate::mihomo_api::ProxyMode::Global)
                            .on_click({
                                let cmds = self.cmds.clone();
                                cx.listener(move |_, _: &ClickEvent, _, _| {
                                    let _ = cmds.send(crate::bridge::UiCmd::SetMode(
                                        crate::mihomo_api::ProxyMode::Global,
                                    ));
                                })
                            }),
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .child(label("当前选择与可用节点", TypeRole::Section))
                    .child(div().flex_1())
                    .child(
                        Button::new("sort-cycle")
                            .label(self.sort.label())
                            .selected(self.sort != SortKey::Subscription)
                            .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                                this.sort = this.sort.next();
                                cx.notify();
                            })),
                    )
                    .child(Button::new("test-all").label("测全部延迟").on_click({
                        let cmds = self.cmds.clone();
                        move |_: &ClickEvent, _: &mut Window, _: &mut App| {
                            let _ = cmds.send(crate::bridge::UiCmd::TestAll);
                        }
                    })),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(label("配置托管", TypeRole::Section))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(
                                div()
                                    .flex_1()
                                    .child(Input::new(&self.render_sub_input(w, cx))),
                            )
                            .child(Button::new("update-sub").label("更新订阅").on_click({
                                let cmds = self.cmds.clone();
                                cx.listener(move |this, _: &ClickEvent, _, cx| {
                                    let url = this
                                        .sub_input
                                        .clone()
                                        .map(|state| state.read(cx).value().to_string())
                                        .unwrap_or_default();
                                    let _ = cmds.send(crate::bridge::UiCmd::UpdateSub { url });
                                })
                            })),
                    ),
            )
            .when(empty, |d| {
                d.child(
                    div()
                        .flex_1()
                        .min_h_0()
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(
                            label("暂无节点：请更新订阅并启动内核", TypeRole::Meta)
                                .text_color(muted),
                        ),
                )
            })
            .when(!empty, |d| d.child(self.render_group_bar(cx)))
    }
}

#[cfg(test)]
mod tests {
    use super::{DelayTint, SortKey, delay_tint, sort_nodes};
    use crate::proxy_view::NodeInfo;
    fn rows(pairs: &[(u32, u32, Option<u32>)]) -> Vec<NodeInfo> {
        pairs
            .iter()
            .map(|&(ix, seed, delay)| NodeInfo {
                name: format!("节点{ix}-{seed}"),
                kind: "Vmess".into(),
                delay,
                udp: false,
            })
            .collect()
    }
    #[test]
    fn sort_by_delay_orders_measured_first_and_fastest() {
        let nodes = rows(&[
            (1, 9, Some(300)),
            (2, 1, None),
            (3, 7, Some(120)),
            (4, 3, Some(5000)),
            (5, 5, None),
        ]);
        let got: Vec<String> = sort_nodes(&nodes, SortKey::Delay)
            .iter()
            .map(|r| r.name.clone())
            .collect();
        assert_eq!(
            got,
            vec!["节点3-7", "节点1-9", "节点4-3", "节点2-1", "节点5-5"]
        );
    }
    #[test]
    fn sort_by_name_is_alphabetical() {
        let got: Vec<String> = sort_nodes(
            &rows(&[(1, 3, None), (2, 1, Some(9)), (3, 2, None)]),
            SortKey::Name,
        )
        .iter()
        .map(|r| r.name.clone())
        .collect();
        assert_eq!(got, vec!["节点1-3", "节点2-1", "节点3-2"]);
    }
    #[test]
    fn sort_default_keeps_subscription_order() {
        let got: Vec<String> = sort_nodes(
            &rows(&[(1, 0, None), (2, 0, None), (3, 0, None)]),
            SortKey::Subscription,
        )
        .iter()
        .map(|r| r.name.clone())
        .collect();
        assert_eq!(got, vec!["节点1-0", "节点2-0", "节点3-0"]);
    }
    #[test]
    fn sort_preserves_stability_for_equal_delays() {
        let got: Vec<String> = sort_nodes(
            &rows(&[(1, 0, Some(88)), (2, 0, Some(88)), (3, 0, Some(44))]),
            SortKey::Delay,
        )
        .iter()
        .map(|r| r.name.clone())
        .collect();
        assert_eq!(got, vec!["节点3-0", "节点1-0", "节点2-0"]);
    }
    #[test]
    fn sort_cycle_returns_to_default() {
        let mut key = SortKey::Subscription;
        key = key.next();
        assert_eq!(key, SortKey::Delay);
        key = key.next();
        assert_eq!(key, SortKey::Name);
        key = key.next();
        assert_eq!(key, SortKey::Subscription);
    }

    #[test]
    fn tint_follows_delay_class_green_yellow_red_muted() {
        assert_eq!(delay_tint(Some(120), false), DelayTint::Good);
        assert_eq!(delay_tint(Some(199), false), DelayTint::Good);
        assert_eq!(delay_tint(Some(200), false), DelayTint::Mid);
        assert_eq!(delay_tint(Some(450), false), DelayTint::Mid);
        assert_eq!(delay_tint(Some(999), false), DelayTint::Mid);
        assert_eq!(delay_tint(Some(1000), false), DelayTint::Bad);
        assert_eq!(delay_tint(Some(4999), false), DelayTint::Bad);
        assert_eq!(delay_tint(Some(0), false), DelayTint::Bad);
        assert_eq!(delay_tint(Some(200_000), false), DelayTint::Bad);
        assert_eq!(delay_tint(None, true), DelayTint::Bad);
        assert_eq!(delay_tint(None, false), DelayTint::Neutral);
    }
}
