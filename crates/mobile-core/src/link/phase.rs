use std::time::Duration;

pub(crate) const BACKGROUND_GRACE: Duration = Duration::from_secs(30);

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Activity {
    pub(crate) background: bool,
    pub(crate) mission_open: bool,
    pub(crate) pose_needed: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum KeepAlive {
    Full,
    Lean,
    DropAfter(Duration),
}

pub(crate) fn keep_alive(activity: Activity) -> KeepAlive {
    match (
        activity.background,
        activity.mission_open || activity.pose_needed,
    ) {
        (false, _) => KeepAlive::Full,
        (true, true) => KeepAlive::Lean,
        (true, false) => KeepAlive::DropAfter(BACKGROUND_GRACE),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keep_alive_table() {
        let cases = [
            (false, false, false, KeepAlive::Full),
            (false, true, false, KeepAlive::Full),
            (false, false, true, KeepAlive::Full),
            (true, true, false, KeepAlive::Lean),
            (true, false, true, KeepAlive::Lean),
            (true, true, true, KeepAlive::Lean),
            (true, false, false, KeepAlive::DropAfter(BACKGROUND_GRACE)),
        ];
        for (background, mission_open, pose_needed, expected) in cases {
            let activity = Activity {
                background,
                mission_open,
                pose_needed,
            };
            assert_eq!(keep_alive(activity), expected, "{activity:?}");
        }
    }
}
