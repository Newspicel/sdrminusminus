use serde::Serialize;

use super::{
    adsc, airline5z, arinc622, cfb, cpdlc, fpn, media_adv, met, miam, ohma, oooi, position,
    qseries, sublabel,
};

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "app", rename_all = "snake_case")]
pub enum AcarsApp {
    Adsc {
        #[serde(flatten)]
        envelope: arinc622::Envelope,
        #[serde(flatten)]
        message: adsc::AdscMessage,
    },
    Miam {
        #[serde(flatten)]
        frame: miam::MiamFrame,
    },
    Ohma {
        message: serde_json::Value,
    },
    Cpdlc {
        #[serde(flatten)]
        envelope: arinc622::Envelope,
        #[serde(flatten, skip_serializing_if = "Option::is_none")]
        message: Option<cpdlc::CpdlcMessage>,
        payload_hex: String,
    },
    MediaAdvisory(media_adv::MediaAdvisory),
    QSeries(qseries::QSeries),
    Cfb(cfb::Cfb),
    FlightPlan(fpn::FlightPlan),
    Airline5z(airline5z::Airline5z),
}

#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct AppDecode {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sublabel: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mfi: Option<String>,
    #[serde(flatten, skip_serializing_if = "Option::is_none")]
    pub oooi: Option<oooi::Oooi>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub position: Option<position::Position>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub met: Option<met::Met>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub app: Option<AcarsApp>,
}

pub fn decode(label: &str, text: &str, downlink: bool) -> AppDecode {
    let mut out = AppDecode::default();
    let mut body = text;

    if label == "H1" {
        let (sublabel, mfi, rest) = sublabel::extract(text, downlink);
        out.sublabel = sublabel;
        out.mfi = mfi;
        body = rest;
    }

    out.app = match label {
        "A6" | "AA" | "B6" | "BA" => arinc622::parse(body, downlink),
        "H1" => arinc622::parse(body, downlink)
            .or_else(|| fpn::parse(body).map(AcarsApp::FlightPlan))
            .or_else(|| cfb::classify(text).map(AcarsApp::Cfb))
            .or_else(|| ohma::parse(body).map(|message| AcarsApp::Ohma { message })),
        "MA" => miam::parse(body).map(|frame| AcarsApp::Miam { frame }),
        "SA" => media_adv::parse(body).map(AcarsApp::MediaAdvisory),
        "5Z" => airline5z::parse(body).map(AcarsApp::Airline5z),
        _ => qseries::classify(label).map(AcarsApp::QSeries),
    };

    out.oooi = oooi::decode(label, text);
    out.position = position::decode(label, text);
    out.met = met::decode(label, text);
    out
}

#[cfg(test)]
pub fn summary(app: &AcarsApp) -> Option<String> {
    match app {
        AcarsApp::Adsc { message, .. } => message.summary(),
        AcarsApp::Cpdlc { envelope, .. } => Some(format!(
            "CPDLC {} ({})",
            envelope.imi.as_str(),
            envelope.gs_addr
        )),
        AcarsApp::MediaAdvisory(m) => Some(format!(
            "MEDIA-ADV link {} {} at {}",
            m.current_link,
            if m.established { "established" } else { "lost" },
            m.time
        )),
        AcarsApp::QSeries(q) => Some(format!("{} {}", q.label, q.description)),
        AcarsApp::Cfb(c) => Some(if c.subtype.is_empty() {
            format!("CFB {}", c.description)
        } else {
            format!("CFB {} ({})", c.subtype, c.description)
        }),
        AcarsApp::FlightPlan(fp) => Some(format!(
            "FPN {}{}->{}",
            fp.flight_number
                .as_deref()
                .map(|f| format!("{f} "))
                .unwrap_or_default(),
            fp.origin.as_deref().unwrap_or("?"),
            fp.destination.as_deref().unwrap_or("?"),
        )),
        AcarsApp::Airline5z(a) => Some(airline5z_summary(a)),
        AcarsApp::Miam { frame } => Some(miam_summary(frame)),
        AcarsApp::Ohma { message } => Some(format!(
            "OHMA {}",
            message
                .pointer("/message/sysid")
                .or_else(|| message.get("version"))
                .map(|v| v.to_string().trim_matches('"').to_owned())
                .unwrap_or_default()
        )),
    }
}

#[cfg(test)]
fn airline5z_summary(a: &airline5z::Airline5z) -> String {
    match a {
        airline5z::Airline5z::Text { text } => format!("5Z TXT {text}"),
        airline5z::Airline5z::Typed {
            message_type,
            description,
            ..
        } => format!("5Z {message_type} ({description})"),
    }
}

#[cfg(test)]
fn miam_summary(frame: &miam::MiamFrame) -> String {
    match frame {
        miam::MiamFrame::SingleTransfer(p) => format!(
            "MIAM v{} {}{}{}",
            p.version,
            p.pdu_type,
            p.app_id
                .as_deref()
                .map(|a| format!(" app={a}"))
                .unwrap_or_default(),
            if p.compressed {
                format!(" ({} bytes inflated)", p.data_len)
            } else {
                String::new()
            }
        ),
        miam::MiamFrame::FileTransferReq { file_id, file_size } => {
            format!("MIAM file-transfer-req id={file_id} size={file_size}")
        }
        miam::MiamFrame::FileSegment {
            file_id,
            segment_id,
            ..
        } => format!("MIAM file-segment id={file_id} seg={segment_id}"),
        f => format!(
            "MIAM {}",
            serde_json::json!(f)["frame"].as_str().unwrap_or("frame")
        ),
    }
}
