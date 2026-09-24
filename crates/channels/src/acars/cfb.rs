use serde::Serialize;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Cfb {
    pub subtype: String,
    pub kind: CfbKind,
    pub description: &'static str,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CfbKind {
    ApmReport,
    AtaFault,
    RealtimeFailure,
    FlightDeckEffect,
    EngineStatus,
    VibrationReport,
    MaintenancePlanning,
    Warning,
    Lights,
    McduPage,
    FailureRecord,
    Generic,
}

pub fn classify(text: &str) -> Option<Cfb> {
    let rest = text.strip_prefix("#CFB")?;

    let (subtype, kind, description): (&str, CfbKind, &str) =
        if rest.starts_with(".01") || rest.starts_with(".1") {
            let tok = if rest.starts_with(".01") { ".01" } else { ".1" };
            (tok, CfbKind::FailureRecord, "Failure/fault/warning record")
        } else if rest.starts_with("APM_REPORT") {
            (
                "APM_REPORT",
                CfbKind::ApmReport,
                "Aircraft Performance Monitoring / ACMF snapshot report",
            )
        } else if rest.starts_with("APM") {
            (
                "APM",
                CfbKind::ApmReport,
                "Aircraft Performance Monitoring report",
            )
        } else if rest.starts_with("ATA") {
            ("ATA", CfbKind::AtaFault, "ATA-chapter fault report")
        } else if rest.starts_with("FDE") {
            ("FDE", CfbKind::FlightDeckEffect, "Flight Deck Effect")
        } else if rest.starts_with("FLR") {
            ("FLR", CfbKind::RealtimeFailure, "Realtime failure")
        } else if rest.starts_with("ECT") {
            ("ECT", CfbKind::EngineStatus, "Engine status / fault report")
        } else if rest.starts_with("LIGHTS") {
            ("LIGHTS", CfbKind::Lights, "Lighting status / fault report")
        } else if rest.starts_with("MIL") {
            (
                "MIL",
                CfbKind::VibrationReport,
                "Engine spool vibration units report",
            )
        } else if rest.starts_with("MPF") {
            (
                "MPF",
                CfbKind::MaintenancePlanning,
                "Maintenance Planning Function",
            )
        } else if rest.starts_with("PAGE") {
            ("PAGE", CfbKind::McduPage, "MDC report page")
        } else if rest.starts_with("WRN") {
            ("WRN", CfbKind::Warning, "Warning")
        } else if rest.starts_with("AL") {
            (
                "AL",
                CfbKind::EngineStatus,
                "Air temperature / FADEC bleed status report",
            )
        } else {
            ("", CfbKind::Generic, "Crew Flight Bag message")
        };

    Some(Cfb {
        subtype: subtype.to_owned(),
        kind,
        description,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn apm_report() {
        let c = classify("#CFBAPM_REPORT_A_20200805180631S.CSV").unwrap();
        assert_eq!(c.subtype, "APM_REPORT");
        assert_eq!(c.kind, CfbKind::ApmReport);
    }

    #[test]
    fn ata_fault() {
        let c = classify("#CFBATA\n\nVIA-1//20DEC//1653//1026//0//DU-6 LOW BRIGHTNESS..").unwrap();
        assert_eq!(c.subtype, "ATA");
        assert_eq!(c.kind, CfbKind::AtaFault);
    }

    #[test]
    fn al_engine_status() {
        let c = classify("#CFBAL AIR TEMP            32.5 C").unwrap();
        assert_eq!(c.subtype, "AL");
        assert_eq!(c.kind, CfbKind::EngineStatus);
    }

    #[test]
    fn fde_flight_deck_effect() {
        let c = classify("#CFBFDE1807300805ABD").unwrap();
        assert_eq!(c.subtype, "FDE");
        assert_eq!(c.kind, CfbKind::FlightDeckEffect);
    }

    #[test]
    fn ect_engine_status() {
        let c = classify("#CFBECT FAULT     CH-A").unwrap();
        assert_eq!(c.subtype, "ECT");
        assert_eq!(c.kind, CfbKind::EngineStatus);
    }

    #[test]
    fn flr_realtime_failure() {
        let c = classify("#CFBFLR/FR19121418400034433406TCAS (1SG)").unwrap();
        assert_eq!(c.subtype, "FLR");
        assert_eq!(c.kind, CfbKind::RealtimeFailure);
        assert_eq!(c.description, "Realtime failure");
    }

    #[test]
    fn lights_report() {
        let c = classify("#CFBLIGHTS\n R PRIM NAV LT      DS23").unwrap();
        assert_eq!(c.subtype, "LIGHTS");
        assert_eq!(c.kind, CfbKind::Lights);
    }

    #[test]
    fn mil_vibration_report() {
        let c = classify("#CFBMIL\nR N1 VIBES                 0.2 MIL").unwrap();
        assert_eq!(c.subtype, "MIL");
        assert_eq!(c.kind, CfbKind::VibrationReport);
    }

    #[test]
    fn mpf_maintenance_planning() {
        let c = classify("#CFBMPF/               /AN.N660AW/FIAAL652").unwrap();
        assert_eq!(c.subtype, "MPF");
        assert_eq!(c.kind, CfbKind::MaintenancePlanning);
    }

    #[test]
    fn page_mdc_report() {
        let c = classify("#CFBPAGE 00001\nMDC REPORT: ENGINE TREND").unwrap();
        assert_eq!(c.subtype, "PAGE");
        assert_eq!(c.kind, CfbKind::McduPage);
    }

    #[test]
    fn wrn_warning() {
        let c = classify("#CFBWRN/WN19121418390034000006NAV TCAS FAULT").unwrap();
        assert_eq!(c.subtype, "WRN");
        assert_eq!(c.kind, CfbKind::Warning);
    }

    #[test]
    fn dotted_failure_record() {
        let c = classify("#CFB.1/FLR/FR1602082254 27513406ADR1 X2,ADR3X,ADR2X").unwrap();
        assert_eq!(c.subtype, ".1");
        assert_eq!(c.kind, CfbKind::FailureRecord);
    }

    #[test]
    fn generic_cfb_with_slash() {
        let c = classify("#CFB/1315//38//0//RA-1").unwrap();
        assert_eq!(c.kind, CfbKind::Generic);
    }

    #[test]
    fn non_cfb_rejected() {
        assert!(classify("#DFB/M1 ENGINE DATA").is_none());
        assert!(classify("POSN38160W077075").is_none());
        assert!(classify("").is_none());
    }
}
