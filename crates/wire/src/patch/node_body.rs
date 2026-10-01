macro_rules! port_row {
    (array_in) => {
        PortSpec::named(ARRAY_PORT, PortType::Array, PortDirection::In, false)
    };
    (events_out($note:literal)) => {
        PortSpec::named(EVENTS_PORT, PortType::Events, PortDirection::Out, true).noted($note)
    };
    (steer_in($note:literal)) => {
        PortSpec::named(STEER_PORT, PortType::Events, PortDirection::In, false).noted($note)
    };
    (beam_out($note:literal)) => {
        PortSpec::named(BEAM_PORT, PortType::Iq, PortDirection::Out, true).noted($note)
    };
    (wide_out($note:literal)) => {
        PortSpec::named(STITCH_WIDE_PORT, PortType::Iq, PortDirection::Out, true).noted($note)
    };
    (tx_in($note:literal)) => {
        PortSpec::named(RADAR_TX_PORT, PortType::Position, PortDirection::In, false).noted($note)
    };
    (adsb_in($note:literal)) => {
        PortSpec::named(RADAR_TRUTH_PORT, PortType::Events, PortDirection::In, true).noted($note)
    };
}

macro_rules! lane_outputs {
    ([$($found:expr),*]) => {
        &[$($found),*]
    };
    ([$($found:expr),*] beam_out $(, $($rest:ident),*)?) => {
        lane_outputs!([$($found,)* BEAM_PORT] $($($rest),*)?)
    };
    ([$($found:expr),*] wide_out $(, $($rest:ident),*)?) => {
        lane_outputs!([$($found,)* STITCH_WIDE_PORT] $($($rest),*)?)
    };
    ([$($found:expr),*] $skip:ident $(, $($rest:ident),*)?) => {
        lane_outputs!([$($found),*] $($($rest),*)?)
    };
}

