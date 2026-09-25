use crate::store::Store;

pub fn open(store: Store, tool: Option<&str>) {
    tracing::debug!(?tool, "tools dialog requested");
    store.note("Tools are not in this build yet");
}
