use serde::Serialize;

const TYPES: &[(&str, &str)] = &[
    ("B1", "Request Weight and Balance"),
    ("B3", "Request Departure Clearance"),
    ("CD", "Weight and Balance"),
    ("CG", "Request Pre-departure clearance, PDC"),
    ("CM", "Crew Scheduling"),
    ("C3", "Off Message"),
    ("C4", "Flight Dispatch"),
    ("C5", "Maintenance Message"),
    ("C6", "Customer Service"),
    ("10", "PIREP"),
    ("C11", "International PIREP"),
    ("DS", "Late Message"),
    ("D3", "Holding Pattern Message"),
    ("D6", "From-To + Date"),
    ("D7", "From-To + Alternate + Time"),
    ("EO", "In Range"),
    ("ET", "Expected Time"),
    ("PW", "Position Weather"),
    ("RL", "Request Release"),
    ("R3", "Request HOWGOZIT Message"),
    ("R4", "Request the Latest POSBD"),
    ("TC", "From-To Fuel"),
    ("WB", "From-To"),
    ("W1", "Request Weather for City"),
];

fn type_description(t: &str) -> Option<&'static str> {
    TYPES.iter().find(|(k, _)| *k == t).map(|(_, v)| *v)
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Airline5z {
    Text {
        text: String,
    },
    Typed {
        message_type: String,
        description: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        airline: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        origin: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        destination: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        arrival_runway: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        day: Option<u8>,
    },
}

pub fn parse(text: &str) -> Option<Airline5z> {
    let mut lines = text.split("\r\n");
    let first = lines.next()?;

    if first == "/TXT" {
        let rest: Vec<&str> = lines.collect();
        return Some(Airline5z::Text {
            text: rest.join("\r\n"),
        });
    }

    let data: Vec<&str> = first.split('/').collect();
    if data.len() < 2 {
        return None;
    }
    let header: Vec<&str> = data[1].split(' ').filter(|s| !s.is_empty()).collect();
    let ty = *header.first()?;
    let description = type_description(ty)?;

    let mut origin = None;
    let mut destination = None;
    let mut arrival_runway = None;
    let mut day = None;

    if (ty == "B3" || ty == "C3") && header.len() >= 2 && header[1].len() >= 6 {
        let pair = header[1];
        origin = pair.get(..3).map(str::to_string);
        destination = pair.get(3..6).map(str::to_string);
        if ty == "B3" {
            if let Some(d) = header.get(2) {
                day = d.parse().ok();
            }
            if let Some(rwy) = header.get(3) {
                arrival_runway = rwy.strip_prefix('R').map(|s| s.to_string());
            }
        }
    }

    Some(Airline5z::Typed {
        message_type: ty.to_string(),
        description: description.to_string(),
        airline: Some("United Airlines".to_string()),
        origin,
        destination,
        arrival_runway,
        day,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn txt_free_text() {
        let m = parse("/TXT\r\nDID U GET THE TIMES").unwrap();
        assert_eq!(
            m,
            Airline5z::Text {
                text: "DID U GET THE TIMES".into()
            }
        );
    }

    #[test]
    fn b3_request_departure_clearance() {
        let m = parse("/B3 DCAORD 14 R27C").unwrap();
        let Airline5z::Typed {
            message_type,
            description,
            airline,
            origin,
            destination,
            arrival_runway,
            day,
        } = m
        else {
            panic!("expected typed: {m:?}");
        };
        assert_eq!(message_type, "B3");
        assert_eq!(description, "Request Departure Clearance");
        assert_eq!(airline.as_deref(), Some("United Airlines"));
        assert_eq!(origin.as_deref(), Some("DCA"));
        assert_eq!(destination.as_deref(), Some("ORD"));
        assert_eq!(arrival_runway.as_deref(), Some("27C"));
        assert_eq!(day, Some(14));
    }

    #[test]
    fn b3_variant_1_with_trailing_token() {
        let m = parse("/B3 ATLIAD 14 R1C G1273").unwrap();
        let Airline5z::Typed {
            origin,
            destination,
            arrival_runway,
            day,
            ..
        } = m
        else {
            panic!("expected typed");
        };
        assert_eq!(origin.as_deref(), Some("ATL"));
        assert_eq!(destination.as_deref(), Some("IAD"));
        assert_eq!(arrival_runway.as_deref(), Some("1C"));
        assert_eq!(day, Some(14));
    }

    #[test]
    fn c6_customer_service_typed() {
        let m = parse("/C6 ORDCHS CHS HI...NO APU TONIGHT\r\nWILL NEED GROUND PWR").unwrap();
        let Airline5z::Typed {
            message_type,
            description,
            ..
        } = &m
        else {
            panic!("expected typed");
        };
        assert_eq!(message_type, "C6");
        assert_eq!(description, "Customer Service");
    }

    #[test]
    fn unknown_type_not_decoded() {
        assert!(parse("/ZZ SOMETHING").is_none());
        assert!(parse("not a 5z message").is_none());
    }
}