macro_rules! define_node_body {
    (
        processors: [$((
            $variant:ident,
            $type_id:literal,
            $params:ty,
            $reading:ty,
            $node:ident,
            $name:literal,
            $summary:literal,
            [$($port:ident $(($note:literal))?),* $(,)?]
        )),* $(,)?],
        probe: ($probe:ident, $probe_id:literal, $probe_params:ty) $(,)?
    ) => {
        $(
            #[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, ToSchema)]
            pub struct $node {
                #[serde(default)]
                pub settings: $params,
            }
        )*

        #[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
        #[serde(tag = "kind", content = "data", rename_all = "snake_case")]
        pub enum NodeBody {
            Device(DeviceNode),
            Recording(RecordingNode),
            SignalGen(SignalGenNode),
            Array(crate::array::ArrayNode),
            Gps(GpsNode),
            Channel(ChannelNode),
            Scope,
            BasebandScope,
            Speaker,
            Map,
            SignalMap(SignalMapNode),
            Propagation(PropagationNode),
            Readout,
            DecoderLog,
            DmrTrunk(DmrTrunkNode),
            SpectrumMonitor(crate::SpectrumMonitorNode),
            EventOutput(EventOutputNode),
            EventFilter(EventFilterNode),
            AudioFx(crate::AudioFxNode),
            Video,
            Recorder(RecorderNode),
            AudioRecorder(RecorderNode),
            BasebandRecorder(RecorderNode),
            TimeMachine(TimeMachineNode),
            NetworkExport(NetworkExportNode),
            Export,
            Scanner,
            Hunt(HuntNode),
            Satellite(SatelliteNode),
            $(
                #[serde(rename = $type_id)]
                $variant($node),
            )*
            Triangulation(TriangulationNode),
        }

        impl NodeBody {
            #[must_use]
            pub const fn kind(&self) -> &'static str {
                match self {
                    Self::Device(_) => "device",
                    Self::Recording(_) => "recording",
                    Self::SignalGen(_) => "signal_gen",
                    Self::Array(_) => "array",
                    Self::Gps(_) => "gps",
                    Self::Channel(_) => "channel",
                    Self::Scope => "scope",
                    Self::BasebandScope => "baseband_scope",
                    Self::Speaker => "speaker",
                    Self::Map => "map",
                    Self::SignalMap(_) => "signal_map",
                    Self::Propagation(_) => "propagation",
                    Self::Readout => "readout",
                    Self::DecoderLog => "decoder_log",
                    Self::DmrTrunk(_) => "dmr_trunk",
                    Self::SpectrumMonitor(_) => "spectrum_monitor",
                    Self::EventOutput(_) => "event_output",
                    Self::EventFilter(_) => "event_filter",
                    Self::AudioFx(_) => "audio_fx",
                    Self::Video => "video",
                    Self::Recorder(_) => "recorder",
                    Self::AudioRecorder(_) => "audio_recorder",
                    Self::BasebandRecorder(_) => "baseband_recorder",
                    Self::TimeMachine(_) => "time_machine",
                    Self::NetworkExport(_) => "network_export",
                    Self::Export => "export",
                    Self::Scanner => "scanner",
                    Self::Hunt(_) => "hunt",
                    Self::Satellite(_) => "satellite",
                    $(Self::$variant(_) => $type_id,)*
                    Self::Triangulation(_) => "triangulation",
                }
            }

            #[must_use]
            pub const fn category(&self) -> NodeCategory {
                match self {
                    Self::Device(_) | Self::Recording(_) | Self::SignalGen(_) | Self::Gps(_) => {
                        NodeCategory::Source
                    }
                    Self::Channel(_) => NodeCategory::Channel,
                    Self::Array(_)
                    | Self::Scanner
                    | Self::Hunt(_)
                    | Self::Satellite(_)
                    | Self::SpectrumMonitor(_)
                    | Self::DmrTrunk(_)
                    | Self::EventFilter(_)
                    | Self::AudioFx(_)
                    | Self::Triangulation(_)
                    $(| Self::$variant(_))* => NodeCategory::Tool,
                    Self::Scope
                    | Self::BasebandScope
                    | Self::Map
                    | Self::SignalMap(_)
                    | Self::Propagation(_)
                    | Self::Readout
                    | Self::DecoderLog
                    | Self::Video
                    | Self::Speaker
                    | Self::Recorder(_)
                    | Self::AudioRecorder(_)
                    | Self::BasebandRecorder(_)
                    | Self::TimeMachine(_)
                    | Self::NetworkExport(_)
                    | Self::EventOutput(_)
                    | Self::Export => NodeCategory::Output,
                }
            }

            #[must_use]
            pub const fn is_array_processor(&self) -> bool {
                matches!(self, $(Self::$variant(_))|*)
            }

            #[must_use]
            pub const fn lane_outputs(&self) -> &'static [&'static str] {
                match self {
                    $(Self::$variant(_) => lane_outputs!([] $($port),*),)*
                    _ => &[],
                }
            }

            #[must_use]
            pub fn processor_params(&self) -> Option<crate::processor::ProcessorParams> {
                match self {
                    $(
                        Self::$variant(node) => Some(crate::processor::ProcessorParams::$variant(
                            Clone::clone(&node.settings),
                        )),
                    )*
                    _ => None,
                }
            }

            #[must_use]
            pub fn default_for(kind: &str) -> Option<Self> {
                Some(match kind {
                    "device" => Self::Device(DeviceNode::default()),
                    "recording" => Self::Recording(RecordingNode::default()),
                    "signal_gen" => Self::SignalGen(SignalGenNode::default()),
                    "array" => Self::Array(crate::array::ArrayNode::default()),
                    "gps" => Self::Gps(GpsNode::default()),
                    "channel" => Self::Channel(ChannelNode {
                        channel_type: String::new(),
                        tuning_locked: false,
                    }),
                    "scope" => Self::Scope,
                    "baseband_scope" => Self::BasebandScope,
                    "speaker" => Self::Speaker,
                    "map" => Self::Map,
                    "signal_map" => Self::SignalMap(SignalMapNode::default()),
                    "propagation" => Self::Propagation(PropagationNode::default()),
                    "readout" => Self::Readout,
                    "decoder_log" => Self::DecoderLog,
                    "dmr_trunk" => Self::DmrTrunk(DmrTrunkNode::default()),
                    "spectrum_monitor" => {
                        Self::SpectrumMonitor(crate::SpectrumMonitorNode::default())
                    }
                    "event_output" => Self::EventOutput(EventOutputNode::default()),
                    "event_filter" => Self::EventFilter(EventFilterNode::default()),
                    "audio_fx" => Self::AudioFx(crate::AudioFxNode::default()),
                    "video" => Self::Video,
                    "recorder" => Self::Recorder(RecorderNode::default()),
                    "audio_recorder" => Self::AudioRecorder(RecorderNode::default()),
                    "baseband_recorder" => Self::BasebandRecorder(RecorderNode::default()),
                    "time_machine" => Self::TimeMachine(TimeMachineNode::default()),
                    "network_export" => Self::NetworkExport(NetworkExportNode::default()),
                    "export" => Self::Export,
                    "scanner" => Self::Scanner,
                    "hunt" => Self::Hunt(HuntNode::default()),
                    "satellite" => Self::Satellite(SatelliteNode::default()),
                    $($type_id => Self::$variant($node::default()),)*
                    "triangulation" => Self::Triangulation(TriangulationNode::default()),
                    _ => return None,
                })
            }
        }

        fn processor_ports(kind: &str) -> Option<Vec<PortSpec>> {
            match kind {
                $($type_id => Some(vec![$(port_row!($port $(($note))?)),*]),)*
                _ => None,
            }
        }

        const PROCESSOR_CATALOG: &[(&str, &str, &str)] = &[$(($type_id, $name, $summary)),*];
    };
}
