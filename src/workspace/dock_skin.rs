use std::{
    cell::{Cell, RefCell},
    rc::Rc,
    sync::Arc,
};

use gpui_kit::component::{
    ActiveTheme as _, IconName, Sizable as _,
    button::{Button, ButtonVariants as _},
    dock::{
        BasePanelView, DockArea, DockAreaRenderer, DockContext, DockSkin, DropIndicator, NodeId,
        PanelId, PanelState, TabGroupContext, TabGroupRenderer, TilesRenderer,
    },
    h_flex,
    menu::{DropdownMenu as _, PopupMenuItem},
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use super::compose::COMPOSE_HEIGHT;
use crate::i18n::t;

/// The name of a center tab, by its panel, for 所有标签; `None` for a panel
/// that is not one, such as a sidebar.
pub(super) type TabLabels = Rc<dyn Fn(PanelId, &App) -> Option<SharedString>>;

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
    /// The workspace's names for its tabs, once it is there to give them.
    tab_labels: Rc<RefCell<Option<TabLabels>>>,
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
                tab_labels: Rc::default(),
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

    /// How the workspace names its tabs, for 所有标签.
    pub(super) fn set_tab_labels(&self, labels: TabLabels) {
        self.tab_labels.replace(Some(labels));
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
        Rc::new(TabGroupWithList {
            inner: self.skin.tab_group_renderer(),
            labels: self.tab_labels.clone(),
        })
    }

    fn tiles_renderer(&self) -> Rc<dyn TilesRenderer> {
        self.skin.tiles_renderer()
    }
}

/// gpui-kit's tab groups, with 所有标签 at the end of a center group's tab
/// bar: a menu of its tabs, the one shown checked, that brings the one
/// picked into view. The bar scrolls only sideways, which a mouse wheel does
/// not, so the tabs off its ends are reached here.
struct TabGroupWithList {
    inner: Rc<dyn TabGroupRenderer>,
    labels: Rc<RefCell<Option<TabLabels>>>,
}

impl TabGroupRenderer for TabGroupWithList {
    fn frame(&self, group: &TabGroupContext, window: &mut Window, cx: &mut App) -> Stateful<Div> {
        self.inner.frame(group, window, cx)
    }

    fn content_frame(
        &self,
        group: &TabGroupContext,
        window: &mut Window,
        cx: &mut App,
    ) -> Stateful<Div> {
        self.inner.content_frame(group, window, cx)
    }

    fn render_tab_bar(
        &self,
        group: &TabGroupContext,
        window: &mut Window,
        cx: &mut App,
    ) -> AnyElement {
        let bar = self.inner.render_tab_bar(group, window, cx);
        // Only for a center group of more than one tab.
        let labels = self.labels.borrow().clone();
        let Some(labels) = labels
            .and_then(|labels| {
                group
                    .panels()
                    .iter()
                    .map(|panel| labels(panel.panel_id(cx), cx))
                    .collect::<Option<Vec<_>>>()
            })
            .filter(|labels| labels.len() > 1)
        else {
            return bar;
        };
        let (shown, group) = (group.active_ix(), group.clone());
        h_flex()
            .w_full()
            .items_stretch()
            .child(div().flex_1().min_w_0().child(bar))
            .child(
                h_flex()
                    .flex_shrink_0()
                    .items_center()
                    .px_1()
                    .border_b_1()
                    .border_color(cx.theme().border)
                    .bg(cx.theme().tokens.tab_bar)
                    .child(
                        Button::new("all-tabs")
                            .ghost()
                            .xsmall()
                            .icon(IconName::ChevronDown)
                            .tooltip(t!("workspace.tabs.all"))
                            .dropdown_menu(move |mut menu, _, _| {
                                menu = menu.scrollable(true);
                                for (ix, label) in labels.iter().enumerate() {
                                    let group = group.clone();
                                    menu = menu.item(
                                        PopupMenuItem::new(label.clone())
                                            .checked(ix == shown)
                                            .on_click(move |_, window, cx| {
                                                group.select_tab(ix, window, cx)
                                            }),
                                    );
                                }
                                menu
                            })
                            .anchor(Anchor::TopRight),
                    ),
            )
            .into_any_element()
    }

    fn render_active_panel(
        &self,
        panel: AnyView,
        group: &TabGroupContext,
        window: &mut Window,
        cx: &mut App,
    ) -> AnyElement {
        self.inner.render_active_panel(panel, group, window, cx)
    }

    fn render_drop_indicator(
        &self,
        indicator: DropIndicator,
        window: &mut Window,
        cx: &mut App,
    ) -> Option<AnyElement> {
        self.inner.render_drop_indicator(indicator, window, cx)
    }

    fn render_empty(
        &self,
        group: &TabGroupContext,
        window: &mut Window,
        cx: &mut App,
    ) -> Option<AnyElement> {
        self.inner.render_empty(group, window, cx)
    }
}
