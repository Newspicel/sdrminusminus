use tokio::sync::watch;

#[derive(Debug)]
pub(crate) struct Window {
    available: watch::Sender<i64>,
}

impl Window {
    pub(crate) fn new(initial: u32) -> Self {
        Self {
            available: watch::Sender::new(i64::from(initial)),
        }
    }

    pub(crate) fn grant(&self, bytes: u32) {
        self.available
            .send_modify(|available| *available = available.saturating_add(i64::from(bytes)));
    }

    pub(crate) async fn spend(&self, bytes: usize) {
        let mut open = self.available.subscribe();
        if open.wait_for(|available| *available > 0).await.is_err() {
            return;
        }
        let bytes = i64::try_from(bytes).unwrap_or(i64::MAX);
        self.available
            .send_modify(|available| *available = available.saturating_sub(bytes));
    }

    #[cfg(test)]
    pub(crate) fn available(&self) -> i64 {
        *self.available.borrow()
    }
}

#[cfg(test)]
mod tests {
    use std::{sync::Arc, time::Duration};

    use super::*;

    #[tokio::test]
    async fn spending_may_overdraw_once_then_waits_for_a_grant() {
        let window = Arc::new(Window::new(10));
        window.spend(25).await;
        assert_eq!(window.available(), -15);
        let blocked = tokio::spawn({
            let window = window.clone();
            async move { window.spend(1).await }
        });
        tokio::time::sleep(Duration::from_millis(20)).await;
        assert!(!blocked.is_finished());
        window.grant(15);
        tokio::time::sleep(Duration::from_millis(20)).await;
        assert!(!blocked.is_finished());
        window.grant(1);
        tokio::time::timeout(Duration::from_secs(1), blocked)
            .await
            .expect("grant releases the sender")
            .expect("spend task");
        assert_eq!(window.available(), 0);
    }
}
