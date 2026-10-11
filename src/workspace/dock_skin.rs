use std::{
    cell::{Cell, RefCell},
    rc::Rc,
    sync::Arc,
};

use gpui_kit::component::dock::{
    BasePanelView, DockArea, DockAreaRenderer, DockContext, DockSkin, NodeId, PanelState,
    TabGroupRenderer, TilesRenderer,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use super::compose::COMPOSE_HEIGHT;

/// The workspace's dock appearance: gpui-kit's `DockSkin`, plus a view that
/// fills the center while it holds no tab.
///
/// The dock draws nothing for an empty center and `DockAreaRenderer` has no
/// empty-center hook, so the placeholder rides on `center_frame`. It is drawn
/// deferred, above the empty layout the dock paints there, whenever the
/// workspace last said the center is empty (`set_center_empty`); the
/// workspace keeps that in step with `DockEvent::LayoutChanged`.
pub(super) struct WorkspaceDockSkin {
    skin: Rc<DockSkin>,
    area: WeakEntity<DockArea>,
    placeholder: AnyView,
    center_empty: Cell<bool>,
    /// 撰写, while it is open. It rides on `center_frame` too: the bottom
    /// dock would take tabs dragged into it and keep a strip when closed.
    compose: RefCell<Option<AnyView>>,
}

impl WorkspaceDockSkin {
    /// Build a `DockArea` wearing this skin, together with the handle its
    /// settings are changed through. `placeholder` is what the center shows
    /// while it is empty.
    pub(super) fn dock_area(
        id: &'static str,
        version: Option<usize>,
        placeholder: AnyView,
        window: &mut Window,
        cx: &mut App,
    ) -> (Entity<DockArea>, Rc<Self>) {
        let mut skin = None;
        let area = cx.new(|cx| {
            let this = Rc::new(Self {
                skin: DockSkin::new(cx),
                area: cx.weak_entity(),
                placeholder,
                center_empty: Cell::new(false),
                compose: RefCell::new(None),
            });
            skin = Some(this.clone());
            DockArea::new(id, version, window, cx).with_renderer(this)
        });
        // The closure above runs before `cx.new` returns.
        (
            area,
            skin.expect("the skin is built inside the constructor"),
        )
    }

    pub(super) fn is_center_empty(&self) -> bool {
        self.center_empty.get()
    }

    /// Show (`true`) or hide the placeholder. The skin is not an entity, so
    /// it redraws the area itself when the value changes.
    pub(super) fn set_center_empty(&self, empty: bool, cx: &mut App) {
        if self.center_empty.replace(empty) != empty {
            _ = self.area.update(cx, |_, cx| cx.notify());
        }
    }

    /// Show 撰写 (`Some`) under the center's tabs, or take it away. Redraws
    /// only when that changes: it is asked on every move of the tab in front.
    pub(super) fn set_compose(&self, compose: Option<AnyView>, cx: &mut App) {
        let changed = self.compose.borrow().is_some() != compose.is_some();
        self.compose.replace(compose);
        if changed {
            _ = self.area.update(cx, |_, cx| cx.notify());
        }
    }
}

impl DockAreaRenderer for WorkspaceDockSkin {
    fn frame(&self, window: &mut Window, cx: &mut App) -> Stateful<Div> {
        self.skin.frame(window, cx)
    }

    fn split_frame(
        &self,
        node: NodeId,
        axis: Axis,
        window: &mut Window,
        cx: &mut App,
    ) -> Stateful<Div> {
        self.skin.split_frame(node, axis, window, cx)
    }

    fn center_frame(&self, window: &mut Window, cx: &mut App) -> Stateful<Div> {
        let compose = self.compose.borrow().clone();
        self.skin
            .center_frame(window, cx)
            .relative()
            .when_some(compose, |frame, compose| {
                frame.pb(COMPOSE_HEIGHT).child(
                    div()
                        .absolute()
                        .left_0()
                        .right_0()
                        .bottom_0()
                        .h(COMPOSE_HEIGHT)
                        .child(compose),
                )
            })
            .when(self.center_empty.get(), |frame| {
                // Deferred, so it paints after the empty layout's own
                // background, which the dock draws later in the same frame.
                frame.child(deferred(
                    div().absolute().inset_0().child(self.placeholder.clone()),
                ))
            })
    }

    fn render_dock(
        &self,
        dock: &DockContext,
        content: AnyElement,
        window: &mut Window,
        cx: &mut App,
    ) -> AnyElement {
        self.skin.render_dock(dock, content, window, cx)
    }

    fn build_placeholder(
        &self,
        state: &PanelState,
        window: &mut Window,
        cx: &mut App,
    ) -> Option<Arc<dyn BasePanelView>> {
        self.skin.build_placeholder(state, window, cx)
    }

    fn tab_group_renderer(&self) -> Rc<dyn TabGroupRenderer> {
        self.skin.tab_group_renderer()
    }

    fn tiles_renderer(&self) -> Rc<dyn TilesRenderer> {
        self.skin.tiles_renderer()
    }
}
