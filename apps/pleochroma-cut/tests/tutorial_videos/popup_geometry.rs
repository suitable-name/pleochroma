//! Geometry and identity for controls in embedded popup windows on the tutorial testing backend.
//! The harness keeps Slint's native popup mode disabled, so this helper intentionally rejects a
//! popup whose adapter differs from the main window instead of guessing a screen coordinate.

use crate::{support::ElementHandle, targets::Spot};
use pleochroma_cut::MainWindow;
use slint::{
    ComponentHandle,
    private_unstable_api::re_exports::{ItemRc, LogicalPoint, WindowInner},
};
use std::{collections::HashSet, ops::ControlFlow, rc::Rc};
use uiprobe::{AccessibleRole, view::WindowView};

pub struct PopupGeometry {
    component: &'static str,
    local_x: f32,
    local_y: f32,
    width: f32,
    height: f32,
    offset_x: f32,
    offset_y: f32,
    types_and_ids: HashSet<(String, String)>,
}

impl PopupGeometry {
    pub(crate) fn find(ui: &MainWindow, component: &'static str) -> Result<Self, String> {
        let inner = WindowInner::from_pub(ui.window());
        let main_adapter = inner.window_adapter();
        let popups = inner.active_popups();
        let mut candidates = Vec::new();
        let mut observed = Vec::new();
        for popup in popups.iter() {
            let root = ItemRc::new_root(popup.component.clone());
            let mut names = HashSet::new();
            let mut types_and_ids = HashSet::new();
            let mut collect = |item: &ItemRc| {
                for index in 0..item.element_count().unwrap_or_default() {
                    for (type_name, id) in
                        item.element_type_names_and_ids(index).unwrap_or_default()
                    {
                        let type_name = type_name.to_string();
                        let id = id.to_string();
                        names.insert(type_name.clone());
                        types_and_ids.insert((type_name, id));
                    }
                }
                ControlFlow::<()>::Continue(())
            };
            let _ = collect(&root);
            let _ = root.visit_descendants(&mut collect);
            let mut observed_names: Vec<_> = names.iter().cloned().collect();
            observed_names.sort();
            observed.push(observed_names);
            if !names.contains(component) {
                continue;
            }
            let popup_adapter = root
                .window_adapter()
                .ok_or_else(|| format!("{component} popup has no window adapter"))?;
            if !Rc::ptr_eq(&main_adapter, &popup_adapter) {
                return Err(format!(
                    "{component} popup belongs to a different native window"
                ));
            }
            let geometry = root.geometry();
            let origin = LogicalPoint::new(0.0, 0.0);
            let local = root.map_to_window(origin);
            let native = root.map_to_native_window(origin);
            let (width, height) = (geometry.size.width, geometry.size.height);
            let (offset_x, offset_y) = (native.x - local.x, native.y - local.y);
            if !width.is_finite()
                || !height.is_finite()
                || width <= 0.0
                || height <= 0.0
                || !offset_x.is_finite()
                || !offset_y.is_finite()
            {
                return Err(format!(
                    "{component} popup has invalid mapped geometry: {width}x{height} at ({offset_x}, {offset_y})"
                ));
            }
            candidates.push(Self {
                component,
                local_x: local.x + geometry.origin.x,
                local_y: local.y + geometry.origin.y,
                width,
                height,
                offset_x,
                offset_y,
                types_and_ids,
            });
        }
        match candidates.len() {
            1 => Ok(candidates.remove(0)),
            count => Err(format!(
                "expected exactly one active {component} popup, found {count}; active popup element types: {observed:?}"
            )),
        }
    }

    fn owns(&self, element: &ElementHandle, spot: &Spot) -> bool {
        let type_name = element.type_name().map(|name| name.to_string());
        let id = element.id().map(|id| id.to_string()).unwrap_or_default();
        let belongs = type_name.is_some_and(|name| self.types_and_ids.contains(&(name, id)));
        belongs
            && spot.x + spot.w / 2.0 >= self.local_x
            && spot.x + spot.w / 2.0 <= self.local_x + self.width
            && spot.y + spot.h / 2.0 >= self.local_y
            && spot.y + spot.h / 2.0 <= self.local_y + self.height
    }

    pub(crate) fn bounds(&self) -> Spot {
        Spot {
            x: self.local_x + self.offset_x,
            y: self.local_y + self.offset_y,
            w: self.width,
            h: self.height,
            handle: None,
        }
    }

    pub(crate) fn map_spot(&self, mut spot: Spot) -> Spot {
        spot.x += self.offset_x;
        spot.y += self.offset_y;
        spot
    }

    pub(crate) fn control(
        &self,
        ui: &MainWindow,
        label: &str,
        role: AccessibleRole,
    ) -> Result<Spot, String> {
        let view = WindowView::of("MainWindow", ui);
        let matches: Vec<_> = view
            .root
            .query_descendants()
            .match_accessible_role(role)
            .find_all()
            .into_iter()
            .filter(|element| element.accessible_label().is_some_and(|text| text == label))
            .map(|element| {
                let spot = Spot::of(&element);
                (element, spot)
            })
            .filter(|(element, spot)| self.owns(element, spot))
            .collect();
        match matches.len() {
            1 => {
                let (_, spot) = matches
                    .into_iter()
                    .next()
                    .ok_or("popup control disappeared")?;
                let spot = self.map_spot(spot);
                if spot.usable(view.size) {
                    Ok(spot)
                } else {
                    Err(format!(
                        "{label:?} in {} popup is outside the main window after mapping",
                        self.component
                    ))
                }
            }
            count => Err(format!(
                "expected one {role:?} labelled {label:?} in {} popup, found {count}",
                self.component
            )),
        }
    }
}
