use axum::{extract::MatchedPath, http::Method};
use sdrmm_wire::{ClientCommand, ServerEvent};

pub(crate) const PHONE_ROUTES: &[(Method, &str)] = &[
    (Method::GET, "/api/about"),
    (Method::GET, "/api/state"),
    (Method::GET, "/api/missions"),
    (Method::POST, "/api/missions/workspace"),
    (Method::POST, "/api/missions/{node}/actions"),
    (Method::GET, "/api/survey/{node}"),
    (Method::GET, "/api/fusion/{node}"),
    (Method::GET, "/api/radar/{node}"),
    (Method::GET, "/api/phones/self"),
    (Method::DELETE, "/api/phones/self"),
    (Method::GET, "/api/ws"),
];

pub(crate) fn phone_may(method: &Method, matched: Option<&MatchedPath>) -> bool {
    matched.is_some_and(|path| {
        PHONE_ROUTES
            .iter()
            .any(|(allowed, route)| allowed == method && *route == path.as_str())
    })
}

pub(crate) fn phone_event(event: &ServerEvent) -> bool {
    matches!(
        event,
        ServerEvent::Hello { .. }
            | ServerEvent::StateChanged { .. }
            | ServerEvent::Error { .. }
            | ServerEvent::PositionChanged { .. }
            | ServerEvent::HuntUpdate { .. }
            | ServerEvent::ProcessorUpdate { .. }
            | ServerEvent::ArrayUpdate { .. }
            | ServerEvent::DfFusionUpdate { .. }
            | ServerEvent::SurveyUpdate { .. }
            | ServerEvent::SurfaceStreamStarted { .. }
            | ServerEvent::StreamStopped { .. }
    )
}

pub(crate) fn phone_command(command: &ClientCommand) -> bool {
    matches!(
        command,
        ClientCommand::PublishPose { .. }
            | ClientCommand::SubscribeSurface { .. }
            | ClientCommand::UnsubscribeSurface { .. }
    )
}
