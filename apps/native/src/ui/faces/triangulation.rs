use zgui::prelude::*;

use crate::store::Store;

pub fn face(_store: Store, _node: String) -> impl IntoView {
    super::plain("triangulation")
}
