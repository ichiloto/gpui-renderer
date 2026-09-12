//! Opt-in observations around GPUI's public element lifecycle, not GPU completion.
use crate::diagnostics::{Diagnostics, FrameTrace};
use gpui::{
    AnyElement, App, Bounds, Element, ElementId, GlobalElementId, InspectorElementId, IntoElement,
    LayoutId, Pixels, Window,
};

pub fn observe(
    element: impl IntoElement,
    diagnostics: &Diagnostics,
    observation: Option<FrameTrace>,
) -> AnyElement {
    diagnostics.frame_stage(observation, "elements_built");
    match observation {
        Some(observation) => Observed {
            element: element.into_element(),
            diagnostics: diagnostics.clone(),
            observation,
        }
        .into_any_element(),
        None => element.into_any_element(),
    }
}

struct Observed<E> {
    element: E,
    diagnostics: Diagnostics,
    observation: FrameTrace,
}

impl<E: Element> IntoElement for Observed<E> {
    type Element = Self;
    fn into_element(self) -> Self {
        self
    }
}

impl<E: Element> Element for Observed<E> {
    type RequestLayoutState = E::RequestLayoutState;
    type PrepaintState = E::PrepaintState;

    fn id(&self) -> Option<ElementId> {
        self.element.id()
    }
    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        self.element.source_location()
    }

    fn request_layout(
        &mut self,
        id: Option<&GlobalElementId>,
        inspector_id: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        self.diagnostics
            .frame_stage(Some(self.observation), "layout_request_begin");
        let result = self.element.request_layout(id, inspector_id, window, cx);
        self.diagnostics
            .frame_stage(Some(self.observation), "layout_request_end");
        result
    }

    fn prepaint(
        &mut self,
        id: Option<&GlobalElementId>,
        inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        request_layout: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> Self::PrepaintState {
        self.diagnostics
            .frame_stage(Some(self.observation), "prepaint_begin");
        let result = self
            .element
            .prepaint(id, inspector_id, bounds, request_layout, window, cx);
        self.diagnostics
            .frame_stage(Some(self.observation), "prepaint_end");
        result
    }

    fn paint(
        &mut self,
        id: Option<&GlobalElementId>,
        inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        request_layout: &mut Self::RequestLayoutState,
        prepaint: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        self.diagnostics
            .frame_stage(Some(self.observation), "paint_begin");
        self.element.paint(
            id,
            inspector_id,
            bounds,
            request_layout,
            prepaint,
            window,
            cx,
        );
        self.diagnostics
            .frame_stage(Some(self.observation), "paint_end");
    }
}
