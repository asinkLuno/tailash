//! 夹具：以生产入口 open_window 打开确定性尺寸的测试窗口（拷贝自 gpui-kit 测试夹具）。
#![allow(dead_code)]
use gpui_kit::{
    App, Bounds, Entity, Pixels, Point, Render, Size, TestAppContext, Window, WindowBounds,
    WindowHandle, WindowOptions, base::Root,
};

pub fn open_window<V: Render>(
    cx: &mut TestAppContext,
    size: Option<Size<Pixels>>,
    build: impl FnOnce(&mut Window, &mut App) -> Entity<V>,
) -> (WindowHandle<Root>, Entity<V>) {
    cx.update(|cx| {
        let bounds = size
            .map(|size| Bounds {
                origin: Point::default(),
                size,
            })
            .unwrap_or_else(|| Bounds::maximized(None, cx));
        let (window, content) = gpui_kit::open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                ..Default::default()
            },
            cx,
            build,
        )
        .expect("open test window");
        (window.downcast::<Root>().expect("Base Root"), content)
    })
}
